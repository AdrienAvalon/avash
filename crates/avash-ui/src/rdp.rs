//! Intégration RDP côté Tauri : lance le sidecar `avash-rdp` (isolé de russh),
//! qui sert lui-même le bureau à la webview via un WebSocket local **binaire**
//! (vrai `ArrayBuffer` : pas de base64, pas de JSON — débit maximal). Avash ne
//! fait que gérer le cycle de vie du sidecar et transmettre le point de
//! connexion (port + jeton) au front.

use std::collections::HashMap;
use std::sync::Mutex;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Child;

/// Sessions RDP vivantes, par id d'onglet.
#[derive(Default)]
pub struct RdpStore {
    pub inner: Mutex<HashMap<u64, Child>>,
}

/// Point de connexion WebSocket renvoyé au front.
#[derive(serde::Serialize)]
pub struct RdpConn {
    pub port: u16,
    pub token: String,
}

fn sidecar_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("AVASH_RDP_BIN") {
        return p.into();
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let side = dir.join("avash-rdp");
            if side.exists() {
                return side;
            }
        }
    }
    "rdp-sidecar/target/release/avash-rdp".into()
}

/// Lance le sidecar et renvoie le WebSocket (port + jeton) qu'il annonce.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn rdp_open(
    state: tauri::State<'_, RdpStore>,
    id: u64,
    host: String,
    port: Option<u16>,
    user: String,
    password: String,
    width: u16,
    height: u16,
) -> Result<RdpConn, String> {
    use std::process::Stdio;
    let mut child = tokio::process::Command::new(sidecar_path())
        .args([
            "--host",
            &host,
            "--port",
            &port.unwrap_or(3389).to_string(),
            "-u",
            &user,
            "-p",
            &password,
            "--width",
            &width.to_string(),
            "--height",
            &height.to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Lancement du sidecar RDP impossible : {e}"))?;

    let stdout = child.stdout.take().ok_or("stdout sidecar indisponible")?;
    let mut stderr = child.stderr.take().ok_or("stderr sidecar indisponible")?;

    // Le sidecar imprime « PORT TOKEN » quand son WebSocket est prêt. S'il
    // échoue avant (auth, TLS, NLA…), on remonte son diagnostic (stderr).
    let mut lines = BufReader::new(stdout).lines();
    let first = lines.next_line().await.map_err(|e| e.to_string())?;
    let Some(line) = first else {
        let mut diag = String::new();
        let _ = stderr.read_to_string(&mut diag).await;
        let msg = diag.trim().lines().last().unwrap_or("").to_string();
        return Err(if msg.is_empty() {
            "Le sidecar RDP s'est arrêté sans se connecter.".into()
        } else {
            msg
        });
    };
    let mut it = line.split_whitespace();
    let port: u16 = it
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or("port WebSocket illisible")?;
    let token = it.next().ok_or("jeton WebSocket manquant")?.to_string();

    // Garde l'enfant pour pouvoir le tuer à la fermeture de l'onglet.
    if let Some(mut old) = state.inner.lock().unwrap().insert(id, child) {
        let _ = old.start_kill();
    }
    Ok(RdpConn { port, token })
}

#[tauri::command]
pub fn rdp_close(state: tauri::State<'_, RdpStore>, id: u64) -> Result<(), String> {
    if let Some(mut child) = state.inner.lock().unwrap().remove(&id) {
        let _ = child.start_kill();
    }
    Ok(())
}
