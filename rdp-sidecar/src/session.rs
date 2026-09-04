//! La session établie : boucle d'événements, cadencement des trames, redimensionnement, presse-papiers, statistiques.

use crate::acces_local::{etablir_poste, Poste};
use crate::args::{taille_sure, Args};
use crate::capture::{run_shot, Graphique};
use crate::connexion::{connect, session_close_par_le_serveur, NLA_INDISPONIBLE};
use crate::entrees::{input_ops, lock_sync_event};
use crate::presse_papiers::{ClipBackend, ClipReq, LocalClip};
use crate::trames::{ajouter_rect, frame_msg, frames_msg, nouvelle_taille};
use crate::{egfx, magnetoscope};
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use ironrdp::cliprdr::pdu::{ClipboardFormat, ClipboardFormatId};
use ironrdp::cliprdr::CliprdrClient;
use ironrdp::connector::connection_activation::ConnectionActivationState;
use ironrdp::core::WriteBuf;
use ironrdp::displaycontrol::pdu::MonitorLayoutEntry;
use ironrdp::dvc::DrdynvcClient;
use ironrdp::graphics::image_processing::PixelFormat;
use ironrdp::input::Database;
use ironrdp::pdu::geometry::InclusiveRectangle;
use ironrdp::session::image::DecodedImage;
use ironrdp::session::{ActiveStage, ActiveStageBuilder, ActiveStageOutput};
use ironrdp_tokio::single_sequence_step;
use ironrdp_tokio::FramedWrite as _;
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Message;

/// Filet anti-gel : si un ACK de rendu se perd, on renvoie l'état courant
/// passé ce délai plutôt que de figer l'affichage.
const ACK_TIMEOUT: Duration = Duration::from_millis(250);

pub(crate) enum Suite {
    /// Rien n'a été dessiné : ce serveur n'a probablement que le canal
    /// graphique, qu'il faut lui offrir — donc se reconnecter.
    ReprendreAvecGraphique,
    /// La session s'est terminée normalement.
    Fini,
    /// Le serveur nous renvoie ailleurs ; il faut tout refaire avec ce qu'il donne.
    Rediriger(Box<ironrdp::session::redirection::Redirection>),
}

pub(crate) async fn executer(
    args: &Args,
    redirection: Option<Box<ironrdp::session::redirection::Redirection>>,
    poste: &mut Option<Poste>,
    graphique: egfx::Politique,
    dessine: &std::sync::atomic::AtomicBool,
) -> Result<Suite> {
    let local_text: LocalClip = std::sync::Arc::new(std::sync::Mutex::new(None));
    let (clip_tx, mut clip_rx) = tokio::sync::mpsc::unbounded_channel::<ClipReq>();
    // Actif par défaut — parité avec les autres clients RDP ; l'interface
    // annonce aussitôt le réglage retenu par l'utilisateur (message [12]).
    let partage_clip = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let clip_backend = ClipBackend {
        partage: partage_clip.clone(),
        local_text: local_text.clone(),
        tx: clip_tx.clone(),
    };
    // Une tentative de connexion doit être BORNÉE. Sans cela le processus reste
    // pendu indéfiniment, sans un mot : constaté contre un xrdp qui annonce NLA
    // mais ne mène jamais l'échange CredSSP à son terme — TLS est monté, les
    // données circulent, et rien n'aboutit. L'utilisateur voit un onglet figé.
    const DELAI_CONNEXION: Duration = Duration::from_secs(25);
    let (result, mut framed, canal_egfx, file_egfx) = match tokio::time::timeout(
        DELAI_CONNEXION,
        connect(args, clip_backend, redirection.as_deref(), graphique),
    )
    .await
    {
        Ok(r) => r?,
        Err(_) if !args.sans_nla => anyhow::bail!(
            "{NLA_INDISPONIBLE} L'authentification réseau (NLA) n'a pas abouti \
             en {} s. Certains serveurs annoncent NLA sans savoir le mener à \
             bien — c'est le cas de plusieurs versions de xrdp.",
            DELAI_CONNEXION.as_secs()
        ),
        Err(_) => anyhow::bail!(
            "Le serveur n'a pas répondu en {} s.",
            DELAI_CONNEXION.as_secs()
        ),
    };
    let (w, h) = taille_sure(result.desktop_size.width, result.desktop_size.height)?;
    let activation_factory = result.activation_factory;
    eprintln!("connecté : {w}x{h}");
    let mut image = DecodedImage::new(PixelFormat::RgbA32, w, h);
    let (io_channel_id, user_channel_id) = (result.io_channel_id, result.user_channel_id);
    let (message_channel_id, share_id) = (result.message_channel_id, result.share_id);
    let canal_dvc = result
        .static_channels
        .get_channel_id_by_type::<DrdynvcClient>();
    let canal_clip = result
        .static_channels
        .get_channel_id_by_type::<CliprdrClient>();
    if std::env::var_os("AVASH_EGFX_TRACE").is_some() {
        eprintln!("canaux statiques : dvc={canal_dvc:?} clip={canal_clip:?} io={io_channel_id} user={user_channel_id}");
    }
    let mut active = ActiveStageBuilder {
        static_channels: result.static_channels,
        user_channel_id: result.user_channel_id,
        io_channel_id: result.io_channel_id,
        message_channel_id: result.message_channel_id,
        share_id: result.share_id,
        compression_type: result.compression_type,
        enable_server_pointer: result.enable_server_pointer,
        pointer_software_rendering: result.pointer_software_rendering,
    }
    .build();

    // Magnétoscope : capture le dialogue du serveur pour le rejouer plus tard,
    // sans réseau. Voir magnetoscope.rs — c'est ce qui transforme une machine du
    // parc en fixture permanente.
    let mut magneto = match args.enregistrer.as_deref() {
        Some(chemin) => {
            let entete = magnetoscope::Entete {
                largeur: w,
                hauteur: h,
                io: io_channel_id,
                utilisateur: user_channel_id,
                message: message_channel_id,
                partage: share_id,
                compression: 0,
                canal_dvc: canal_dvc.unwrap_or(0),
                canal_clip: canal_clip.unwrap_or(0),
            };
            let e =
                magnetoscope::Enregistreur::nouveau(chemin, &entete, args.plafond_enregistrement)?;
            eprintln!(
                "enregistrement : {chemin} (plafond {} octets)",
                args.plafond_enregistrement
            );
            Some(e)
        }
        None => None,
    };

    if let Some(path) = args.shot.clone() {
        return run_shot(
            &mut active,
            &mut image,
            &mut framed,
            &path,
            magneto.as_mut(),
            &Graphique {
                canal: &canal_egfx,
                file: &file_egfx,
                dessine,
            },
        )
        .await;
    }

    // Serveur WebSocket local : un seul client (Avash), jeton obligatoire.
    // Établi au premier passage seulement : une redirection de serveur rappelle
    // cette fonction, et rouvrir un port neuf laisserait l'interface parler dans
    // le vide, attachée à l'ancien.
    etablir_poste(poste).await?;
    let Poste { sink, stream, .. } = poste.as_mut().expect("poste établi juste au-dessus");

    // CONNECTED [1][w][h]
    let mut hello = vec![1u8];
    hello.extend_from_slice(&w.to_le_bytes());
    hello.extend_from_slice(&h.to_le_bytes());
    sink.send(Message::Binary(hello.into())).await?;

    let mut db = Database::new();

    // --- Cadencement adaptatif sur ACK (anti-lag) ---------------------------
    // Au plus une trame « en vol ». Les mises à jour qui arrivent pendant le
    // rendu de la webview sont fusionnées (union des rectangles) ; à l'ACK, on
    // envoie l'état le plus récent. Rapide → chaque trame part aussitôt (aucune
    // latence ajoutée) ; lent → on fusionne, jamais de file qui s'accumule.
    let mut dirty: Vec<InclusiveRectangle> = Vec::new();
    let mut awaiting_ack = false;
    // Onglet masqué : le canvas n'est pas à l'écran, mais l'accusé de rendu
    // partait quand même — le serveur voyait la voie libre en permanence et le
    // sidecar continuait à décoder et à pousser des trames pleines (8 Mo en
    // 1080p) que personne ne regardait. En pause, on accumule le rectangle sale
    // sans rien émettre ; le retour au premier plan demande un REFRESH.
    let mut en_pause = false;
    // Le serveur dessine-t-il par le chemin classique ? Voir ATTENTE_EGFX.
    let mut dessin_classique = false;
    let mut silence: Option<Instant> = None;
    let mut last_send = Instant::now();
    // Métriques (fenêtre ~1 s) : fps, débit, latence de bout en bout.
    let (mut stat_frames, mut stat_bytes): (u32, u64) = (0, 0);
    let mut lat_ms: f32 = 0.0;
    let mut stat_window = Instant::now();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    // Envoie la zone accumulée si la voie est libre. `sink`/`image`/compteurs
    // viennent du scope englobant (macro non hygiénique volontairement).
    #[allow(clippy::items_after_statements)]
    macro_rules! flush_dirty {
        () => {
            if !awaiting_ack && !en_pause {
                if !dirty.is_empty() {
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
            }
        };
    }

    // Récolte les trames décodées par le canal graphique et les peint dans
    // l'image. EGFX décode dans ses propres surfaces : sans ce report, la
    // session est parfaitement fonctionnelle et l'écran reste noir.
    #[allow(clippy::items_after_statements)]
    macro_rules! peindre_egfx {
        () => {{
            let sortie = std::mem::take(&mut *file_egfx.lock().unwrap());
            if let Some((nl, nh)) = sortie.taille {
                if nouvelle_taille(&mut image, &mut dirty, &mut awaiting_ack, nl, nh) {
                    let mut hello = vec![1u8];
                    hello.extend_from_slice(&nl.to_le_bytes());
                    hello.extend_from_slice(&nh.to_le_bytes());
                    sink.send(Message::Binary(hello.into()))
                        .await
                        .context("nouvelle taille")?;
                }
            }
            if !sortie.trames.is_empty() {
                dessine.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            for t in sortie.trames {
                image.peindre_rgba(t.x, t.y, t.largeur, t.hauteur, &t.pixels);
                ajouter_rect(
                    &mut dirty,
                    &InclusiveRectangle {
                        left: t.x,
                        top: t.y,
                        right: t.x.saturating_add(t.largeur).saturating_sub(1),
                        bottom: t.y.saturating_add(t.hauteur).saturating_sub(1),
                    },
                );
            }
            flush_dirty!();
        }};
    }

    // Envoie au serveur les messages d'un canal statique (ici CLIPRDR).
    #[allow(clippy::items_after_statements)]
    macro_rules! send_svc {
        ($msgs:expr) => {{
            let bytes = active
                .process_svc_processor_messages($msgs)
                .context("encodage SVC")?;
            framed.write_all(&bytes).await.context("écriture SVC")?;
        }};
    }

    loop {
        tokio::select! {
            biased;
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Binary(b))) if b.first() == Some(&5) && b.len() >= 5 => {
                        // RESIZE : demande au serveur de re-rendre à la nouvelle
                        // taille via Display Control. Sans effet tant que le canal
                        // n'a pas reçu ses capacités (encode_resize renvoie None).
                        let rw = u16::from_le_bytes([b[1], b[2]]);
                        let rh = u16::from_le_bytes([b[3], b[4]]);
                        let (aw, ah) = MonitorLayoutEntry::adjust_display_size(u32::from(rw), u32::from(rh));
                        if let Some(res) = active.encode_resize(aw, ah, None, None) {
                            let bytes = res.context("encodage resize")?;
                            framed.write_all(&bytes).await.context("écriture resize")?;
                        }
                    }
                    Some(Ok(Message::Binary(b))) if b.first() == Some(&6) => {
                        // ACK de rendu : RTT de bout en bout, lissé.
                        let rtt = last_send.elapsed().as_secs_f32() * 1000.0;
                        lat_ms = if lat_ms == 0.0 { rtt } else { lat_ms.mul_add(0.8, rtt * 0.2) };
                        awaiting_ack = false;
                        flush_dirty!();
                    }
                    Some(Ok(Message::Binary(b))) if b.first() == Some(&8) => {
                        // Presse-papiers du poste : on mémorise le texte et on
                        // l'annonce au serveur (collage possible dans le distant).
                        if partage_clip.load(std::sync::atomic::Ordering::Relaxed) {
                            if let Ok(text) = std::str::from_utf8(&b[1..]) {
                                if let Ok(mut g) = local_text.lock() {
                                    *g = Some(text.to_owned());
                                }
                                let _ = clip_tx.send(ClipReq::Advertise);
                            }
                        }
                    }
                    Some(Ok(Message::Binary(b))) if b.first() == Some(&12) && b.len() >= 2 => {
                        // CLIPBOARD : 0 = l'utilisateur a coupé le partage.
                        partage_clip.store(b[1] != 0, std::sync::atomic::Ordering::Relaxed);
                    }
                    Some(Ok(Message::Binary(b))) if b.first() == Some(&11) && b.len() >= 2 => {
                        // PAUSE : 1 = l'onglet est passé à l'arrière-plan.
                        en_pause = b[1] != 0;
                        if !en_pause {
                            flush_dirty!();
                        }
                    }
                    Some(Ok(Message::Binary(b))) if b.first() == Some(&9) => {
                        // REFRESH : l'onglet a été réaffiché et son canvas peut être
                        // vide ; on renvoie l'image entière, hors cadencement.
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
                        // Un REFRESH ne s'obtient qu'en revenant au premier plan :
                        // il lève la pause. Le flux ne peut donc pas rester gelé
                        // si l'interface oubliait de la lever explicitement.
                        en_pause = false;
                    }
                    Some(Ok(Message::Binary(b))) => {
                        // [10] ne décrit pas une frappe mais l'état des verrous :
                        // il produit son événement directement, sans passer par
                        // la base d'état des touches.
                        let events: Vec<_> = if b.first() == Some(&10) && b.len() >= 2 {
                            vec![lock_sync_event(b[1])]
                        } else {
                            db.apply(input_ops(&b)).into_vec()
                        };
                        for o in active.process_fastpath_input(&mut image, &events)? {
                            if let ActiveStageOutput::ResponseFrame(f) = o {
                                framed.write_all(&f).await.context("écriture entrée")?;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_)) | Err(_)) | None => break, // fin/erreur
                    Some(Ok(_)) => {}
                }
            }
            _ = tick.tick() => {
                // Annonce des capacités graphiques, une fois le canal ouvert et
                // dans une écriture qui lui est propre (voir `egfx::start`).
                // Deux issues selon ce que le serveur nous a laissé faire. Si
                // le canal graphique est ouvert, il ne reste qu'à annoncer nos
                // capacités — dans une écriture qui lui soit propre. S'il ne
                // l'est pas et que rien n'a été dessiné, c'est que ce serveur
                // n'a que celui-là : il faut reprendre en le lui accordant.
                const ATTENTE_EGFX: Duration = Duration::from_secs(4);
                if !dessin_classique {
                    let depuis = *silence.get_or_insert_with(Instant::now);
                    if egfx::canal_ouvert(&canal_egfx).is_some() {
                        if let Some((id, pdu)) = egfx::annonce_a_emettre(&canal_egfx) {
                            eprintln!("egfx : annonce des capacités sur le canal {id}");
                            let bytes = active
                                .process_svc_processor_messages(egfx::lot_dvc(id, pdu)?)
                                .context("encodage egfx")?;
                            framed.write_all(&bytes).await.context("écriture egfx")?;
                        }
                    } else if graphique == egfx::Politique::Observer
                        && depuis.elapsed() >= ATTENTE_EGFX
                    {
                        return Ok(Suite::ReprendreAvecGraphique);
                    }
                }
                // Un ACK perdu ne doit pas geler l'écran : au-delà du délai, on
                // considère la webview libre et on renvoie l'état courant.
                if awaiting_ack && last_send.elapsed() > ACK_TIMEOUT {
                    awaiting_ack = false;
                }
                flush_dirty!();
                if stat_window.elapsed() >= Duration::from_secs(1) {
                    let secs = stat_window.elapsed().as_secs_f32();
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
                    let fps = (stat_frames as f32 / secs).round() as u16;
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
                    let kbps = ((stat_bytes as f32 / 1024.0) / secs).round() as u32;
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let lat = lat_ms.round() as u16;
                    // STATS [7] fps:u16 kbps:u32 lat_ms:u16
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
            Some(req) = clip_rx.recv() => {
                match req {
                    ClipReq::Advertise => {
                        let msgs = active.get_svc_processor_mut::<CliprdrClient>().and_then(|c| {
                            c.initiate_copy(&[ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT)]).ok()
                        });
                        if let Some(msgs) = msgs {
                            send_svc!(msgs);
                        }
                    }
                    ClipReq::RequestPaste(fmt) => {
                        let msgs = active
                            .get_svc_processor_mut::<CliprdrClient>()
                            .and_then(|c| c.initiate_paste(fmt).ok());
                        if let Some(msgs) = msgs {
                            send_svc!(msgs);
                        }
                    }
                    ClipReq::ServeData(resp) => {
                        let msgs = active
                            .get_svc_processor_mut::<CliprdrClient>()
                            .and_then(|c| c.submit_format_data(resp).ok());
                        if let Some(msgs) = msgs {
                            send_svc!(msgs);
                        }
                    }
                    ClipReq::RemoteText(text) => {
                        // CLIPBOARD [8] vers le front : le distant a copié du texte.
                        let mut m = vec![8u8];
                        m.extend_from_slice(text.as_bytes());
                        sink.send(Message::Binary(m.into())).await.context("envoi presse-papiers")?;
                    }
                }
            }
            read = framed.read_pdu() => {
                let (action, payload) = read.map_err(|e| {
                    // Même événement, même message : une coupure en pleine session
                    // affichait le code brut du système. Voir
                    // session_close_par_le_serveur.
                    let t = format!("{e} {e:?}");
                    if session_close_par_le_serveur(&t) {
                        anyhow::anyhow!(
                            "Le serveur a fermé la connexion. Si cela se produit \
                             juste après l'ouverture, c'est que la session ne \
                             démarre pas de son côté ; son journal le dira."
                        )
                    } else {
                        anyhow::Error::new(e).context("lecture PDU")
                    }
                })?;
                if let Some(m) = magneto.as_mut() {
                    m.ajouter(action, &payload);
                }
                let sorties = active.process(&mut image, action, &payload)?;
                peindre_egfx!();
                for o in sorties {
                    match o {
                        ActiveStageOutput::ResponseFrame(f) => framed.write_all(&f).await.context("écriture réponse")?,
                        ActiveStageOutput::GraphicsUpdate(rect) => {
                            dessin_classique = true;
                            dessine.store(true, std::sync::atomic::Ordering::Relaxed);
                            ajouter_rect(&mut dirty, &rect);
                            flush_dirty!();
                        }
                        ActiveStageOutput::Terminate(_) => return Ok(Suite::Fini),
                        ActiveStageOutput::Redirection(r) => return Ok(Suite::Rediriger(r)),
                        ActiveStageOutput::DeactivateAll => {
                            // Le serveur a accepté le changement de résolution : dérouler
                            // la séquence désactivation/réactivation pour renégocier.
                            let mut buf = WriteBuf::new();
                            let mut seq = activation_factory.create();
                            let size = loop {
                                single_sequence_step(&mut framed, &mut seq, &mut buf)
                                    .await
                                    .context("réactivation")?;
                                if let ConnectionActivationState::Finalized { desktop_size, share_id, .. } =
                                    seq.connection_activation_state()
                                {
                                    active.set_share_id(share_id);
                                    break desktop_size;
                                }
                            };
                            let (nw, nh) = taille_sure(size.width, size.height)?;
                            // Sur un serveur qui dessine par le canal graphique, le
                            // ResetGraphics est déjà passé par là : l'image est à la
                            // bonne taille et porte les premières trames de la
                            // nouvelle surface. La recréer ici les effaçait — voir
                            // `nouvelle_taille`. Sur le chemin classique, la taille
                            // change vraiment et le serveur renvoie un
                            // rafraîchissement complet.
                            if nouvelle_taille(&mut image, &mut dirty, &mut awaiting_ack, nw, nh) {
                                // Annonce la nouvelle taille à Avash (réutilise CONNECTED [1][w][h]).
                                let mut msg = vec![1u8];
                                msg.extend_from_slice(&nw.to_le_bytes());
                                msg.extend_from_slice(&nh.to_le_bytes());
                                sink.send(Message::Binary(msg.into())).await.context("annonce resize")?;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    Ok(Suite::Fini)
}

/// L'annonce de capacités graphiques à écrire, s'il y en a une.
///
/// Rendue prête à l'emploi : le PDU est déjà encadré pour le canal statique.
pub(crate) fn annonce_egfx(
    active: &mut ActiveStage,
    g: &Graphique<'_>,
) -> Result<Option<(u32, Vec<u8>)>> {
    let Some((id, pdu)) = egfx::annonce_a_emettre(g.canal) else {
        return Ok(None);
    };
    let bytes = active
        .process_svc_processor_messages(egfx::lot_dvc(id, pdu)?)
        .context("encodage egfx")?;
    Ok(Some((id, bytes)))
}
