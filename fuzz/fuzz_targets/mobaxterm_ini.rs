//! `MobaXterm.ini` : sections `[Bookmarks]`, lignes `Nom=#109#…` (SSH) et
//! `Nom=#91#…` (bureau RDP) découpées sur `%`.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let contenu = String::from_utf8_lossy(data);
    let lecture = avash::import::parse_mobaxterm_ini(&contenu);
    for s in &lecture.sessions {
        assert!(!s.host.alias.is_empty(), "alias vide");
        assert_ne!(s.host.port, Some(0), "port nul");
    }
    for b in &lecture.bureaux {
        assert!(!b.name.is_empty(), "bureau sans nom");
        assert!(!b.host.is_empty(), "bureau sans hôte");
        assert_ne!(b.port, 0, "port RDP nul");
    }
});
