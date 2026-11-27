//! Banc M : analyse d'une configuration SSH de 500 hôtes, celle qu'Avash relit
//! au démarrage et à chaque modification du fichier. Imprime une ligne
//! `BANC {json}` que scripts/banc.mjs récolte (échantillons en µs).
use std::fmt::Write as _;
use std::time::Instant;

fn main() {
    let mut cfg = String::new();
    for i in 0..500 {
        let _ = write!(
            cfg,
            "Host srv-{i}\n    HostName 10.0.{}.{}\n    User deploy\n    Port 22\n    IdentityFile ~/.ssh/id_ed25519\n    ProxyJump bastion\n    #Folder: prod/zone-{}\n\n",
            i / 250,
            i % 250,
            i % 7
        );
    }
    let mut echantillons = Vec::new();
    for _ in 0..30 {
        let t = Instant::now();
        let hotes = avash::parse_config_str(&cfg);
        echantillons.push(t.elapsed().as_secs_f64() * 1e6);
        assert_eq!(hotes.len(), 500);
    }
    let liste: Vec<String> = echantillons.iter().map(|v| format!("{v:.1}")).collect();
    println!(
        r#"BANC {{"cle":"M/config-ssh-500-hotes","unite":"µs","sens":"bas","echantillons":[{}]}}"#,
        liste.join(",")
    );
}
