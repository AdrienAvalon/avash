//! Trames vers l'interface : zone sale bornée, format binaire des rectangles.

use ironrdp::pdu::geometry::InclusiveRectangle;
use ironrdp::session::image::DecodedImage;

/// Nombre maximal de rectangles portés par une trame.
const RECTS_MAX: usize = 8;

/// En-tête d'un rectangle dans le message : x, y, largeur, hauteur (u16).
const ENTETE_RECT: usize = 8;

/// Ajoute un rectangle à la zone sale, en ne fusionnant que si c'est rentable.
///
/// L'ancienne version gardait une **union englobante** : deux petites zones aux
/// coins opposés donnaient un rectangle plein écran. Mesuré contre un vrai xrdp,
/// sur une session animée : 7,94 Mo envoyés pour 4,35 Mo utiles, soit 1,8 fois
/// trop dès que trois zones se rejoignaient.
///
/// La règle est arithmétique, pas heuristique : on ne fusionne que si l'union
/// coûte moins cher que les deux rectangles séparés, en-têtes compris. Deux
/// zones voisines fusionnent donc ; deux zones opposées, jamais.
///
/// Au-delà de `RECTS_MAX`, il faut bien céder : on fusionne alors la paire dont
/// l'union gaspille le moins. Une trame ne peut pas porter un nombre illimité
/// de rectangles.
pub(crate) fn ajouter_rect(zone: &mut Vec<InclusiveRectangle>, r: &InclusiveRectangle) {
    let aire = |a: &InclusiveRectangle| {
        (u64::from(a.right) - u64::from(a.left) + 1) * (u64::from(a.bottom) - u64::from(a.top) + 1)
    };
    let union = |a: &InclusiveRectangle, b: &InclusiveRectangle| InclusiveRectangle {
        left: a.left.min(b.left),
        top: a.top.min(b.top),
        right: a.right.max(b.right),
        bottom: a.bottom.max(b.bottom),
    };
    let cout = |a: &InclusiveRectangle| aire(a) * 4 + ENTETE_RECT as u64;

    for e in zone.iter_mut() {
        let fusion = union(e, r);
        if cout(&fusion) <= cout(e) + cout(r) {
            *e = fusion;
            return;
        }
    }
    zone.push(r.clone());
    while zone.len() > RECTS_MAX {
        let mut choix = (u64::MAX, 0usize, 1usize);
        for i in 0..zone.len() {
            for j in (i + 1)..zone.len() {
                // saturating_sub : deux rectangles qui SE CHEVAUCHENT ont une union
                // plus petite que la somme de leurs aires — la soustraction brute
                // débordait (panique en debug/test, enroulement silencieux en
                // release, où la valeur ~u64::MAX n'était alors jamais choisie et la
                // paire chevauchante ne fusionnait jamais). Saturé à 0, une paire
                // qui se recouvre devient au contraire la moins coûteuse à fusionner
                // — exactement ce qu'on veut.
                let perte = aire(&union(&zone[i], &zone[j]))
                    .saturating_sub(aire(&zone[i]) + aire(&zone[j]));
                if perte < choix.0 {
                    choix = (perte, i, j);
                }
            }
        }
        let (_, i, j) = choix;
        zone[i] = union(&zone[i], &zone[j]);
        zone.remove(j);
    }
}

/// Zone sale -> message binaire. Un seul rectangle garde la forme historique
/// `[2]` ; plusieurs empruntent `[13]`, qui porte leur nombre. Une trame, un
/// accusé de rendu : le cadencement reste exact.
pub(crate) fn frames_msg(image: &DecodedImage, zone: &[InclusiveRectangle]) -> Vec<u8> {
    if let [seul] = zone {
        return frame_msg(image, seul);
    }
    let iw = usize::from(image.width());
    let data = image.data();
    // Capacité calculée d'avance : 2 octets d'en-tête + par rectangle 8 octets de
    // géométrie et w*h*4 de pixels. Sans elle, le Vec repartait de 1 octet et
    // doublait ~20 fois sur un message plein écran (plusieurs Mo), recopiant tout
    // le contenu déjà écrit à chaque fois — le frère `frame_msg` réservait pourtant.
    let capacite = 2 + zone
        .iter()
        .map(|r| {
            let (w, h) = (
                usize::from(r.right - r.left + 1),
                usize::from(r.bottom - r.top + 1),
            );
            8 + w * h * 4
        })
        .sum::<usize>();
    let mut m = Vec::with_capacity(capacite);
    m.push(13u8);
    m.push(u8::try_from(zone.len()).unwrap_or(u8::MAX));
    for r in zone {
        let (x, y) = (r.left, r.top);
        let (w, h) = (r.right - r.left + 1, r.bottom - r.top + 1);
        m.extend_from_slice(&x.to_le_bytes());
        m.extend_from_slice(&y.to_le_bytes());
        m.extend_from_slice(&w.to_le_bytes());
        m.extend_from_slice(&h.to_le_bytes());
        for row in 0..usize::from(h) {
            let start = ((usize::from(y) + row) * iw + usize::from(x)) * 4;
            m.extend_from_slice(&data[start..start + usize::from(w) * 4]);
        }
    }
    m
}

pub(crate) fn frame_msg(
    image: &DecodedImage,
    r: &ironrdp::pdu::geometry::InclusiveRectangle,
) -> Vec<u8> {
    let iw = usize::from(image.width());
    let data = image.data();
    let (x, y) = (r.left, r.top);
    let (w, h) = (r.right - r.left + 1, r.bottom - r.top + 1);
    let mut m = Vec::with_capacity(9 + usize::from(w) * usize::from(h) * 4);
    m.push(2);
    m.extend_from_slice(&x.to_le_bytes());
    m.extend_from_slice(&y.to_le_bytes());
    m.extend_from_slice(&w.to_le_bytes());
    m.extend_from_slice(&h.to_le_bytes());
    for row in 0..usize::from(h) {
        let start = ((usize::from(y) + row) * iw + usize::from(x)) * 4;
        m.extend_from_slice(&data[start..start + usize::from(w) * 4]);
    }
    m
}

#[cfg(test)]
mod tests_zone_sale {
    use super::{ajouter_rect, RECTS_MAX};
    use ironrdp::pdu::geometry::InclusiveRectangle;

    fn r(l: u16, t: u16, ri: u16, b: u16) -> InclusiveRectangle {
        InclusiveRectangle {
            left: l,
            top: t,
            right: ri,
            bottom: b,
        }
    }

    #[test]
    fn deux_zones_voisines_fusionnent() {
        // Côte à côte : l'union ne coûte pas plus que les deux séparés.
        let mut z = Vec::new();
        ajouter_rect(&mut z, &r(0, 0, 9, 9));
        ajouter_rect(&mut z, &r(10, 0, 19, 9));
        assert_eq!(z.len(), 1, "deux zones contiguës doivent n'en faire qu'une");
        assert_eq!((z[0].left, z[0].right), (0, 19));
    }

    #[test]
    fn deux_coins_opposes_ne_fusionnent_pas() {
        // C'est LE cas qui envoyait un plein écran pour deux poussières.
        let mut z = Vec::new();
        ajouter_rect(&mut z, &r(0, 0, 9, 9));
        ajouter_rect(&mut z, &r(1200, 700, 1209, 709));
        assert_eq!(z.len(), 2, "deux coins opposés doivent rester séparés");
    }

    #[test]
    fn un_rectangle_inclus_disparait_dans_le_sien() {
        let mut z = Vec::new();
        ajouter_rect(&mut z, &r(0, 0, 99, 99));
        ajouter_rect(&mut z, &r(10, 10, 19, 19));
        assert_eq!(z.len(), 1);
        assert_eq!((z[0].right, z[0].bottom), (99, 99));
    }

    #[test]
    fn le_nombre_de_rectangles_reste_borne() {
        // Une trame ne peut pas porter un nombre illimité de zones : au-delà du
        // plafond, la paire la moins coûteuse fusionne.
        let mut z = Vec::new();
        for i in 0..40u16 {
            let x = i * 30;
            ajouter_rect(&mut z, &r(x, x, x + 5, x + 5));
        }
        assert!(
            z.len() <= RECTS_MAX,
            "zone non bornée : {} rectangles",
            z.len()
        );
    }

    #[test]
    fn des_rectangles_qui_se_chevauchent_au_dela_du_plafond_ne_paniquent_pas() {
        // Bug corrigé : le choix de la paire à fusionner calculait
        // aire(union) - aire(a) - aire(b) ; pour deux rectangles qui se recouvrent,
        // l'union est plus petite que la somme → soustraction u64 négative, panique
        // en debug/test. Il faut PLUS de RECTS_MAX rectangles, dont certains se
        // chevauchent, pour entrer dans la boucle de fusion fautive.
        let mut z = Vec::new();
        for i in 0..(RECTS_MAX as u16 + 4) {
            // Des rectangles largement recouvrants (pas seulement inclus l'un dans
            // l'autre, sinon ils fusionneraient avant d'atteindre la boucle).
            ajouter_rect(&mut z, &r(i * 3, i * 3, i * 3 + 40, i * 3 + 40));
        }
        assert!(z.len() <= RECTS_MAX, "zone non bornée : {}", z.len());
    }

    #[test]
    fn la_zone_couvre_toujours_tout_ce_qui_a_ete_signale() {
        // Propriété essentielle : on peut fusionner, jamais PERDRE un pixel sale.
        let mut z = Vec::new();
        let entrees = [
            r(5, 5, 9, 9),
            r(700, 400, 720, 420),
            r(1200, 10, 1210, 20),
            r(300, 300, 305, 305),
        ];
        for e in &entrees {
            ajouter_rect(&mut z, e);
        }
        for e in &entrees {
            assert!(
                z.iter().any(|c| c.left <= e.left
                    && c.top <= e.top
                    && c.right >= e.right
                    && c.bottom >= e.bottom),
                "le rectangle {e:?} n'est couvert par aucune zone"
            );
        }
    }
}

#[cfg(test)]
mod tests_trames {
    use super::{frame_msg, frames_msg};
    use crate::entrees::mouse_button;
    use ironrdp::graphics::image_processing::PixelFormat;
    use ironrdp::pdu::geometry::InclusiveRectangle;
    use ironrdp::session::image::DecodedImage;

    fn image_numerotee(l: u16, h: u16) -> DecodedImage {
        // Chaque pixel porte sa position : un rectangle mal découpé se voit.
        let mut image = DecodedImage::new(PixelFormat::RgbA32, l, h);
        let pixels: Vec<u8> = (0..usize::from(l) * usize::from(h))
            .flat_map(|i| [(i % 256) as u8, (i / 256) as u8, 0xAA, 0xFF])
            .collect();
        image.peindre_rgba(0, 0, l, h, &pixels);
        image
    }

    fn r(l: u16, t: u16, ri: u16, b: u16) -> InclusiveRectangle {
        InclusiveRectangle {
            left: l,
            top: t,
            right: ri,
            bottom: b,
        }
    }

    /// Le format binaire est le contrat avec l'interface (`ws.onmessage`) : un
    /// octet de type, quatre u16 petit-boutiens, puis les pixels ligne par
    /// ligne. Aucun test ne le fixait.
    #[test]
    fn une_trame_simple_porte_sa_geometrie_et_ses_pixels() {
        let image = image_numerotee(8, 4);
        let m = frame_msg(&image, &r(2, 1, 4, 2)); // 3 × 2 pixels
        assert_eq!(m[0], 2);
        assert_eq!(&m[1..9], &[2, 0, 1, 0, 3, 0, 2, 0]);
        assert_eq!(m.len(), 9 + 3 * 2 * 4);
        // Premier pixel copié = position (2,1) = indice 1*8+2 = 10.
        assert_eq!(&m[9..13], &[10, 0, 0xAA, 0xFF]);
        // Première ligne complète : indices 10, 11, 12 ; puis 18, 19, 20.
        assert_eq!(m[9 + 3 * 4], 18, "la seconde ligne suit le pas de l'image");
    }

    #[test]
    fn un_seul_rectangle_garde_la_forme_historique() {
        let image = image_numerotee(8, 4);
        let zone = [r(0, 0, 1, 1)];
        assert_eq!(frames_msg(&image, &zone), frame_msg(&image, &zone[0]));
    }

    #[test]
    fn plusieurs_rectangles_sont_concatenes_avec_leur_compte() {
        let image = image_numerotee(8, 4);
        let zone = [r(0, 0, 1, 0), r(6, 3, 7, 3)]; // 2×1 chacun
        let m = frames_msg(&image, &zone);
        assert_eq!(m[0], 13);
        assert_eq!(m[1], 2, "nombre de rectangles");
        assert_eq!(m.len(), 2 + 2 * (8 + 2 * 4));
        assert_eq!(&m[2..10], &[0, 0, 0, 0, 2, 0, 1, 0]);
        assert_eq!(&m[10..14], &[0, 0, 0xAA, 0xFF]);
        let second = 2 + 8 + 8;
        assert_eq!(&m[second..second + 8], &[6, 0, 3, 0, 2, 0, 1, 0]);
        // (6,3) = indice 3*8+6 = 30.
        assert_eq!(&m[second + 8..second + 12], &[30, 0, 0xAA, 0xFF]);
    }

    #[test]
    fn les_boutons_de_souris_suivent_la_convention_du_front() {
        use ironrdp::input::MouseButton;
        // 0 = gauche (et tout inconnu), 1 = milieu, 2 = droit — l'ordre de
        // `MouseEvent.button` du navigateur, transmis tel quel.
        assert_eq!(mouse_button(0), MouseButton::Left);
        assert_eq!(mouse_button(1), MouseButton::Middle);
        assert_eq!(mouse_button(2), MouseButton::Right);
        assert_eq!(mouse_button(3), MouseButton::X1);
        assert_eq!(mouse_button(4), MouseButton::X2);
        assert_eq!(mouse_button(200), MouseButton::Left);
    }
}
