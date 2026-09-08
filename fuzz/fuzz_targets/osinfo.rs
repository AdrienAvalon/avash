//! La sonde d'OS (`osinfo::parse_probe_output`) : elle interprète la sortie
//! brute d'une commande distante, stdout ET stderr mêlés, donc du texte que le
//! serveur (ou son shell) contrôle entièrement. Parseur jusque-là sans cible,
//! rendu tolérant au bruit de stderr par l'audit du 7 septembre 2026.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let texte = String::from_utf8_lossy(data);
    // On veut surtout qu'il rende la main sans paniquer sur une entrée
    // quelconque. Les invariants ci-dessous ne valent que s'il classe l'hôte.
    if let Some(info) = avash::osinfo::parse_probe_output(&texte) {
        // Un `id` classé est toujours non vide et en minuscules : le front s'en
        // sert pour choisir un fichier de logo, un `id` vide ou mixte casserait
        // la correspondance.
        assert!(!info.id.is_empty(), "id vide alors qu'un OS est rendu");
        assert_eq!(
            info.id,
            info.id.to_lowercase(),
            "id non minuscule : {info:?}"
        );
        // La famille `bsd` est le seul `like` déduit hors os-release.
        for f in &info.like {
            assert!(!f.is_empty(), "famille vide");
        }
    }
});
