//! Intégration RDP côté Tauri : lance le sidecar `avash-rdp` (isolé de russh),
//! relaie le framebuffer vers le front (Channel) et lui transmet les entrées.
//!
//! Le sidecar parle un protocole binaire encadré sur stdio (voir son en-tête).

use base64::Engine as _;
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::ipc::Channel;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Child;
use tokio::sync::mpsc;

/// Sessions RDP vivantes, par id d'onglet.
#[derive(Default)]
pub struct RdpStore {
    pub inner: Mutex<HashMap<u64, RdpHandle>>,
}

pub struct RdpHandle {
    child: Child,
    /// File vers la tâche qui possède le stdin du sidecar.
    input_tx: mpsc::UnboundedSender<Vec<u8>>,
}

/// Message poussé au front. `data` est du RGBA encodé base64 (les tableaux
/// d'octets JSON seraient énormes et lents).
#[derive(Clone, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RdpMsg {
    Connected {
        w: u16,
        h: u16,
    },
    Frame {
        x: u16,
        y: u16,
        w: u16,
        h: u16,
        data: String,
    },
    Error {
        message: String,
    },
    Closed,
}

/// Chemin du binaire sidecar : variable d'env, sinon à côté de l'app, sinon
/// l'emplacement de dev.
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

async fn read_exact(r: &mut (impl AsyncReadExt + Unpin), n: usize) -> Option<Vec<u8>> {
    let mut b = vec![0u8; n];
    r.read_exact(&mut b).await.ok().map(|_| b)
}

/// Ouvre une session RDP : lance le sidecar et pompe son flux vers `channel`.
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
    channel: Channel<RdpMsg>,
) -> Result<(), String> {
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
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Lancement du sidecar RDP impossible : {e}"))?;

    let mut stdin = child.stdin.take().ok_or("stdin sidecar indisponible")?;
    let mut stdout = child.stdout.take().ok_or("stdout sidecar indisponible")?;
    let mut stderr = child.stderr.take().ok_or("stderr sidecar indisponible")?;

    // Le sidecar écrit ses diagnostics (échec d'auth, TLS, NLA…) sur stderr :
    // on les collecte pour les remonter au front si la connexion échoue.
    let stderr_buf = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    {
        let buf = stderr_buf.clone();
        tokio::spawn(async move {
            let mut s = String::new();
            let _ = stderr.read_to_string(&mut s).await;
            *buf.lock().unwrap() = s;
        });
    }

    // Tâche d'écriture : elle seule possède le stdin, alimentée par une file.
    let (input_tx, mut input_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    tokio::spawn(async move {
        while let Some(bytes) = input_rx.recv().await {
            if stdin.write_all(&bytes).await.is_err() || stdin.flush().await.is_err() {
                break;
            }
        }
    });

    // Remplace une éventuelle session sous cet id.
    if let Some(mut old) = state
        .inner
        .lock()
        .unwrap()
        .insert(id, RdpHandle { child, input_tx })
    {
        let _ = old.child.start_kill();
    }

    // Pompe stdout -> Channel, dans une tâche.
    let b64 = base64::engine::general_purpose::STANDARD;
    tokio::spawn(async move {
        let mut connected = false;
        loop {
            let Some(hdr) = read_exact(&mut stdout, 5).await else {
                break;
            };
            let kind = hdr[0];
            let len = u32::from_le_bytes([hdr[1], hdr[2], hdr[3], hdr[4]]) as usize;
            let Some(payload) = read_exact(&mut stdout, len).await else {
                break;
            };
            let msg = match kind {
                1 if payload.len() >= 4 => {
                    connected = true;
                    RdpMsg::Connected {
                        w: u16::from_le_bytes([payload[0], payload[1]]),
                        h: u16::from_le_bytes([payload[2], payload[3]]),
                    }
                }
                2 if payload.len() >= 8 => RdpMsg::Frame {
                    x: u16::from_le_bytes([payload[0], payload[1]]),
                    y: u16::from_le_bytes([payload[2], payload[3]]),
                    w: u16::from_le_bytes([payload[4], payload[5]]),
                    h: u16::from_le_bytes([payload[6], payload[7]]),
                    data: b64.encode(&payload[8..]),
                },
                3 => RdpMsg::Error {
                    message: String::from_utf8_lossy(&payload).into_owned(),
                },
                _ => continue,
            };
            if channel.send(msg).is_err() {
                break;
            }
        }
        // Fin du flux : si on n'a jamais reçu CONNECTED, la connexion a
        // échoué — on remonte le diagnostic du sidecar plutôt qu'une
        // fermeture muette.
        if !connected {
            // Laisse le lecteur de stderr finir.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let diag = stderr_buf.lock().unwrap().trim().to_string();
            let message = if diag.is_empty() {
                "Connexion RDP fermée avant établissement.".to_string()
            } else {
                // Dernière ligne = la cause la plus parlante.
                diag.lines().last().unwrap_or(&diag).to_string()
            };
            let _ = channel.send(RdpMsg::Error { message });
        }
        let _ = channel.send(RdpMsg::Closed);
    });

    Ok(())
}

/// Transmet une entrée au sidecar (encodage binaire attendu par lui).
#[tauri::command]
pub fn rdp_input(state: tauri::State<'_, RdpStore>, id: u64, bytes: Vec<u8>) -> Result<(), String> {
    let store = state.inner.lock().unwrap();
    let h = store
        .get(&id)
        .ok_or_else(|| format!("Session RDP {id} inconnue"))?;
    h.input_tx
        .send(bytes)
        .map_err(|_| "sidecar RDP fermé".to_string())
}

#[tauri::command]
pub async fn rdp_close(state: tauri::State<'_, RdpStore>, id: u64) -> Result<(), String> {
    if let Some(mut h) = state.inner.lock().unwrap().remove(&id) {
        let _ = h.child.start_kill();
    }
    Ok(())
}
