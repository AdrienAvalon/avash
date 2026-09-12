//! Le processus RDP ne survit pas à l'application qui l'a lancé.
//!
//! Trouvé par l'audit du 12 septembre 2026 (C-sidecar-1) : le poste local est
//! établi APRÈS l'authentification, et la fin de l'entrée standard (le parent
//! a disparu) ne faisait que tarir les désignations. Si l'application tombait
//! entre le lancement et la connexion de sa WebSocket (plantage, `kill -9`,
//! fermeture brutale de la session graphique), personne ne tuait l'enfant :
//! une session RDP authentifiée restait ouverte sur le serveur, invisible,
//! jusqu'à son expiration côté serveur.
//!
//! Ces tests lancent le vrai binaire contre un serveur muet : la connexion TCP
//! aboutit (file d'attente du noyau) et plus rien ne répond, si bien que le
//! processus resterait 25 s dans sa négociation s'il ne regardait pas stdin.

use std::io::Write as _;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

fn lancer(extra: &[&str], port: u16, foyer: &std::path::Path) -> Child {
    let mut enfant = Command::new(env!("CARGO_BIN_EXE_avash-rdp"))
        .args([
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "-u",
            "essai",
            "--layout",
            "us",
            "--sans-son",
        ])
        .args(extra)
        // Bac à sable pour tout fichier de configuration : l'environnement de
        // l'enfant seulement, jamais `set_var` dans ce processus.
        .env("AVASH_HOME", foyer)
        .env_remove("AVASH_RDP_TRACE")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("lancement du processus RDP");
    let mut entree = enfant.stdin.take().expect("stdin de l'enfant");
    entree.write_all(b"mot-de-passe\n").unwrap();
    // Le parent disparaît : fin de l'entrée standard.
    drop(entree);
    enfant
}

fn attendre_la_fin(enfant: &mut Child, delai: Duration) -> Option<ExitStatus> {
    let debut = Instant::now();
    while debut.elapsed() < delai {
        if let Some(s) = enfant.try_wait().expect("état de l'enfant") {
            return Some(s);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}

fn foyer(nom: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("avash-stdin-{}-{nom}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn la_fin_de_l_entree_standard_termine_le_processus() {
    let serveur_muet = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = serveur_muet.local_addr().unwrap().port();
    let d = foyer("parent");
    let mut enfant = lancer(&[], port, &d);
    // Dix secondes : bien moins que les 25 s de la négociation bornée, bien plus
    // que le temps de voir la fin de stdin.
    let fin = attendre_la_fin(&mut enfant, Duration::from_secs(10));
    if fin.is_none() {
        let _ = enfant.kill();
        let _ = enfant.wait();
    }
    let _ = std::fs::remove_dir_all(&d);
    assert!(
        fin.is_some(),
        "le processus a survécu à la fin de son entrée standard (parent disparu)"
    );
}

/// Contrôle du mode manuel : `--shot` se lance à la main ou par un script
/// (`echo "$MDP" | avash-rdp --shot …`), stdin fermé juste après le mot de
/// passe. Il ne doit pas s'arrêter pour autant.
#[test]
fn en_capture_d_ecran_la_fin_de_l_entree_standard_ne_coupe_rien() {
    let serveur_muet = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = serveur_muet.local_addr().unwrap().port();
    let d = foyer("capture");
    let image = d.join("ecran.png");
    let mut enfant = lancer(&["--shot", image.to_str().unwrap()], port, &d);
    let fin = attendre_la_fin(&mut enfant, Duration::from_millis(1500));
    let _ = enfant.kill();
    let _ = enfant.wait();
    let _ = std::fs::remove_dir_all(&d);
    assert!(
        fin.is_none(),
        "le mode --shot s'est arrêté à la fin de stdin : {fin:?}"
    );
}
