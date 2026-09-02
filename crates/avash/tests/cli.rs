//! L'outil en ligne de commande `avash`, exercé comme un utilisateur le ferait.
//!
//! Il n'avait aucun test : ses trois commandes et ses codes de sortie ne
//! tenaient qu'à la relecture. On lance le vrai binaire, dans un répertoire
//! personnel isolé par `AVASH_HOME`.

use std::process::Command;

fn bac(nom: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("avash-cli-{}-{nom}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join(".ssh")).unwrap();
    d
}

fn avash(home: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_avash"))
        .args(args)
        .env("AVASH_HOME", home)
        .env("HOME", home)
        .output()
        .expect("lancement du binaire")
}

#[test]
fn list_affiche_les_hotes_du_config() {
    let home = bac("liste");
    std::fs::write(
        home.join(".ssh/config"),
        "Host prod\n  HostName 10.0.0.1\n  User adrien\n  Port 2222\n\nHost saut\n  HostName 10.0.0.2\n  ProxyJump prod\n",
    )
    .unwrap();
    let sortie = avash(&home, &["list"]);
    assert!(sortie.status.success());
    let texte = String::from_utf8_lossy(&sortie.stdout);
    assert!(texte.contains("2 hôtes"), "{texte}");
    assert!(texte.contains("adrien@10.0.0.1:2222"), "{texte}");
    assert!(
        texte.contains("(via prod)"),
        "le rebond doit être visible : {texte}"
    );
    // Sans sous-commande, `list` est le défaut.
    let defaut = avash(&home, &[]);
    assert_eq!(defaut.stdout, sortie.stdout);
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn sans_config_lisible_le_code_de_sortie_est_1_et_le_conseil_donne() {
    let home = bac("vide");
    let sortie = avash(&home, &["list"]);
    assert_eq!(sortie.status.code(), Some(1));
    let err = String::from_utf8_lossy(&sortie.stderr);
    assert!(err.contains("~/.ssh/config"), "{err}");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn une_commande_inconnue_sort_en_2() {
    let home = bac("inconnue");
    let sortie = avash(&home, &["bidule"]);
    assert_eq!(sortie.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&sortie.stderr).contains("Commande inconnue"));
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn run_exige_un_alias_et_une_commande() {
    let home = bac("run");
    std::fs::write(home.join(".ssh/config"), "Host prod\n  HostName 10.0.0.1\n").unwrap();
    let sans_alias = avash(&home, &["run"]);
    assert!(!sans_alias.status.success());
    assert!(String::from_utf8_lossy(&sans_alias.stderr).contains("Usage"));
    let alias_inconnu = avash(&home, &["run", "absent", "true"]);
    assert!(!alias_inconnu.status.success());
    assert!(String::from_utf8_lossy(&alias_inconnu.stderr).contains("introuvable"));
    let _ = std::fs::remove_dir_all(&home);
}
