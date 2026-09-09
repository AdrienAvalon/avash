//! La sortie de `reg query` (sessions PuTTY dans le registre Windows).
//!
//! Portée réelle de cette cible, dite ici plutôt que supposée (audit du
//! 9 septembre 2026) : les campagnes tournent sur `ubuntu-latest`, où
//! `decoder_nom_putty_windows` prend sa branche `cfg(not(windows))`, un simple
//! repli UTF-8. La FFI vers `MultiByteToWideChar`, ses conversions de longueur
//! et sa troncature UTF-16, qui sont le morceau délicat, ne sont donc JAMAIS
//! secouées ici. Ce qui les couvre, c'est le job « Cœur — tests Windows » de
//! `ci.yml`, qui joue `cargo test -p avash --all-targets` sur un vrai Windows,
//! et en particulier `une_page_de_code_windows_est_decodee_sans_mojibake` dans
//! `crates/avash/src/import.rs`. Ce que cette cible-ci couvre vraiment sur
//! toute plateforme : le découpage de la sortie du registre, le déséchappement
//! `%XX` de `mungestr` et la construction des sessions.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let sortie = String::from_utf8_lossy(data);
    let lecture = avash::import::parse_reg_query(&sortie);
    for s in &lecture.sessions {
        assert!(!s.host.alias.is_empty(), "alias vide");
        assert_ne!(s.host.port, Some(0), "port nul");
    }
});
