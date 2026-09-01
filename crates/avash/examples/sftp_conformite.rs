//! Éprouve SFTP contre le VRAI sshd du parc : dépôt, relecture, effacement.
//!
//! SFTP était couvert par des tests d'intégration contre un serveur monté en
//! mémoire — c'est-à-dire contre notre compréhension du protocole. Le parc, lui,
//! parle à un OpenSSH véritable, dont le sous-système SFTP n'est pas le nôtre.
//!
//! Usage : cargo run -p avash --example `sftp_conformite` -- <port> <user> <mdp>
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let mut a = std::env::args().skip(1);
    let port: u16 = a.next().and_then(|v| v.parse().ok()).unwrap_or(2222);
    let user = a.next().unwrap_or_else(|| "essai".to_owned());
    let mdp = a.next().unwrap_or_else(|| "essai-mot-de-passe".to_owned());

    // Fichier de confiance neuf : le parc regénère sa clé d'hôte à chaque
    // construction, sinon un premier contact deviendrait un changement de clé.
    let bac = std::env::temp_dir().join(format!("avash-conf-sftp-{}", std::process::id()));
    std::fs::create_dir_all(bac.join(".ssh")).ok();
    unsafe { std::env::set_var("AVASH_HOME", &bac) };

    let auth = avash::ssh::ClientAuth {
        user,
        key_path: None,
        password: Some(mdp),
    };
    let resultat = eprouver(&auth, port).await;
    std::fs::remove_dir_all(&bac).ok();

    match resultat {
        Ok(taille) => {
            println!("  ✓ SFTP : dépôt, relecture ({taille} octets) et effacement");
            ExitCode::SUCCESS
        }
        Err(e) => {
            println!("  ✗ SFTP : {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn eprouver(auth: &avash::ssh::ClientAuth, port: u16) -> anyhow::Result<usize> {
    let s = avash::ssh::AvashSession::connect("127.0.0.1", port, auth).await?;
    let h = avash::sftp::SftpHandle::open(s).await?;

    // Un contenu qui n'est ni vide ni purement ASCII : les deux cas où un
    // transfert peut sembler fonctionner sans l'être.
    let contenu: Vec<u8> = "conformité SFTP — accents, ligne 1\n\u{0}\u{1}\u{2}binaire\n"
        .bytes()
        .cycle()
        .take(200_000)
        .collect();
    let distant = format!("/tmp/avash-conf-{}.bin", std::process::id());
    let local = std::env::temp_dir().join(format!("avash-conf-{}.bin", std::process::id()));
    std::fs::write(&local, &contenu)?;

    h.upload(&local, &distant).await?;
    let relu = std::env::temp_dir().join(format!("avash-conf-relu-{}.bin", std::process::id()));
    h.download(&distant, &relu).await?;

    let octets = std::fs::read(&relu)?;
    anyhow::ensure!(
        octets == contenu,
        "le fichier relu diffère de l'original ({} contre {} octets)",
        octets.len(),
        contenu.len()
    );

    h.remove(&distant, false).await?;
    anyhow::ensure!(
        h.download(&distant, &relu).await.is_err(),
        "le fichier effacé se télécharge encore"
    );

    std::fs::remove_file(&local).ok();
    std::fs::remove_file(&relu).ok();
    Ok(contenu.len())
}
