//! `~/.ssh/config` : la surface d'entrée la plus exposée du cœur. Mêmes
//! invariants que le test de mutation déterministe de `lib.rs`, mais avec un
//! générateur guidé par la couverture au lieu d'une graine fixe.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Le parseur reçoit ce que `read_to_string` aurait rendu.
    let contenu = String::from_utf8_lossy(data);
    for h in avash::parse_config_str(&contenu) {
        assert!(!h.alias.is_empty(), "alias vide");
        assert!(!h.alias.contains(['\n', '\r']), "alias multiligne");
        assert_ne!(h.port, Some(0), "port nul accepté");
        if let Some(pj) = &h.proxy_jump {
            for hop in avash::split_proxy_jump(pj) {
                assert_eq!(hop.host.trim(), hop.host, "rebond non rogné : {hop:?}");
                assert_ne!(hop.port, Some(0), "port de rebond nul");
            }
        }
        // Ce que le parseur rend doit pouvoir être réécrit puis relu : un bloc
        // rendu illisible serait perdu à la prochaine sauvegarde.
        let bloc = avash::render_host_block(&h);
        let relu = avash::parse_config_str(&bloc);
        assert_eq!(relu.len(), 1, "bloc rendu illisible :\n{bloc}");
        assert_eq!(relu[0].alias, h.alias, "alias perdu au rendu :\n{bloc}");
    }
});
