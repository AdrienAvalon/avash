//! Serveur VNC de TEST pour Avash : un vrai serveur RFB (rustvncserver, ZRLE
//! compris) qui sert une image connue et RÉAGIT aux entrées, pour que la suite
//! bout en bout vérifie tout le chemin — poignée de main, mot de passe,
//! décodage, peinture, clavier, souris, presse-papiers — sur des pixels
//! qu'elle peut mesurer, sans machine de plus.
//!
//! Le bureau : moitié gauche rouge, moitié droite bleue. Puis :
//! - la touche « g » (keysym 0x67) repeint tout en vert ;
//! - un clic gauche pose un carré magenta de 40 pixels à l'endroit du clic ;
//! - un texte collé par le client revient sur son presse-papiers, précédé
//!   de « reçu: ».
//!
//! Chaque entrée est aussi écrite sur la sortie standard, une ligne par
//! événement, pour que le scénario lise ce que le serveur a compris (le
//! keysym d'un « é », par exemple).
//!
//! Il écoute sur toutes les interfaces (la bibliothèque ne propose pas
//! d'adresse) : à ne lancer que le temps d'un test, avec un mot de passe qui
//! ne vaut rien.
//!
//! Usage : test-vnc-server [--port 35900] [--pass test] [--width 640] [--height 480]
//!         [--tls-port 35902 --cert cert.pem --key key.pem]   (VeNCrypt, voir vencrypt.rs)

mod vencrypt;

use anyhow::Context as _;
use rustvncserver::server::{ServerEvent, VncServer};
use std::sync::Arc;

const COTE_CARRE: u16 = 40;

struct Options {
    port: u16,
    pass: String,
    largeur: u16,
    hauteur: u16,
    /// Un second port, VeNCrypt (TLS), relié au premier.
    tls_port: Option<u16>,
    cert: Option<std::path::PathBuf>,
    cle: Option<std::path::PathBuf>,
}

fn options() -> anyhow::Result<Options> {
    let mut a = pico_args::Arguments::from_env();
    Ok(Options {
        port: a.opt_value_from_str("--port")?.unwrap_or(35900),
        pass: a
            .opt_value_from_str("--pass")?
            .unwrap_or_else(|| "test".to_owned()),
        largeur: a.opt_value_from_str("--width")?.unwrap_or(640),
        hauteur: a.opt_value_from_str("--height")?.unwrap_or(480),
        tls_port: a.opt_value_from_str("--tls-port")?,
        cert: a.opt_value_from_str("--cert")?,
        cle: a.opt_value_from_str("--key")?,
    })
}

/// Le bureau de départ : rouge à gauche, bleu à droite, en RGBA.
fn bureau(largeur: u16, hauteur: u16) -> Vec<u8> {
    let (l, h) = (usize::from(largeur), usize::from(hauteur));
    let mut px = Vec::with_capacity(l * h * 4);
    for _ in 0..h {
        for x in 0..l {
            if x < l / 2 {
                px.extend_from_slice(&[255, 0, 0, 255]);
            } else {
                px.extend_from_slice(&[0, 0, 255, 255]);
            }
        }
    }
    px
}

/// Un carré uni de `cote` pixels, borné au cadre, en (x, y).
fn carre(x: u16, y: u16, cote: u16, largeur: u16, hauteur: u16) -> (u16, u16, u16, u16) {
    let x = x.min(largeur.saturating_sub(1));
    let y = y.min(hauteur.saturating_sub(1));
    let w = cote.min(largeur - x);
    let h = cote.min(hauteur - y);
    (x, y, w, h)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let o = options()?;
    let (serveur, mut evenements) = VncServer::new(
        o.largeur,
        o.hauteur,
        "avash-test".to_owned(),
        Some(o.pass.clone()),
    );
    let serveur = Arc::new(serveur);
    serveur
        .framebuffer()
        .update_from_slice(&bureau(o.largeur, o.hauteur))
        .await
        .map_err(|e| anyhow::anyhow!(e))
        .context("bureau initial")?;

    let reacteur = serveur.clone();
    let (largeur, hauteur) = (o.largeur, o.hauteur);
    tokio::spawn(async move {
        while let Some(ev) = evenements.recv().await {
            match ev {
                ServerEvent::ClientConnected { client_id } => {
                    println!("client {client_id} connecté")
                }
                ServerEvent::ClientDisconnected { client_id } => {
                    println!("client {client_id} parti");
                }
                ServerEvent::KeyPress { key, down, .. } => {
                    println!(
                        "touche 0x{key:x} {}",
                        if down { "enfoncée" } else { "relâchée" }
                    );
                    if down && key == 0x67 {
                        let vert: Vec<u8> =
                            [0, 255, 0, 255].repeat(usize::from(largeur) * usize::from(hauteur));
                        let _ = reacteur.framebuffer().update_from_slice(&vert).await;
                    }
                }
                ServerEvent::PointerMove {
                    x, y, button_mask, ..
                } => {
                    println!("pointeur en {x},{y} masque {button_mask}");
                    if button_mask & 1 != 0 {
                        println!("clic gauche en {x},{y}");
                        let (cx, cy, w, h) = carre(x, y, COTE_CARRE, largeur, hauteur);
                        let magenta: Vec<u8> =
                            [255, 0, 255, 255].repeat(usize::from(w) * usize::from(h));
                        let _ = reacteur
                            .framebuffer()
                            .update_cropped(&magenta, cx, cy, w, h)
                            .await;
                    }
                }
                ServerEvent::CutText { text, .. } => {
                    println!("presse-papiers reçu : {text}");
                    // Dans une tâche à part : `send_cut_text_to_all` attend le
                    // verrou d'écriture de chaque client, que la tâche du
                    // client garde pendant qu'elle lit ses messages. Appelé
                    // ici, il bloquait cette boucle et plus aucune entrée
                    // n'était traitée (clic sans carré magenta, trois passages
                    // sur cinq du scénario bout en bout, 2026-09-04).
                    let serveur = reacteur.clone();
                    tokio::spawn(async move {
                        let _ = serveur.send_cut_text_to_all(format!("reçu:{text}")).await;
                    });
                }
                _ => {}
            }
        }
    });

    println!(
        "test-vnc-server : {}x{} sur le port {}",
        o.largeur, o.hauteur, o.port
    );
    if let Some(port_tls) = o.tls_port {
        let (cert, cle) = (
            o.cert.clone().context("--cert requis avec --tls-port")?,
            o.cle.clone().context("--key requis avec --tls-port")?,
        );
        let port_interne = o.port;
        tokio::spawn(async move {
            if let Err(e) = vencrypt::ecouter(port_tls, port_interne, &cert, &cle).await {
                println!("vencrypt : arrêt : {e:#}");
            }
        });
    }
    serveur.listen(o.port).await.context("écoute VNC")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{bureau, carre};

    #[test]
    fn le_bureau_est_rouge_a_gauche_et_bleu_a_droite() {
        let px = bureau(4, 2);
        assert_eq!(px.len(), 4 * 2 * 4);
        assert_eq!(&px[0..4], &[255, 0, 0, 255]);
        assert_eq!(&px[2 * 4..3 * 4], &[0, 0, 255, 255]);
        assert_eq!(&px[(4 + 3) * 4..(4 + 4) * 4], &[0, 0, 255, 255]);
    }

    /// Un clic au bord ne fait pas déborder le carré du cadre.
    #[test]
    fn le_carre_reste_dans_le_cadre() {
        assert_eq!(carre(0, 0, 40, 640, 480), (0, 0, 40, 40));
        assert_eq!(carre(630, 470, 40, 640, 480), (630, 470, 10, 10));
        assert_eq!(carre(1000, 1000, 40, 640, 480), (639, 479, 1, 1));
    }
}
