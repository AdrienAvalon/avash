//! Capture d'écran (`--shot`) : une trame, un PNG, et l'on sort.

use crate::session::{annonce_egfx, Suite};
use crate::{egfx, magnetoscope};
use anyhow::{Context, Result};
use ironrdp::session::image::DecodedImage;
use ironrdp::session::{ActiveStage, ActiveStageOutput};
use ironrdp_tokio::FramedWrite as _;
use std::time::Duration;
use tokio::net::TcpStream;

/// Ce qu'il faut savoir du canal graphique pendant une session.
pub(crate) struct Graphique<'a> {
    pub(crate) canal: &'a egfx::CanalPartage,
    pub(crate) file: &'a egfx::FilePartagee,
    /// Une image, une seule, a-t-elle été affichée ? C'est ce qui décide s'il
    /// faut reprendre la connexion en accordant le canal.
    pub(crate) dessine: &'a std::sync::atomic::AtomicBool,
}

pub(crate) async fn run_shot(
    active: &mut ActiveStage,
    image: &mut DecodedImage,
    framed: &mut ironrdp_tokio::TokioFramed<ironrdp_tls::TlsStream<TcpStream>>,
    path: &str,
    mut magneto: Option<&mut magnetoscope::Enregistreur>,
    g: &Graphique<'_>,
) -> Result<Suite> {
    // Deux fenêtres, selon ce que le serveur donne. Un serveur qui a commencé à
    // dessiner a tout dit en cinq secondes ; un serveur muet est peut-être un
    // pipeline graphique, qu'il faut laisser venir.
    let debut = tokio::time::Instant::now();
    // Lecture par courtes attentes plutôt qu'en un seul long blocage : un
    // serveur Windows qui attend le canal graphique n'envoie plus rien du tout,
    // et l'annonce de capacités — émise dans le corps de cette boucle — ne
    // partait jamais. Le silence est précisément le moment où il faut parler.
    loop {
        let limite = debut
            + if g.dessine.load(std::sync::atomic::Ordering::Relaxed) {
                Duration::from_secs(5)
            } else {
                Duration::from_secs(12)
            };
        if tokio::time::Instant::now() >= limite {
            break;
        }
        let lu = tokio::time::timeout(Duration::from_millis(200), framed.read_pdu()).await;
        let (action, payload) = match lu {
            Ok(Ok(v)) => v,
            Ok(Err(_)) => break,
            Err(_) => {
                // Rien n'est venu : on repasse quand même par l'entretien du
                // canal graphique ci-dessous, puis on attend de nouveau.
                if let Some((id, pdu)) = annonce_egfx(active, g)? {
                    framed.write_all(&pdu).await.context("annonce egfx")?;
                    let _ = id;
                }
                continue;
            }
        };
        if let Some(m) = magneto.as_mut() {
            m.ajouter(action, &payload);
        }
        let mut done = false;
        if let Some((_, pdu)) = annonce_egfx(active, g)? {
            framed.write_all(&pdu).await.context("annonce egfx")?;
        }
        for t in std::mem::take(&mut *g.file.lock().unwrap()).trames {
            g.dessine.store(true, std::sync::atomic::Ordering::Relaxed);
            image.peindre_rgba(t.x, t.y, t.largeur, t.hauteur, &t.pixels);
        }
        for o in active.process(image, action, &payload)? {
            match o {
                ActiveStageOutput::ResponseFrame(f) => framed.write_all(&f).await?,
                ActiveStageOutput::GraphicsUpdate(_) => {
                    g.dessine.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                ActiveStageOutput::Terminate(_) => done = true,
                // Même chemin que la session interactive : suivre la redirection
                // plutôt que de s'arrêter dessus.
                ActiveStageOutput::Redirection(r) => return Ok(Suite::Rediriger(r)),
                _ => {}
            }
        }
        if done {
            break;
        }
    }
    let buf: image::ImageBuffer<image::Rgba<u8>, _> = image::ImageBuffer::from_raw(
        u32::from(image.width()),
        u32::from(image.height()),
        image.data().to_vec(),
    )
    .context("image invalide")?;
    buf.save(path)?;
    eprintln!("capture : {path}");
    Ok(Suite::Fini)
}
