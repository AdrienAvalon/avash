//! Une session PuTTY (fichier `~/.putty/sessions/<nom>` sous Unix) et le
//! décodage `%XX` de son nom.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let contenu = String::from_utf8_lossy(data);
    // Le nom vient du système de fichiers ; on lui donne la même entrée pour
    // exercer le décodage sur des séquences `%` tronquées ou invalides.
    let nom = avash::import::decoder_nom_putty(&contenu);
    if let Some(s) = avash::import::parse_putty_session(&nom, &contenu) {
        assert!(!s.host.alias.is_empty(), "alias vide");
        assert!(s.host.hostname.as_deref().is_some_and(|h| !h.is_empty()), "hôte vide");
        assert_ne!(s.host.port, Some(0), "port nul");
    }
});
