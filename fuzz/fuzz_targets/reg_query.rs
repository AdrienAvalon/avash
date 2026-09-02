//! La sortie de `reg query` (sessions PuTTY dans le registre Windows).
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
