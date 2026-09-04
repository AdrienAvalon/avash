//! Décodage ClearCodec (MS-RDPEGFX 2.2.4.1) : les trois couches (résiduelle,
//! bandes, sous-codecs), les sous-codecs brut, NSCodec et RLEX, les caches de
//! glyphes et de barres verticales. C'est ce que Windows envoie par le canal
//! graphique, et le sous-codec NSCodec comme le RLEX à une couleur sont nos
//! portages (`rdp-sidecar/vendor/README.md`) : tout ce qui y lit une taille de
//! plan, une longueur de série ou un indice de palette vient du serveur.
#![no_main]

use ironrdp_graphics::clearcodec::ClearCodecDecoder;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // [largeur − 1][hauteur − 1][longueur de la première image, u16] puis deux
    // images décodées à la suite par le MÊME décodeur : la seconde exerce les
    // caches remplis par la première (glyphe réutilisé, barres verticales).
    // Les côtés restent sous 128 pour que l'exploration aille vite.
    if data.len() < 4 {
        return;
    }
    let largeur = u16::from(data[0] & 0x7F) + 1;
    let hauteur = u16::from(data[1] & 0x7F) + 1;
    let reste = &data[4..];
    let coupe = usize::from(u16::from_le_bytes([data[2], data[3]])).min(reste.len());
    let (premiere, seconde) = reste.split_at(coupe);
    let mut decodeur = ClearCodecDecoder::new();
    for image in [premiere, seconde] {
        if let Ok(pixels) = decodeur.decode(image, largeur, hauteur) {
            // Une image acceptée a exactement ses pixels, jamais moins : le
            // canal graphique les recopie sans autre vérification.
            assert_eq!(
                pixels.len(),
                usize::from(largeur) * usize::from(hauteur) * 4,
                "image acceptée de la mauvaise taille"
            );
        }
    }
});
