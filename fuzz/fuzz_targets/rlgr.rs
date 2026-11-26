//! Décodage RLGR (MS-RDPRFX 3.1.8.1.7.3), l'entropie de RemoteFX et de
//! RemoteFX progressif : le décodeur rapide porté (`rlgr::decode`, lecteur de
//! bits par mots, 26 novembre 2026) contre le décodeur d'origine gardé comme
//! référence (`rlgr_reference`). Tout le flux vient du serveur. La moindre
//! différence de verdict ou de sortie est un défaut du portage.
#![no_main]

use ironrdp_graphics::{rlgr, rlgr_reference};
use ironrdp_pdu::codecs::rfx::EntropyAlgorithm;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // [mode u8][longueur de la sortie u16] puis le flux. La sortie va
    // jusqu'à 65 535 coefficients : au-delà d'une tuile de 4 096, pour que
    // les deux décodeurs aillent au bout d'un flux plus court que leur sortie.
    let [mode, l0, l1, flux @ ..] = data else {
        return;
    };
    let mode = if mode & 1 == 0 {
        EntropyAlgorithm::Rlgr1
    } else {
        EntropyAlgorithm::Rlgr3
    };
    let longueur = usize::from(u16::from_le_bytes([*l0, *l1]));
    let mut rapide = vec![0x5a5a_i16; longueur];
    let mut reference = vec![0x5a5a_i16; longueur];
    let a = rlgr::decode(mode, flux, &mut rapide).map_err(|e| e.to_string());
    let b = rlgr_reference::decode(mode, flux, &mut reference).map_err(|e| e.to_string());
    assert_eq!(a, b, "verdicts différents");
    assert!(rapide == reference, "sorties différentes");
});
