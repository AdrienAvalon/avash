//! Les fichiers YAML d'Avash (`rdp.yaml`, `tunnels.yaml`, `snippets.yaml`).
//!
//! Cible ajoutée le 22 novembre 2026 avec le passage de `serde_yaml` à
//! `serde-saphyr` : un lecteur neuf sur des fichiers que l'utilisateur édite à
//! la main, ou qu'un outil de synchronisation peut lui déposer. Deux
//! invariants : aucune entrée ne fait paniquer ni ne développe une bombe
//! d'alias (le lecteur borne son budget), et tout ce qui a été lu se réécrit
//! puis se relit à l'identique, faute de quoi la prochaine sauvegarde
//! changerait un bureau, un tunnel ou une commande en silence.
#![no_main]

use avash::rdphost::RdpHost;
use avash::snippet::Snippet;
use avash::tunnel::TunnelDef;
use libfuzzer_sys::fuzz_target;

fn aller_retour<T>(contenu: &str)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let Ok(lus) = serde_saphyr::from_str::<Vec<T>>(contenu) else {
        return;
    };
    let texte = serde_saphyr::to_string(&lus).expect("ce qui a été lu doit pouvoir se réécrire");
    let relus: Vec<T> = serde_saphyr::from_str(&texte)
        .unwrap_or_else(|e| panic!("réécriture illisible ({e}) :\n{texte}"));
    assert_eq!(relus, lus, "valeur changée par un aller-retour :\n{texte}");
}

fuzz_target!(|data: &[u8]| {
    // Les lecteurs reçoivent ce que `read_to_string` aurait rendu.
    let contenu = String::from_utf8_lossy(data);
    aller_retour::<RdpHost>(&contenu);
    aller_retour::<TunnelDef>(&contenu);
    aller_retour::<Snippet>(&contenu);
});
