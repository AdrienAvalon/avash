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
//! Aucune empreinte à épingler : le RFB classique ne chiffre rien et ne
//! présente rien à vérifier. `SECURITY.md` le dit, et recommande un tunnel
//! SSH pour tout ce qui sort du réseau local.

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
        // Nos propres messages (VeNCrypt, certificat épinglé) : tels quels,
        // sans le « VNC Error with message » que la bibliothèque colle devant.
        vnc::VncError::General(m) => anyhow::anyhow!("{m}"),
        autre => anyhow::anyhow!("{autre}"),
    }
}

pub(crate) async fn executer(args: &Args) -> Result<()> {
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
    let client = tokio::time::timeout(DELAI_CONNEXION, async move {
        VncConnector::new(crate::vnc_tls::MaybeTls::Clair(tcp))
            .set_tls_upgrader(monteur)
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
    etablir_poste(&mut poste).await?;
    let Poste { sink, stream, .. } = poste.as_mut().expect("poste établi juste au-dessus");
    let mut hello = vec![1u8];
    hello.extend_from_slice(&w.to_le_bytes());
    hello.extend_from_slice(&h.to_le_bytes());
    sink.send(Message::Binary(hello.into())).await?;

    let mut pointeur = Pointeur::default();
    let mut partage_clip = true;
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
                    Some(Ok(Message::Binary(b))) if b.first() == Some(&8) => {
                        if partage_clip {
                            if let Ok(text) = std::str::from_utf8(&b[1..]) {
                                client.input(X11Event::CopyText(text.to_owned())).await.map_err(message)?;
                            }
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
                            if partage_clip {
                                let mut m = vec![8u8];
                                m.extend_from_slice(texte.as_bytes());
                                sink.send(Message::Binary(m.into())).await.context("envoi presse-papiers")?;
                            }
                        }
                        VncEvent::Error(m) => anyhow::bail!("{m}"),
                        // Curseur et JPEG ne sont pas demandés ; cloche,
                        // format de pixel : sans effet.
                        _ => {}
                    }
                }
                if image_recue {
                    demande_en_vol = false;
                    flush_dirty!();
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
