//! Assemblage du canal graphique (MS-RDPEGFX) par le processus RDP : découpage
//! d'un segment en commandes, surfaces, cache, remplissages, copies, et
//! surtout l'assemblage RemoteFX Progressive (`progressif.rs` : lecteur SRL,
//! paliers d'affinage, tuiles en différence, quantifications), qui est du code
//! d'Avash et lit des indices de tuile, des longueurs et des paliers fournis
//! par le serveur.
//!
//! Audit du 27 novembre 2026 (QT-11) : ce chemin n'était secoué que par la
//! mutation à graine fixe de `magnetoscope.rs` (les mêmes 150 mutations à
//! chaque passage) et par `tests/assemblage_hostile.rs`, sans exploration
//! guidée par la couverture. La bibliothèque `avash_rdp`, ouverte par l'audit
//! du 7 septembre pour cette cible, le rend atteignable d'ici ; la cible prend
//! la même frontière que le processus RDP : `decouper` puis `Egfx::traiter`,
//! sur le flux déjà décompressé (ZGFX est l'affaire d'IronRDP).
#![no_main]

use avash_rdp::egfx::{decouper, Egfx, Pdu};
use libfuzzer_sys::fuzz_target;

/// Surface de départ bornée comme celle de `clearcodec` : 4 Mpx, 16 Mio une
/// fois en RGBA. Le processus RDP accepte plus grand (`TAILLE_MAX` par côté) ;
/// la cible reste sous le plafond de mémoire de libFuzzer.
const SURFACE_MAX: usize = 1 << 22;

/// Ce que la file de trames peut porter au plus après un seul PDU, en octets
/// de pixels : le budget de `publier` (`OCTETS_PUBLIES_MAX`, 64 Mio) a été posé
/// le 12 septembre 2026 contre une région aux tuiles répétées qui empilait un
/// gigaoctet pour un seul PDU (C-panique-2). La file est vidée après chaque
/// PDU, comme le fait la boucle de session, si bien qu'un dépassement ne peut
/// venir que d'une garde tombée.
const OCTETS_PAR_PDU_MAX: usize = 64 * 1024 * 1024;

fuzz_target!(|data: &[u8]| {
    // [largeur u16][hauteur u16] puis des commandes EGFX encadrées (id u16,
    // réservé u16, longueur totale u32, charge), telles que le serveur les
    // envoie après décompression. La surface 1 est créée d'office pour que les
    // premières mutations atteignent le décodage au lieu d'échouer sur une
    // surface inconnue ; le flux peut en créer, supprimer ou réinitialiser
    // d'autres.
    if data.len() < 4 {
        return;
    }
    let largeur = u16::from_le_bytes([data[0], data[1]]);
    let hauteur = u16::from_le_bytes([data[2], data[3]]);
    if largeur == 0 || hauteur == 0 || usize::from(largeur) * usize::from(hauteur) > SURFACE_MAX {
        return;
    }
    let (mut egfx, _canal, file) = Egfx::nouveau();
    let l = largeur.to_le_bytes();
    let h = hauteur.to_le_bytes();
    // CreateSurface (0x0009) : identifiant 1, côtés, format XRGB (0x20).
    egfx.traiter(&Pdu {
        id: 0x0009,
        charge: &[1, 0, l[0], l[1], h[0], h[1], 0x20],
    });
    let mut vider = || match file.lock() {
        Ok(mut sortie) => std::mem::take(&mut sortie.trames),
        Err(empoisonne) => std::mem::take(&mut empoisonne.into_inner().trames),
    };
    vider();
    for pdu in decouper(&data[4..]) {
        egfx.traiter(&pdu);
        let trames = vider();
        let octets: usize = trames.iter().map(|t| t.pixels.len()).sum();
        assert!(
            octets <= OCTETS_PAR_PDU_MAX,
            "{octets} octets publiés pour un seul PDU {:#06x}",
            pdu.id
        );
        for t in &trames {
            // Une trame publiée porte exactement les pixels de sa zone : la
            // page les recopie dans le canevas sans autre vérification.
            assert_eq!(
                t.pixels.len(),
                usize::from(t.largeur) * usize::from(t.hauteur) * 4,
                "trame {}×{} de {} octets",
                t.largeur,
                t.hauteur,
                t.pixels.len()
            );
        }
    }
});
