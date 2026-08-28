//! Sonde manuelle : ouvre un PTY sur un vrai serveur et affiche ce qui arrive.
//! Usage : cargo run -p avash --example pty_probe -- <port> <chemin_cle>
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let port: u16 = args.next().unwrap().parse()?;
    let key = args.next().unwrap();

    let auth = avash::ssh::ClientAuth {
        user: whoami::username(),
        key_path: Some(key.into()),
        password: None,
    };
    println!("→ connexion 127.0.0.1:{port}");
    let mut session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth).await?;
    println!("✓ connecté, ouverture du PTY");
    let mut pty = session.open_pty(80, 24, "xterm-256color").await?;
    println!("✓ PTY ouvert, attente de sortie (8 s)");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    let mut total = 0usize;
    loop {
        match tokio::time::timeout_at(deadline, pty.out_rx.recv()).await {
            Ok(Some(b)) => {
                total += b.len();
                println!("  ← {} octets : {:?}", b.len(), String::from_utf8_lossy(&b));
            }
            Ok(None) => {
                println!("  canal ferme");
                break;
            }
            Err(_) => break,
        }
    }
    // Le shell distant interroge le terminal (DA1, couleur de fond) et attend
    // les reponses avant d'afficher son invite. xterm.js y repond ; ici il faut
    // le faire a la main pour reproduire des conditions realistes.
    println!("→ reponse aux interrogations du terminal");
    pty.in_tx.send(b"\x1b[?62;1;2;6;8;9;15;c".to_vec()).await?; // DA1
    pty.in_tx
        .send(b"\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\".to_vec())
        .await?; // couleur de fond
    tokio::time::sleep(Duration::from_millis(600)).await;
    println!("→ envoi de \"echo BONJOUR\\n\"");
    pty.in_tx.send(b"echo BONJOUR_AVASH\n".to_vec()).await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while let Ok(Some(b)) = tokio::time::timeout_at(deadline, pty.out_rx.recv()).await {
        total += b.len();
        println!("  ← {} octets : {:?}", b.len(), String::from_utf8_lossy(&b));
    }
    println!("=== total recu : {total} octets ===");
    Ok(())
}
