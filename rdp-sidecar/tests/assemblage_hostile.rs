//! Fuzzing par mutation déterministe de l'assemblage propre au sidecar, piloté
//! depuis l'EXTÉRIEUR du crate par sa bibliothèque `avash_rdp`.
//!
//! Trouvé par l'audit du 7 septembre 2026 : `rdp-sidecar` ne déclarait qu'un
//! `[[bin]]`, si bien que `CLAUDE.md` (« un nouveau parseur reçoit une cible
//! dans `fuzz/` ») restait intenable pour le cadrage EGFX (`egfx::decouper`
//! puis `Egfx::traiter`) et la correspondance de motifs RDPDR
//! (`disque::correspond`) : rien ne pouvait les appeler du dehors. Ces tests
//! d'intégration ne compilent qu'avec le `[lib]` ajouté par le même correctif ;
//! ils sont la garde rapide (mutation déterministe) qu'une cible `fuzz/`
//! guidée par la couverture prolongera.

use avash_rdp::disque::correspond;
use avash_rdp::egfx::{decouper, Egfx, Pdu};

/// Encadre une commande EGFX dans un en-tête filaire (id, réservé, longueur
/// totale) tel que `decouper` l'attend, pour reconstituer un vrai segment.
fn encadrer(id: u16, charge: &[u8]) -> Vec<u8> {
    let total = u32::try_from(charge.len() + 8).unwrap();
    let mut o = Vec::with_capacity(charge.len() + 8);
    o.extend_from_slice(&id.to_le_bytes());
    o.extend_from_slice(&[0, 0]); // réservé
    o.extend_from_slice(&total.to_le_bytes());
    o.extend_from_slice(charge);
    o
}

#[test]
fn un_flux_egfx_hostile_ne_fait_pas_paniquer_le_traitement() {
    // Une surface 64 × 64 créée en tête, comme la cible `fuzz/` décrite par
    // l'audit : `data → decouper → traiter`. Les côtés, codecs, rectangles et
    // longueurs venant tous du réseau, aucune mutation ne doit paniquer, ni
    // faire enfler la file de trames sans borne (les gardes octets_surfaces /
    // octets_publies / SURFACES_MAX doivent tenir).
    let (mut e, _canal, file) = Egfx::nouveau();
    // CreateSurface(id = 1, 64 × 64, format 0x21).
    e.traiter(&Pdu {
        id: 0x0009,
        charge: vec![1, 0, 64, 0, 64, 0, 0x21],
    });

    // Un segment de départ portant plusieurs commandes plausibles, puis on le
    // triture octet par octet à travers un générateur déterministe.
    let mut graine = Vec::new();
    graine.extend_from_slice(&encadrer(
        0x0001, // WireToSurface1
        &[
            1, 0, 0x03, 0x00, 0x20, 0, 0, 0, 0, 64, 0, 64, 0, 4, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF,
        ],
    ));
    graine.extend_from_slice(&encadrer(
        0x0005,
        &[1, 0, 0, 0, 0, 0, 8, 0, 8, 0, 1, 0, 2, 0, 2, 0],
    )); // SurfaceToSurface
    graine.extend_from_slice(&encadrer(0x0006, &[1, 0, 0, 0, 0, 0, 4, 0, 4, 0, 0, 0])); // SurfaceToCache
    graine.extend_from_slice(&encadrer(0x0007, &[3, 0, 1, 0, 0, 0, 0, 0, 0, 0])); // CacheToSurface

    let mut etat = 0x2545_1623u32;
    let mut prochain = || {
        // Générateur xorshift : reproductible, sans dépendance.
        etat ^= etat << 13;
        etat ^= etat >> 17;
        etat ^= etat << 5;
        etat
    };

    for _ in 0..2000 {
        let mut flux = graine.clone();
        // Deux à cinq octets retournés par tour, à des positions tirées.
        let n = 2 + (prochain() as usize % 4);
        for _ in 0..n {
            if flux.is_empty() {
                break;
            }
            let pos = prochain() as usize % flux.len();
            flux[pos] ^= (prochain() & 0xFF) as u8;
        }
        for pdu in decouper(&flux) {
            e.traiter(&pdu);
        }
        // La file peut accumuler des trames légitimes, mais jamais sans borne :
        // un demi-million d'entrées signalerait une garde tombée.
        let trames = file.lock().unwrap().trames.len();
        assert!(trames < 500_000, "file de trames débornée : {trames}");
    }
}

#[test]
fn un_motif_rdpdr_multi_etoiles_ne_fige_pas_l_appelant_externe() {
    // Le motif que le serveur envoie pour filtrer une énumération de dossier.
    // Trouvé par l'audit du 7 septembre 2026 : `a*a*a*…*b` contre une chaîne de
    // « a » sans « b » faisait exploser l'ancienne récursion sans mémoïsation.
    // Si la garde tombait, ce test ne terminerait pas ; qu'il rende la main
    // prouve la terminaison, et le résultat reste correct.
    let motif: String = std::iter::repeat_n("a*", 40)
        .chain(std::iter::once("b"))
        .collect();
    let nom = "a".repeat(200);
    assert!(
        !correspond(&motif, &nom),
        "aucun « b » : pas de correspondance"
    );

    // Contrôle positif : le même motif concorde dès qu'un « b » clôt le nom.
    let nom_ok = format!("{}b", "a".repeat(200));
    assert!(correspond(&motif, &nom_ok));
}
