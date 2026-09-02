//! Relecture d'un enregistrement asciicast v2 (une ligne JSON par événement).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let contenu = String::from_utf8_lossy(data);
    if let Some((_entete, evenements)) = avash::enregistrement::relire(&contenu) {
        // Un événement par ligne, l'en-tête en plus : jamais davantage.
        assert!(
            evenements.len() < contenu.lines().count().max(1),
            "plus d'événements que de lignes"
        );
    }
});
