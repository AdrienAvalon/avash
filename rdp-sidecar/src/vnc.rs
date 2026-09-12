//! Session VNC (RFB 3.8) : même poste local et même protocole avec l'interface
//! que le RDP, un autre dialogue avec le serveur.
//!
//! Le client `vnc-rs` (copie portée dans `vendor/`) mène la poignée de main,
//! l'authentification VNC classique et le décodage (ZRLE, CopyRect, Raw) ; ce
//! module tient l'image, la zone sale et le cadencement sur accusé de
//! réception, exactement comme `session.rs` pour le RDP. Deux différences
//! visibles de l'interface : le clavier voyage en keysyms X11 (message [14],
//! le caractère tapé plutôt que la touche physique, ce que RFB attend), et le
//! redimensionnement à la demande n'existe pas (le serveur décide de sa
//! taille ; l'interface adapte le canvas).
//!
//! Le RFB classique ne chiffre rien et ne présente rien à vérifier ;
//! `SECURITY.md` recommande un tunnel SSH pour tout ce qui sort du réseau
//! local. En revanche, dès qu'une session VeNCrypt a réussi sur un serveur, sa
//! clé publique est épinglée (`vnc:<hôte>:<port>`, voir `vnc_tls`) : les
//! connexions suivantes exigent alors TLS et refusent toute rétrogradation vers
//! le RFB en clair (modèle HSTS), sinon un interposeur retirant VeNCrypt de la
//! liste (échangée en clair) ferait fuiter la réponse DES puis la session
//! entière. Il n'y a donc « aucune empreinte à épingler » que tant qu'aucune
//! session VeNCrypt n'a eu lieu.

use crate::acces_local::{etablir_poste, Poste};
use crate::args::{taille_sure, Args};
use crate::trames::{ajouter_rect, frame_msg, frames_msg, nouvelle_taille};
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use ironrdp::graphics::image_processing::PixelFormat;
use ironrdp::pdu::geometry::InclusiveRectangle;
use ironrdp::session::image::DecodedImage;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use vnc::{ClientKeyEvent, ClientMouseEvent, Rect, VncConnector, VncEncoding, VncEvent, X11Event};

/// Filet anti-gel, comme en RDP : un accusé perdu ne fige pas l'affichage.
const ACK_TIMEOUT: Duration = Duration::from_millis(250);
/// Une connexion (TCP, poignée de main, authentification) doit aboutir ou dire
/// pourquoi ; un serveur muet ne laisse pas un onglet figé.
const DELAI_CONNEXION: Duration = Duration::from_secs(25);

/// Ce que l'interface envoie sur le canal local, une fois décodé.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Entree {
    Souris {
        x: u16,
        y: u16,
    },
    /// `bouton` est celui du DOM : 0 gauche, 1 milieu, 2 droit.
    Bouton {
        bouton: u8,
        enfonce: bool,
        x: u16,
        y: u16,
    },
    Molette {
        delta: i16,
    },
    Touche {
        keysym: u32,
        enfonce: bool,
    },
}

/// Décode un message d'entrée de l'interface. Les messages qui ne concernent
/// pas le serveur (verrous, redimensionnement) et les messages malformés
/// donnent `None` : un client authentifié reste un client, et un bogue
/// d'interface ne doit pas faire tomber une session.
pub(crate) fn entree(b: &[u8]) -> Option<Entree> {
    let u16le = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    match b.first().copied() {
        Some(1) if b.len() >= 5 => Some(Entree::Souris {
            x: u16le(1),
            y: u16le(3),
        }),
        Some(2) if b.len() >= 7 => Some(Entree::Bouton {
            bouton: b[1],
            enfonce: b[2] != 0,
            x: u16le(3),
            y: u16le(5),
        }),
        Some(3) if b.len() >= 3 => Some(Entree::Molette {
            delta: i16::from_le_bytes([b[1], b[2]]),
        }),
        Some(14) if b.len() >= 6 => Some(Entree::Touche {
            keysym: u32::from_le_bytes([b[1], b[2], b[3], b[4]]),
            enfonce: b[5] != 0,
        }),
        _ => None,
    }
}

/// L'état du pointeur tel que RFB le veut : une position et un masque de
/// boutons envoyés ensemble à chaque événement. Bits : 1 gauche, 2 milieu,
/// 4 droit, 8 molette vers le haut, 16 molette vers le bas.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Pointeur {
    x: u16,
    y: u16,
    boutons: u8,
}

impl Pointeur {
    /// Les événements RFB à émettre pour une entrée de l'interface. Une
    /// entrée clavier n'en produit aucun ; un cran de molette en produit deux
    /// (appui puis relâchement du bouton virtuel), à la dernière position.
    pub(crate) fn appliquer(&mut self, e: &Entree) -> Vec<ClientMouseEvent> {
        match *e {
            Entree::Souris { x, y } => {
                self.x = x;
                self.y = y;
                vec![self.etat()]
            }
            Entree::Bouton {
                bouton,
                enfonce,
                x,
                y,
            } => {
                self.x = x;
                self.y = y;
                let bit = match bouton {
                    0 => 1,
                    1 => 2,
                    2 => 4,
                    _ => return vec![self.etat()],
                };
                if enfonce {
                    self.boutons |= bit;
                } else {
                    self.boutons &= !bit;
                }
                vec![self.etat()]
            }
            Entree::Molette { delta } => {
                let bit = if delta > 0 { 8 } else { 16 };
                let mut appui = self.etat();
                appui.bottons |= bit;
                vec![appui, self.etat()]
            }
            Entree::Touche { .. } => Vec::new(),
        }
    }

    fn etat(&self) -> ClientMouseEvent {
        ClientMouseEvent {
            position_x: self.x,
            position_y: self.y,
            bottons: self.boutons,
        }
    }
}

/// Copie un rectangle de l'image vers un autre (codage CopyRect). Un
/// rectangle qui déborde est ignoré : le serveur est une entrée non fiable, et
/// une copie partielle vaudrait moins qu'aucune.
pub(crate) fn copier(image: &mut DecodedImage, dst: Rect, src: Rect) -> bool {
    let (l, h) = (usize::from(image.width()), usize::from(image.height()));
    let (w, ht) = (usize::from(dst.width), usize::from(dst.height));
    let dans = |r: Rect| usize::from(r.x) + w <= l && usize::from(r.y) + ht <= h;
    if w == 0 || ht == 0 || !dans(src) || !dans(dst) {
        return false;
    }
    let mut pixels = Vec::with_capacity(w * ht * 4);
    let data = image.data();
    for y in 0..ht {
        let debut = ((usize::from(src.y) + y) * l + usize::from(src.x)) * 4;
        pixels.extend_from_slice(&data[debut..debut + w * 4]);
    }
    image.peindre_rgba(dst.x, dst.y, dst.width, dst.height, &pixels);
    true
}

/// Rectangle inclusif d'IronRDP pour un rectangle RFB.
fn inclusif(r: Rect) -> InclusiveRectangle {
    InclusiveRectangle {
        left: r.x,
        top: r.y,
        right: r.x.saturating_add(r.width).saturating_sub(1),
        bottom: r.y.saturating_add(r.height).saturating_sub(1),
    }
}

/// Un message d'erreur pour l'utilisateur : le mot de passe refusé se dit tel
/// quel, le reste porte la raison du client.
fn message(e: vnc::VncError) -> anyhow::Error {
    match e {
        vnc::VncError::WrongPassword => anyhow::anyhow!("Mot de passe VNC refusé."),
        vnc::VncError::NoPassword => {
            anyhow::anyhow!("Le serveur VNC demande un mot de passe, et aucun n'a été donné.")
        }
        vnc::VncError::General(m) if m.contains("has not been implemented") => anyhow::anyhow!(
            "Le serveur n'accepte que des authentifications que ce client ne parle pas \
             (TLS anonyme, RSA-AES). Autorise VeNCrypt avec certificat (X.509) ou \
             l'authentification VNC classique, ou passe par un tunnel SSH."
        ),
        // Le serveur n'annonce que des types de sécurité hors de ce que le
        // client parle (ARD macOS, MS-Logon UltraVNC, RealVNC…) : on nomme ce
        // qui est accepté. Trouvé par l'audit du 7 septembre 2026.
        vnc::VncError::General(m) if m.contains("types de sécurité inconnus") => anyhow::anyhow!(
            "{m}. Ce client accepte : sans authentification, l'authentification VNC classique, \
             ou VeNCrypt avec certificat (X.509)."
        ),
        // Le serveur a choisi (RFB 3.3) ou n'offre qu'un type de sécurité que
        // ce client ne parle pas : phrase française plutôt que le « Unknown VNC
        // security type » de la bibliothèque. Trouvé par l'audit du 7 sept. 2026.
        vnc::VncError::InvalidSecurityTyep(t) => anyhow::anyhow!(
            "Le serveur impose un type de sécurité VNC que ce client ne parle pas ({t}). \
             Types acceptés : sans authentification, authentification VNC classique, \
             ou VeNCrypt avec certificat (X.509)."
        ),
        // Nos propres messages (VeNCrypt, certificat épinglé) : tels quels,
        // sans le « VNC Error with message » que la bibliothèque colle devant.
        vnc::VncError::General(m) => anyhow::anyhow!("{m}"),
        autre => anyhow::anyhow!("{autre}"),
    }
}

/// Ce que le presse-papiers du poste doit envoyer au serveur VNC pour un message
/// de l'interface. En RFB, à la différence du RDP, il n'y a pas de phase de
/// demande : un `ClientCutText` part réellement sur le fil dès qu'on l'émet.
/// L'annonce `[8]` (que l'interface pousse à l'ouverture, au focus et au
/// changement d'onglet) ne fait donc que mémoriser le texte du poste ; il ne part
/// au serveur que sur un collage explicite `[22]` (Ctrl+V / Maj+Inser), et
/// seulement si le partage est actif. Renvoie le texte à émettre en `CopyText`,
/// ou `None`. Trouvé par l'audit du 7 septembre 2026 : le chemin VNC envoyait
/// aussitôt, sur `[8]`, le presse-papiers du poste — souvent un mot de passe
/// fraîchement copié — au serveur, sans le moindre geste de collage, dès la
/// connexion et à chaque focus (fuite du contenu vers l'opérateur du serveur, en
/// clair en RFB classique).
/// Le message `[8]` d'un texte copié par le serveur VNC, ou `None` si le
/// partage est coupé ou si le texte dépasse le plafond de l'interface.
///
/// Trouvé par l'audit du 12 septembre 2026 (C-sidecar-5) : le texte n'était
/// borné que par le tampon des pixels du paquet porté (256 Mio) puis relayé
/// tel quel ; 200 Mio de `ServerCutText` devenaient un message de 400 Mio pour
/// la webview, qui gelait toute l'application. Même plafond qu'en RDP, et le
/// refus se dit dans le journal plutôt que de passer en silence.
fn message_texte_distant(texte: &str, partage: bool) -> Option<Vec<u8>> {
    if !partage {
        return None;
    }
    if texte.len() > crate::presse_papiers::TEXTE_VERS_INTERFACE_MAX {
        eprintln!(
            "vnc : texte du presse-papiers distant ignoré ({} octets, plus que le plafond de {})",
            texte.len(),
            crate::presse_papiers::TEXTE_VERS_INTERFACE_MAX
        );
        return None;
    }
    let mut m = Vec::with_capacity(1 + texte.len());
    m.push(8u8);
    m.extend_from_slice(texte.as_bytes());
    Some(m)
}

fn presse_papiers_vers_serveur(
    memoire: &mut Option<String>,
    b: &[u8],
    partage: bool,
) -> Option<String> {
    match b.first().copied() {
        // [8] ANNONCE : mémoriser sans rien envoyer.
        Some(8) => {
            if let Ok(t) = std::str::from_utf8(&b[1..]) {
                *memoire = Some(t.to_owned());
            }
            None
        }
        // [22] COLLER : le seul moment où le presse-papiers du poste part.
        Some(22) if partage => memoire.clone(),
        _ => None,
    }
}

pub async fn executer(args: &Args) -> Result<()> {
    let tcp = tokio::time::timeout(
        DELAI_CONNEXION,
        TcpStream::connect((args.host.as_str(), args.port)),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "Le serveur n'a pas répondu en {} s.",
            DELAI_CONNEXION.as_secs()
        )
    })?
    .with_context(|| format!("connexion à {}:{}", args.host, args.port))?;
    tcp.set_nodelay(true).ok();
    let pass = args.pass.clone();
    // VeNCrypt : si le serveur l'offre, le flux passe sous TLS et le
    // certificat est épinglé (vnc_tls) ; sinon, l'authentification VNC
    // classique, en clair, comme avant.
    let monteur = crate::vnc_tls::monteur(&args.host, args.port);
    // Modèle HSTS : si une session VeNCrypt a déjà épinglé ce serveur (une ligne
    // `vnc:<hôte>:<port>` existe), on exige TLS et l'on refuse toute
    // rétrogradation en RFB clair — un interposeur ne peut pas contourner
    // l'épinglage en retirant VeNCrypt de la liste. Trouvé par l'audit du
    // 7 septembre 2026.
    let cle_tls = format!("vnc:{}:{}", args.host, args.port);
    // Fichier de confiance illisible : on refuse plutôt que de croire à l'absence
    // d'épinglage (qui rétrograderait vers du RFB clair). Voir `empreinte_memorisee`.
    let exige_tls = crate::empreintes::empreinte_memorisee(&cle_tls)
        .context("fichier de confiance illisible, connexion refusée")?
        .is_some();
    let client = tokio::time::timeout(DELAI_CONNEXION, async move {
        VncConnector::new(crate::vnc_tls::MaybeTls::Clair(tcp))
            .set_tls_upgrader(monteur)
            .exiger_tls(exige_tls.then_some(cle_tls))
            .set_auth_method(async move { Ok(pass) })
            // L'ordre est une préférence annoncée au serveur : ZRLE d'abord
            // (sans perte, compact), CopyRect pour les déplacements, Raw parce
            // que le protocole l'exige, et la taille de bureau pour suivre un
            // serveur qui change de résolution.
            .add_encoding(VncEncoding::Zrle)
            .add_encoding(VncEncoding::CopyRect)
            .add_encoding(VncEncoding::Raw)
            .add_encoding(VncEncoding::DesktopSizePseudo)
            .allow_shared(true)
            // Les pixels arrivent [r, g, b, x] : le format de l'image, à
            // l'alpha près.
            .set_pixel_format(vnc::PixelFormat::rgba())
            .build()?
            .try_start()
            .await?
            .finish()
    })
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "La poignée de main VNC n'a pas abouti en {} s.",
            DELAI_CONNEXION.as_secs()
        )
    })?
    .map_err(message)?;
    let mut evenements = client
        .take_events()
        .await
        .context("file des événements du client VNC")?;

    // Le premier événement est la taille du cadre, envoyée à l'entrée.
    let (w, h) = loop {
        match evenements.recv().await {
            Some(VncEvent::SetResolution(s)) => break taille_sure(s.width, s.height)?,
            Some(VncEvent::Error(m)) => anyhow::bail!("{m}"),
            Some(_) => {}
            None => anyhow::bail!("Le serveur a fermé la connexion avant d'annoncer son cadre."),
        }
    };
    eprintln!("connecté (VNC) : {w}x{h}");
    let mut image = DecodedImage::new(PixelFormat::RgbA32, w, h);

    let mut poste: Option<Poste> = None;
    let Poste { sink, stream, .. } = etablir_poste(&mut poste).await?;
    let mut hello = vec![1u8];
    hello.extend_from_slice(&w.to_le_bytes());
    hello.extend_from_slice(&h.to_le_bytes());
    sink.send(Message::Binary(hello.into())).await?;

    let mut pointeur = Pointeur::default();
    let mut partage_clip = true;
    // Le presse-papiers du poste mémorisé : l'annonce [8] le remplit, le collage
    // explicite [22] le pousse au serveur (voir presse_papiers_vers_serveur).
    let mut memoire_clip: Option<String> = None;
    let mut dirty: Vec<InclusiveRectangle> = Vec::new();
    let mut awaiting_ack = false;
    let mut en_pause = false;
    let mut last_send = Instant::now();
    let (mut stat_frames, mut stat_bytes): (u32, u64) = (0, 0);
    let mut lat_ms: f32 = 0.0;
    let mut stat_window = Instant::now();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Le serveur n'envoie que ce qu'on lui demande : après chaque lot d'images
    // on redemande la suite (incrémental), sauf en pause.
    let mut demande_en_vol = true;

    #[allow(clippy::items_after_statements)]
    macro_rules! flush_dirty {
        () => {
            if !awaiting_ack && !en_pause && !dirty.is_empty() {
                let msg = frames_msg(&image, &dirty);
                dirty.clear();
                stat_bytes += msg.len() as u64;
                stat_frames += 1;
                sink.send(Message::Binary(msg.into()))
                    .await
                    .context("envoi frame")?;
                awaiting_ack = true;
                last_send = Instant::now();
            }
        };
    }
    #[allow(clippy::items_after_statements)]
    macro_rules! demander_la_suite {
        () => {
            if !en_pause && !demande_en_vol {
                client.input(X11Event::Refresh).await.map_err(message)?;
                demande_en_vol = true;
            }
        };
    }

    loop {
        tokio::select! {
            biased;
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Binary(b))) if b.first() == Some(&6) => {
                        let rtt = last_send.elapsed().as_secs_f32() * 1000.0;
                        lat_ms = if lat_ms == 0.0 { rtt } else { lat_ms.mul_add(0.8, rtt * 0.2) };
                        awaiting_ack = false;
                        flush_dirty!();
                    }
                    Some(Ok(Message::Binary(b))) if b.first() == Some(&9) => {
                        // REFRESH : l'onglet revient au premier plan, son canvas
                        // peut être vide ; on renvoie l'image entière, puis on
                        // redemande au serveur ce qui a changé pendant la pause.
                        let full = InclusiveRectangle {
                            left: 0,
                            top: 0,
                            right: image.width().saturating_sub(1),
                            bottom: image.height().saturating_sub(1),
                        };
                        let msg = frame_msg(&image, &full);
                        stat_bytes += msg.len() as u64;
                        stat_frames += 1;
                        sink.send(Message::Binary(msg.into())).await.context("envoi refresh")?;
                        awaiting_ack = true;
                        last_send = Instant::now();
                        dirty.clear();
                        en_pause = false;
                        demander_la_suite!();
                    }
                    Some(Ok(Message::Binary(b))) if b.first() == Some(&11) && b.len() >= 2 => {
                        en_pause = b[1] != 0;
                        if !en_pause {
                            flush_dirty!();
                            demander_la_suite!();
                        }
                    }
                    Some(Ok(Message::Binary(b))) if b.first() == Some(&12) && b.len() >= 2 => {
                        partage_clip = b[1] != 0;
                    }
                    Some(Ok(Message::Binary(b))) if b.first() == Some(&8) || b.first() == Some(&22) => {
                        if let Some(texte) = presse_papiers_vers_serveur(&mut memoire_clip, &b, partage_clip) {
                            client.input(X11Event::CopyText(texte)).await.map_err(message)?;
                        }
                    }
                    Some(Ok(Message::Binary(b))) => {
                        // [5] RESIZE et [10] LOCKS n'ont pas d'équivalent RFB :
                        // le serveur décide de sa taille, et les verrous sont
                        // des touches comme les autres.
                        if let Some(e) = entree(&b) {
                            if let Entree::Touche { keysym, enfonce } = e {
                                client
                                    .input(X11Event::KeyEvent(ClientKeyEvent { keycode: keysym, down: enfonce }))
                                    .await
                                    .map_err(message)?;
                            } else {
                                for ev in pointeur.appliquer(&e) {
                                    client.input(X11Event::PointerEvent(ev)).await.map_err(message)?;
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                    Some(Ok(_)) => {}
                }
            }
            ev = evenements.recv() => {
                let Some(premier) = ev else {
                    anyhow::bail!("Le serveur a fermé la connexion.");
                };
                // Tout ce qui attend déjà part dans la même trame.
                let mut lot = vec![premier];
                while let Ok(e) = evenements.try_recv() {
                    lot.push(e);
                }
                let mut image_recue = false;
                let mut fin_de_maj = false;
                for e in lot {
                    match e {
                        VncEvent::SetResolution(s) => {
                            let (nl, nh) = taille_sure(s.width, s.height)?;
                            client.set_screen(nl, nh).await;
                            if nouvelle_taille(&mut image, &mut dirty, &mut awaiting_ack, nl, nh) {
                                let mut msg = vec![1u8];
                                msg.extend_from_slice(&nl.to_le_bytes());
                                msg.extend_from_slice(&nh.to_le_bytes());
                                sink.send(Message::Binary(msg.into())).await.context("annonce taille")?;
                            }
                        }
                        VncEvent::RawImage(rect, mut pixels) => {
                            // Un rectangle vide (largeur ou hauteur nulle, que
                            // certains serveurs émettent à la marge) ne peint
                            // rien et, en x>0/y>0, donnerait une zone sale
                            // dégénérée (right < left) : on l'écarte comme le
                            // fait déjà `copier`. Trouvé par l'audit du
                            // 7 septembre 2026.
                            if rect.width == 0 || rect.height == 0 {
                                continue;
                            }
                            // Le quatrième octet est du remplissage côté
                            // serveur (souvent 0) : l'interface peint en RGBA,
                            // un alpha nul ferait un trou.
                            for px in pixels.as_chunks_mut::<4>().0 {
                                px[3] = 255;
                            }
                            image.peindre_rgba(rect.x, rect.y, rect.width, rect.height, &pixels);
                            ajouter_rect(&mut dirty, &inclusif(rect));
                            image_recue = true;
                        }
                        VncEvent::Copy(dst, src) => {
                            if copier(&mut image, dst, src) {
                                ajouter_rect(&mut dirty, &inclusif(dst));
                                image_recue = true;
                            }
                        }
                        VncEvent::Text(texte) => {
                            if let Some(m) = message_texte_distant(&texte, partage_clip) {
                                sink.send(Message::Binary(m.into())).await.context("envoi presse-papiers")?;
                            }
                        }
                        VncEvent::UpdateDone => fin_de_maj = true,
                        VncEvent::Error(m) => anyhow::bail!("{m}"),
                        // Curseur et JPEG ne sont pas demandés ; cloche,
                        // format de pixel : sans effet.
                        _ => {}
                    }
                }
                if image_recue {
                    flush_dirty!();
                }
                // La fin d'une mise à jour, même sans un seul pixel (changement
                // de résolution : DesktopSize seul, ou lot à zéro rectangle),
                // honore la demande en vol : on peut redemander. Sans ça une
                // telle mise à jour laissait `demande_en_vol` à `true` pour
                // toujours et figeait le bureau (audit du 7 septembre 2026).
                if fin_de_maj {
                    demande_en_vol = false;
                    demander_la_suite!();
                }
            }
            _ = tick.tick() => {
                if awaiting_ack && last_send.elapsed() > ACK_TIMEOUT {
                    awaiting_ack = false;
                }
                flush_dirty!();
                demander_la_suite!();
                if stat_window.elapsed() >= Duration::from_secs(1) {
                    let secs = stat_window.elapsed().as_secs_f32();
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
                    let fps = (stat_frames as f32 / secs).round() as u16;
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
                    let kbps = ((stat_bytes as f32 / 1024.0) / secs).round() as u32;
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let lat = lat_ms.round() as u16;
                    let mut m = vec![7u8];
                    m.extend_from_slice(&fps.to_le_bytes());
                    m.extend_from_slice(&kbps.to_le_bytes());
                    m.extend_from_slice(&lat.to_le_bytes());
                    sink.send(Message::Binary(m.into())).await.ok();
                    stat_frames = 0;
                    stat_bytes = 0;
                    stat_window = Instant::now();
                }
            }
        }
    }
    client.close().await.ok();
    Ok(())
}

#[cfg(test)]
mod tests_entrees {
    use super::{entree, Entree, Pointeur};

    #[test]
    fn les_quatre_messages_sont_decodes() {
        assert_eq!(
            entree(&[1, 10, 0, 20, 0]),
            Some(Entree::Souris { x: 10, y: 20 })
        );
        assert_eq!(
            entree(&[2, 2, 1, 1, 0, 2, 0]),
            Some(Entree::Bouton {
                bouton: 2,
                enfonce: true,
                x: 1,
                y: 2
            })
        );
        assert_eq!(
            entree(&[3, 0x88, 0xff]),
            Some(Entree::Molette { delta: -120 })
        );
        assert_eq!(
            entree(&[14, 0x0d, 0xff, 0, 0, 1]),
            Some(Entree::Touche {
                keysym: 0xff0d,
                enfonce: true
            })
        );
    }

    /// Redimensionnement, verrous, scancodes RDP : rien pour un serveur RFB.
    #[test]
    fn les_messages_sans_equivalent_ne_donnent_rien() {
        assert_eq!(entree(&[5, 0, 4, 0, 3]), None);
        assert_eq!(entree(&[10, 1]), None);
        assert_eq!(entree(&[4, 0x1e, 0, 1]), None);
        assert_eq!(entree(&[]), None);
    }

    /// Même piège qu'en RDP : le message valide coupé trop tôt.
    #[test]
    fn chaque_type_tronque_ne_panique_pas() {
        for type_msg in 0u8..=15 {
            for longueur in 0..12usize {
                let mut b = vec![type_msg];
                b.extend(std::iter::repeat_n(0xa5u8, longueur));
                let _ = entree(&b);
            }
        }
    }

    /// RFB veut l'état complet du pointeur à chaque événement : un clic droit
    /// pendant un glissé gauche garde le bit gauche.
    #[test]
    fn le_masque_des_boutons_suit_les_appuis_et_les_relachements() {
        let mut p = Pointeur::default();
        let e = p.appliquer(&Entree::Bouton {
            bouton: 0,
            enfonce: true,
            x: 5,
            y: 6,
        });
        assert_eq!((e[0].position_x, e[0].position_y, e[0].bottons), (5, 6, 1));
        let e = p.appliquer(&Entree::Bouton {
            bouton: 2,
            enfonce: true,
            x: 7,
            y: 8,
        });
        assert_eq!(e[0].bottons, 1 | 4);
        let e = p.appliquer(&Entree::Souris { x: 9, y: 9 });
        assert_eq!((e[0].position_x, e[0].bottons), (9, 5));
        let e = p.appliquer(&Entree::Bouton {
            bouton: 0,
            enfonce: false,
            x: 9,
            y: 9,
        });
        assert_eq!(e[0].bottons, 4);
        // Un bouton inconnu (X1, X2) ne touche pas au masque.
        let e = p.appliquer(&Entree::Bouton {
            bouton: 4,
            enfonce: true,
            x: 9,
            y: 9,
        });
        assert_eq!(e[0].bottons, 4);
    }

    /// Un cran de molette est un bouton virtuel : appui puis relâchement, à la
    /// dernière position connue, sans perdre les boutons tenus.
    #[test]
    fn un_cran_de_molette_fait_un_appui_et_un_relachement() {
        let mut p = Pointeur::default();
        p.appliquer(&Entree::Bouton {
            bouton: 0,
            enfonce: true,
            x: 3,
            y: 4,
        });
        let e = p.appliquer(&Entree::Molette { delta: 120 });
        assert_eq!(e.len(), 2);
        assert_eq!(
            (e[0].position_x, e[0].position_y, e[0].bottons),
            (3, 4, 1 | 8)
        );
        assert_eq!(e[1].bottons, 1);
        let e = p.appliquer(&Entree::Molette { delta: -120 });
        assert_eq!(e[0].bottons, 1 | 16);
        assert!(p
            .appliquer(&Entree::Touche {
                keysym: 97,
                enfonce: true
            })
            .is_empty());
    }
}

#[cfg(test)]
mod tests_zone_sale {
    use super::inclusif;
    use crate::trames::ajouter_rect;
    use vnc::Rect;

    fn r(x: u16, y: u16, width: u16, height: u16) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    /// Trouvé par l'audit du 7 septembre 2026 : un serveur (hostile, ou qui
    /// émet des rectangles vides à la marge, ce que certains font) pouvait
    /// envoyer un rectangle Raw de largeur ou de hauteur nulle en x>0 / y>0.
    /// `inclusif` en tirait une zone dégénérée (right = x - 1 < left = x) que
    /// `ajouter_rect` traitait par une soustraction brute `right - left + 1` en
    /// u64 : débordement, panique « attempt to subtract with overflow » en
    /// debug (tests, couverture instrumentée), enroulement en release. La
    /// branche RawImage écarte désormais un rectangle vide avant de peindre
    /// (comme le fait déjà `copier`), et le calcul d'aire sature comme la boucle
    /// RECTS_MAX de `ajouter_rect` le fait déjà.
    #[test]
    fn un_rectangle_raw_de_largeur_nulle_ne_panique_pas() {
        // Une zone déjà peuplée : c'est la comparaison de coûts qui appelle
        // `aire` sur le rectangle ajouté, donc le vide doit y arriver.
        let mut zone = vec![inclusif(r(0, 0, 3, 3))];
        // Rectangle Raw 0×3 en (2,0) : inclusif -> {left:2, right:1}, dégénéré.
        ajouter_rect(&mut zone, &inclusif(r(2, 0, 0, 3)));
        // Hauteur nulle en (0,4) : inclusif -> {top:4, bottom:3}, dégénéré.
        ajouter_rect(&mut zone, &inclusif(r(0, 4, 5, 0)));
        // Aucune panique : une zone dégénérée compte pour une aire nulle.
    }
}

#[cfg(test)]
mod tests_copie {
    use super::copier;
    use ironrdp::graphics::image_processing::PixelFormat;
    use ironrdp::session::image::DecodedImage;
    use vnc::Rect;

    fn image_4x4() -> DecodedImage {
        let mut i = DecodedImage::new(PixelFormat::RgbA32, 4, 4);
        let mut px = Vec::new();
        for n in 0..16u8 {
            px.extend_from_slice(&[n, n, n, 255]);
        }
        i.peindre_rgba(0, 0, 4, 4, &px);
        i
    }
    fn pixel(i: &DecodedImage, x: usize, y: usize) -> u8 {
        i.data()[(y * 4 + x) * 4]
    }
    fn r(x: u16, y: u16, width: u16, height: u16) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn copier_deplace_un_bloc_meme_en_chevauchement() {
        let mut i = image_4x4();
        // Le bloc 2×2 en (0,0) [0,1,4,5] va en (1,1), qui le chevauche.
        assert!(copier(&mut i, r(1, 1, 2, 2), r(0, 0, 2, 2)));
        assert_eq!(pixel(&i, 1, 1), 0);
        assert_eq!(pixel(&i, 2, 1), 1);
        assert_eq!(pixel(&i, 1, 2), 4);
        assert_eq!(pixel(&i, 2, 2), 5);
        // Hors du bloc, rien n'a bougé.
        assert_eq!(pixel(&i, 0, 0), 0);
        assert_eq!(pixel(&i, 3, 3), 15);
    }

    /// Le serveur est une entrée non fiable : un rectangle qui déborde est
    /// ignoré plutôt que de faire paniquer l'indexation.
    #[test]
    fn un_rectangle_qui_deborde_est_ignore() {
        let mut i = image_4x4();
        assert!(!copier(&mut i, r(3, 3, 2, 2), r(0, 0, 2, 2)));
        assert!(!copier(&mut i, r(0, 0, 2, 2), r(3, 0, 2, 2)));
        assert!(!copier(&mut i, r(0, 0, 0, 2), r(0, 0, 0, 2)));
        assert!(!copier(&mut i, r(0, 0, 5, 1), r(0, 0, 5, 1)));
        assert_eq!(pixel(&i, 3, 3), 15);
    }
}

#[cfg(test)]
mod tests_presse_papiers {
    use super::{message_texte_distant, presse_papiers_vers_serveur};

    /// Trouvé par l'audit du 12 septembre 2026 (C-sidecar-5) : le texte
    /// `ServerCutText` n'était borné que par le tampon générique des pixels
    /// (256 Mio) puis relayé tel quel. Un serveur VNC envoyait 200 Mio, le
    /// processus en faisait 400 en passant du Latin-1 à l'UTF-8, et poussait un
    /// message de 400 Mio à la webview : c'est l'application entière qui gelait.
    /// Même plafond qu'en RDP (8 Mio), tenu avant l'envoi.
    #[test]
    fn un_texte_vnc_demesure_n_est_pas_pousse_au_front() {
        let limite = crate::presse_papiers::TEXTE_VERS_INTERFACE_MAX;
        let m = message_texte_distant(&"a".repeat(limite), true).expect("au plafond, il passe");
        assert_eq!((m[0], m.len()), (8, limite + 1));
        assert!(
            message_texte_distant(&"a".repeat(limite + 1), true).is_none(),
            "au-delà du plafond, rien ne part vers l'interface"
        );
        assert!(
            message_texte_distant("court", false).is_none(),
            "partage coupé"
        );
    }

    /// Régression trouvée par l'audit du 7 septembre 2026 : sur l'annonce [8]
    /// (poussée par l'interface à l'ouverture, au focus et au changement
    /// d'onglet), le chemin VNC envoyait aussitôt le presse-papiers du poste au
    /// serveur (`ClientCutText`) — fuite du contenu, souvent un mot de passe
    /// fraîchement copié, sans aucun geste de collage. En RFB il n'y a pas de
    /// phase de demande comme en RDP : le texte part vraiment sur le fil.
    /// L'annonce ne fait plus que mémoriser ; le texte ne part qu'au collage
    /// explicite [22], et seulement si le partage est actif.
    #[test]
    fn le_presse_papiers_ne_part_qu_au_collage_explicite() {
        let mut memoire = None;
        let mut annonce = vec![8u8];
        annonce.extend_from_slice("hunter2".as_bytes());
        // L'annonce mémorise mais n'envoie rien au serveur.
        assert_eq!(
            presse_papiers_vers_serveur(&mut memoire, &annonce, true),
            None
        );
        assert_eq!(memoire.as_deref(), Some("hunter2"));
        // Répétée (chaque focus, chaque bascule d'onglet), elle ne fuit rien.
        assert_eq!(
            presse_papiers_vers_serveur(&mut memoire, &annonce, true),
            None
        );
        // Le collage explicite [22] pousse enfin le texte mémorisé.
        assert_eq!(
            presse_papiers_vers_serveur(&mut memoire, &[22], true),
            Some("hunter2".to_owned())
        );
        // Partage coupé (Ctrl+K) : même un collage ne pousse rien.
        assert_eq!(
            presse_papiers_vers_serveur(&mut memoire, &[22], false),
            None
        );
    }
}
