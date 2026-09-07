//! Capture d'écran (`--shot`) : une trame, un PNG, et l'on sort.

use crate::session::{annonce_egfx, Suite};
use crate::{egfx, magnetoscope};
use anyhow::{Context, Result};
use ironrdp::graphics::image_processing::PixelFormat;
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
                // Une trame décodée peut attendre dans la file sans qu'un
                // nouveau PDU vienne la chasser (serveur EGFX statique) : la
                // peindre ici, sinon l'attente expire sur une image noire.
                // (audit du 7 septembre 2026)
                peindre_en_attente(g.file, image, g.dessine);
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
        // Vider la file APRÈS le traitement du PDU courant : le décodage EGFX de
        // ce PDU vient tout juste d'alimenter la file, la peindre maintenant lie
        // chaque trame au PDU qui l'a produite. L'ancienne vidange se faisait
        // avant `active.process` : les trames du PDU k n'atteignaient l'image
        // qu'à l'arrivée du PDU k+1, et celles du dernier PDU étaient perdues.
        // (audit du 7 septembre 2026)
        peindre_en_attente(g.file, image, g.dessine);
        if done {
            break;
        }
    }
    // Dernière vidange avant d'écrire : sur un serveur EGFX statique la seule
    // trame arrive dans le dernier PDU, sans lecture ultérieure pour la chasser.
    // Sans ce passage, le PNG sortait noir. (audit du 7 septembre 2026)
    peindre_en_attente(g.file, image, g.dessine);
    ecrire_png(image, path)?;
    eprintln!("capture : {path}");
    Ok(Suite::Fini)
}

/// Vide la file du canal graphique dans l'image : recrée l'image quand le
/// serveur a changé la taille de la scène (`ResetGraphics`, porté par
/// `sortie.taille`), puis peint les trames en attente. Rend vrai dès qu'une
/// trame a été peinte.
///
/// Trouvé par l'audit du 7 septembre 2026 : `run_shot` vidait la file AVANT
/// `active.process`, si bien que les trames décodées par un PDU n'atteignaient
/// l'image qu'à l'arrivée du PDU suivant, et jamais celles du dernier PDU ni
/// celles reçues pendant la dernière attente ; `sortie.taille` était de plus
/// ignorée ici, contrairement à la boucle de session (`peindre_egfx!`). La
/// recréation suit le rejeu du magnétoscope (même geste, image à la neuve) ;
/// la taille est déjà bornée à la source (`egfx::CMD_RESET_GRAPHICS`).
fn peindre_en_attente(
    file: &egfx::FilePartagee,
    image: &mut DecodedImage,
    dessine: &std::sync::atomic::AtomicBool,
) -> bool {
    let sortie = std::mem::take(&mut *file.lock().unwrap());
    if let Some((nl, nh)) = sortie.taille {
        if (nl, nh) != (image.width(), image.height()) {
            *image = DecodedImage::new(PixelFormat::RgbA32, nl, nh);
        }
    }
    let mut peint = false;
    for t in sortie.trames {
        dessine.store(true, std::sync::atomic::Ordering::Relaxed);
        image.peindre_rgba(t.x, t.y, t.largeur, t.hauteur, &t.pixels);
        peint = true;
    }
    peint
}

/// Écrit une image décodée en PNG. Partagé avec le rejeu du magnétoscope
/// (`--rejouer … --image`), qui produit la même image sans réseau.
pub(crate) fn ecrire_png(image: &DecodedImage, path: &str) -> Result<()> {
    let buf: image::ImageBuffer<image::Rgba<u8>, _> = image::ImageBuffer::from_raw(
        u32::from(image.width()),
        u32::from(image.height()),
        image.data().to_vec(),
    )
    .context("image invalide")?;
    buf.save(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::peindre_en_attente;
    use crate::egfx::{FilePartagee, Sortie, Trame};
    use ironrdp::graphics::image_processing::PixelFormat;
    use ironrdp::session::image::DecodedImage;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Le cas de l'audit du 7 septembre 2026 : sur un serveur EGFX statique
    /// (GNOME Remote Desktop), la seule trame complète arrive dans le dernier
    /// PDU, sans lecture ultérieure pour la chasser. `run_shot` la vidait avant
    /// `active.process` et jamais avant d'écrire le PNG : l'image restait noire.
    /// Ici, l'unique appel qui précède l'écriture doit la peindre.
    #[test]
    fn la_trame_du_dernier_pdu_est_peinte_avant_l_ecriture() {
        let mut image = DecodedImage::new(PixelFormat::RgbA32, 2, 2);
        let file = FilePartagee::default();
        file.lock().unwrap().trames.push(Trame {
            x: 0,
            y: 0,
            largeur: 2,
            hauteur: 2,
            // Rouge opaque sur les quatre pixels.
            pixels: [0xFF, 0x00, 0x00, 0xFF].repeat(4),
        });
        let dessine = AtomicBool::new(false);

        // Sans cet appel (l'ancien code n'en faisait aucun avant `ecrire_png`),
        // l'image serait restée à ses octets initiaux (noirs).
        assert!(peindre_en_attente(&file, &mut image, &dessine));
        assert!(
            dessine.load(Ordering::Relaxed),
            "une trame peinte pose dessine"
        );
        assert!(
            image.data().iter().any(|&o| o != 0),
            "l'image ne doit plus être noire après la vidange finale"
        );
        assert_eq!(image.data()[0], 0xFF, "le rouge de la trame est bien peint");
    }

    /// Un `ResetGraphics` change la taille avant la trame finale : la file porte
    /// `sortie.taille`, que `run_shot` ignorait. L'image doit repartir à la
    /// nouvelle taille (comme `nouvelle_taille`/le rejeu), sinon `peindre_rgba`
    /// rogne la trame et le PNG garde l'ancienne géométrie.
    #[test]
    fn reset_graphics_change_la_taille_avant_la_trame_finale() {
        let mut image = DecodedImage::new(PixelFormat::RgbA32, 2, 2);
        let file = FilePartagee::default();
        {
            let mut s: std::sync::MutexGuard<'_, Sortie> = file.lock().unwrap();
            s.taille = Some((4, 3));
            s.trames.push(Trame {
                x: 3,
                y: 2,
                largeur: 1,
                hauteur: 1,
                pixels: vec![0x10, 0x20, 0x30, 0xFF],
            });
        }
        let dessine = AtomicBool::new(false);

        assert!(peindre_en_attente(&file, &mut image, &dessine));
        assert_eq!(
            (image.width(), image.height()),
            (4, 3),
            "image recréée à la nouvelle taille"
        );
        // Le pixel du coin bas-droit, hors de l'ancienne image 2×2, est peint.
        let i = (2 * 4 + 3) * 4;
        assert_eq!(image.data()[i..i + 4], [0x10, 0x20, 0x30, 0xFF]);
    }

    /// Une file vide ne peint rien et ne pose pas `dessine` : la boucle de
    /// `run_shot` peut ainsi laisser l'attente longue courir tant que rien
    /// n'est venu, sans se croire déjà servie.
    #[test]
    fn une_file_vide_ne_dessine_rien() {
        let mut image = DecodedImage::new(PixelFormat::RgbA32, 2, 2);
        let file = FilePartagee::default();
        let dessine = AtomicBool::new(false);
        assert!(!peindre_en_attente(&file, &mut image, &dessine));
        assert!(!dessine.load(Ordering::Relaxed));
    }
}
