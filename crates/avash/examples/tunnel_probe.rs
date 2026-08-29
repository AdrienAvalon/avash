//! Sonde manuelle : ouvre les trois types de tunnel sur un vrai sshd et
//! verifie que des octets les traversent.
//! Usage : cargo run -p avash --example `tunnel_probe` -- <port> <`chemin_cle`>
use avash::tunnel::{Tunnel, TunnelDef, TunnelKind};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn free_port() -> u16 {
    let l = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    l.local_addr().unwrap().port()
}

async fn read_some(s: &mut tokio::net::TcpStream) -> anyhow::Result<String> {
    let mut buf = vec![0u8; 256];
    let n = tokio::time::timeout(Duration::from_secs(5), s.read(&mut buf)).await??;
    Ok(String::from_utf8_lossy(&buf[..n]).trim().to_string())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let port: u16 = args.next().unwrap().parse()?;
    let key = args.next().unwrap();
    let auth = avash::ssh::ClientAuth {
        user: avash::ssh::current_username(),
        key_path: Some(key.into()),
        password: None,
    };
    let connect = || avash::ssh::AvashSession::connect("127.0.0.1", port, &auth);

    // -L : localhost:X -> serveur -> 127.0.0.1:22 — on doit lire la banniere sshd.
    let bind = free_port().await;
    let t = Tunnel::open(
        connect().await?,
        TunnelDef::new("probe", TunnelKind::Local, bind, "127.0.0.1", port, ""),
    )
    .await?;
    let mut c = tokio::net::TcpStream::connect(("127.0.0.1", bind)).await?;
    let banner = read_some(&mut c).await?;
    println!("-L  ✓ via localhost:{bind} : {banner}");
    drop(c);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let s = t.snapshot();
    println!(
        "    compteurs : total={} actives={} ↑{} ↓{}",
        s.total, s.active, s.bytes_up, s.bytes_down
    );
    t.close().await;

    // -D : SOCKS5 CONNECT vers 127.0.0.1:22.
    let bind = free_port().await;
    let t = Tunnel::open(
        connect().await?,
        TunnelDef::new("probe", TunnelKind::Dynamic, bind, "", 0, ""),
    )
    .await?;
    let mut c = tokio::net::TcpStream::connect(("127.0.0.1", bind)).await?;
    c.write_all(&[5, 1, 0]).await?;
    let mut rep = [0u8; 2];
    c.read_exact(&mut rep).await?;
    let mut req = vec![5, 1, 0, 1, 127, 0, 0, 1];
    req.extend_from_slice(&port.to_be_bytes());
    c.write_all(&req).await?;
    let mut ok = [0u8; 10];
    c.read_exact(&mut ok).await?;
    anyhow::ensure!(ok[1] == 0, "SOCKS refuse : {}", ok[1]);
    let banner = read_some(&mut c).await?;
    println!("-D  ✓ SOCKS5 localhost:{bind} : {banner}");
    t.close().await;

    // -R : serveur:Y -> nous -> service local (echo en majuscules).
    let local = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let local_port = local.local_addr()?.port();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = local.accept().await {
            let mut buf = [0u8; 64];
            if let Ok(n) = s.read(&mut buf).await {
                let up = String::from_utf8_lossy(&buf[..n]).to_uppercase();
                let _ = s.write_all(up.as_bytes()).await;
            }
        }
    });
    let remote_port = free_port().await;
    let t = Tunnel::open(
        connect().await?,
        TunnelDef::new(
            "probe",
            TunnelKind::Remote,
            remote_port,
            "127.0.0.1",
            local_port,
            "",
        ),
    )
    .await?;
    // Le serveur est cette machine : on frappe au port qu'il ecoute pour nous.
    let mut c = tokio::net::TcpStream::connect(("127.0.0.1", t.bound_port())).await?;
    c.write_all(b"hello via -R").await?;
    let back = read_some(&mut c).await?;
    anyhow::ensure!(back == "HELLO VIA -R", "reponse inattendue : {back:?}");
    println!(
        "-R  ✓ serveur:{} -> local:{local_port} : {back}",
        t.bound_port()
    );
    drop(c);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let s = t.snapshot();
    println!(
        "    compteurs : total={} ↑{} ↓{}",
        s.total, s.bytes_up, s.bytes_down
    );
    t.close().await;

    // Apres cancel : le port ne doit plus repondre.
    tokio::time::sleep(Duration::from_millis(200)).await;
    match tokio::net::TcpStream::connect(("127.0.0.1", remote_port)).await {
        Ok(_) => println!("-R  ⚠ le port {remote_port} repond encore apres fermeture"),
        Err(_) => println!("-R  ✓ port {remote_port} libere apres fermeture"),
    }
    Ok(())
}
