//! Éprouve l'authentification contre un VRAI sshd du parc local.
//!
//! Le serveur du parc refuse la méthode `password` et n'accepte que
//! `keyboard-interactive` : c'est le comportement d'un hôte joint à un
//! annuaire, où SSSD répond par une conversation PAM. avash n'avait pas ce
//! repli — un compte de domaine ne pouvait pas se connecter, et c'est l'usage
//! qui l'a signalé, pas les tests.
//!
//! Usage : cargo run -p avash --example `ssh_conformite` -- <port> <user> <mdp>
//!
//! L'hôte vient de `PARC_HOTE` (127.0.0.1 par défaut ; « docker » sur GitLab,
//! où le parc tourne dans un démon à part).
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let mut a = std::env::args().skip(1);
    let port: u16 = a.next().and_then(|v| v.parse().ok()).unwrap_or(2222);
    let user = a.next().unwrap_or_else(|| "essai".to_owned());
    let mdp = a.next().unwrap_or_else(|| "essai-mot-de-passe".to_owned());

    // Le parc regénère sa clé d'hôte à chaque construction : on part d'un
    // fichier de confiance neuf, sinon le premier contact devient un
    // changement de clé et le test échoue pour une mauvaise raison.
    let bac = std::env::temp_dir().join(format!("avash-conf-ssh-{}", std::process::id()));
    std::fs::create_dir_all(bac.join(".ssh")).ok();
    unsafe { std::env::set_var("AVASH_HOME", &bac) };

    let auth = avash::ssh::ClientAuth {
        user: user.clone(),
        key_path: None,
        password: Some(mdp),
    };
    let hote = std::env::var("PARC_HOTE").unwrap_or_else(|_| "127.0.0.1".to_owned());
    let r = avash::ssh::AvashSession::connect(&hote, port, &auth).await;
    std::fs::remove_dir_all(&bac).ok();

    match r {
        Ok(mut s) => match s.run("echo conformite-ok").await {
            Ok((sortie, _)) if sortie.contains("conformite-ok") => {
                println!("  ✓ clavier-interactif (PAM) : authentifié, commande exécutée");
                ExitCode::SUCCESS
            }
            Ok((sortie, code)) => {
                println!("  ✗ commande inattendue (code {code}) : {sortie:?}");
                ExitCode::FAILURE
            }
            Err(e) => {
                println!("  ✗ session ouverte mais commande impossible : {e}");
                ExitCode::FAILURE
            }
        },
        Err(e) => {
            println!("  ✗ authentification refusée : {e:#}");
            println!("     (ce serveur n'accepte QUE keyboard-interactive)");
            ExitCode::FAILURE
        }
    }
}
