//! Un serveur VNC hostile, du premier octet à la dernière image : la version,
//! la liste des types de sécurité, le défi, l'initialisation (taille, format
//! de pixel, nom), puis des mises à jour (Raw, ZRLE, CopyRect, taille de
//! bureau, texte du presse-papiers). Tout cela est lu par le client `vnc-rs`
//! porté dans `rdp-sidecar/vendor/`, dont les bornes (`tampon`, rectangle dans
//! le cadre, résolution) sont nos ajouts : c'est ce qu'on veut voir tenir.
//!
//! Le serveur est scénarisé dans un tampon (`tokio::io::duplex`) : le client
//! écrit dans le vide et lit ce que le fuzzer a produit. Le côté serveur est
//! fermé aussitôt, ce qui termine les lectures « jusqu'à la fin du flux ».
#![no_main]

use libfuzzer_sys::fuzz_target;
use std::sync::OnceLock;
use tokio::io::AsyncWriteExt as _;
use vnc::{PixelFormat, VncConnector, VncEncoding, VncEvent};

fn moteur() -> &'static tokio::runtime::Runtime {
    static M: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    M.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("moteur tokio")
    })
}

fuzz_target!(|data: &[u8]| {
    let script = data.to_vec();
    moteur().block_on(async move {
        let (client, mut serveur) = tokio::io::duplex(1 << 20);
        // Le script tient dans le tampon (max_len de fuzz.sh) : l'écriture
        // n'attend personne, puis le serveur raccroche.
        let _ = serveur.write_all(&script).await;
        drop(serveur);
        let connexion = VncConnector::new(client)
            .set_auth_method(async { Ok("secret".to_owned()) })
            .add_encoding(VncEncoding::Zrle)
            .add_encoding(VncEncoding::CopyRect)
            .add_encoding(VncEncoding::Raw)
            .add_encoding(VncEncoding::DesktopSizePseudo)
            .set_pixel_format(PixelFormat::rgba())
            .build()
            .expect("un codage est déclaré");
        let delai = std::time::Duration::from_secs(2);
        let Ok(Ok(etat)) = tokio::time::timeout(delai, connexion.try_start()).await else {
            return;
        };
        let Ok(vnc) = etat.finish() else {
            return;
        };
        let Some(mut evenements) = vnc.take_events().await else {
            return;
        };
        // Tout ce que le serveur a envoyé, jusqu'à la fin du flux ou une
        // erreur ; une image acceptée a exactement ses pixels.
        let _ = tokio::time::timeout(delai, async {
            while let Some(e) = evenements.recv().await {
                match e {
                    VncEvent::RawImage(rect, pixels) => {
                        assert_eq!(
                            pixels.len(),
                            usize::from(rect.width) * usize::from(rect.height) * 4,
                            "image acceptée de la mauvaise taille"
                        );
                    }
                    VncEvent::Error(_) => break,
                    _ => {}
                }
            }
        })
        .await;
        let _ = vnc.close().await;
    });
});
