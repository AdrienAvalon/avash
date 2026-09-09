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
    // [largeur u16][hauteur u16][longueur de la première image, u16] puis deux
    // images décodées à la suite par le MÊME décodeur : la seconde exerce les
    // caches remplis par la première (glyphe réutilisé, barres verticales).
    //
    // Les côtés étaient tirés sur 7 bits, donc bornés à 128 quelle que soit
    // l'entrée : le plafond anti-OOM du décodeur (`MAX_DECODE_DIM`, 8192
    // pixels par axe, posé contre un serveur RDP hostile) était HORS
    // D'ATTEINTE de la campagne, et une régression qui l'aurait supprimé
    // n'aurait fait rougir personne. Trouvé par l'audit du 9 septembre 2026.
    // On tire donc chaque côté sur un u16 entier, et c'est la SURFACE qu'on
    // borne, pas les côtés : une image de 9000 × 1 atteint le plafond pour
    // trois fois rien, là où 8192 × 8192 épuiserait la mémoire du fuzzeur
    // avant même d'avoir décodé quoi que ce soit.
    if data.len() < 6 {
        return;
    }
    let largeur = u16::from_le_bytes([data[0], data[1]]);
    let hauteur = u16::from_le_bytes([data[2], data[3]]);
    if largeur == 0 || hauteur == 0 {
        return;
    }
    const SURFACE_MAX: usize = 1 << 22; // 4 Mpx, soit 16 Mio une fois en RGBA
    if usize::from(largeur) * usize::from(hauteur) > SURFACE_MAX {
        return;
    }
    let reste = &data[6..];
    let coupe = usize::from(u16::from_le_bytes([data[4], data[5]])).min(reste.len());
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
