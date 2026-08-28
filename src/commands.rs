//! Commandes Tauri d'Avash : hôtes, one-shot, sessions PTY.

use avash::{parse_ssh_config, ssh::AvashSession, SshHost};
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc::{Sender, Receiver};

pub struct SessionStore {
    pub inner: Mutex<HashMap<u64, SessionHandle>>,
}

pub struct SessionHandle {
    /// Clavier du front → canal SSH
    pub input: Sender<Vec<u8>>,
    pub alias: String,
}

fn find_host(alias: &str) -> Result<SshHost, String> {
    parse_ssh_config()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|h| h.alias == alias)
        .ok_or_else(|| format!("Hôte introuvable : {alias}"))
}

fn auth_for(host: &SshHost) -> avash::ssh::ClientAuth {
    avash::ssh::ClientAuth {
        user: host.user.clone().unwrap_or_else(whoami::username),
        key_path: host.identity_file.as_ref().map(std::path::PathBuf::from),
        password: None,
    }
}

/// Liste les hôtes de ~/.ssh/config.
#[tauri::command]
pub fn list_hosts() -> Result<Vec<SshHost>, String> {
    parse_ssh_config().map_err(|e| e.to_string())
}

/// Exécution one-shot (écho de test / commandes rapides).
#[tauri::command]
pub async fn run_command(alias: String, command: String) -> Result<String, String> {
    let host = find_host(&alias)?;
    let addr = host.hostname.clone().unwrap_or_else(|| host.alias.clone());
    let auth = auth_for(&host);
    let mut session = AvashSession::connect(&addr, host.port.unwrap_or(22), &auth)
        .await
        .map_err(|e| e.to_string())?;
    let (stdout, code) = session.run(&command).await.map_err(|e| e.to_string())?;
    session.disconnect().await.map_err(|e| e.to_string())?;
    Ok(format!("{stdout}\n[exit {code}]"))
}

/// Ouvre une session PTY et démarre le pump out → événements Tauri `pty-output`.
#[tauri::command]
pub async fn pty_open(
    app: AppHandle,
    state: tauri::State<'_, SessionStore>,
    id: u64,
    alias: String,
    cols: u32,
    rows: u32,
) -> Result<(), String> {
    let host = find_host(&alias)?;
    let addr = host.hostname.clone().unwrap_or_else(|| host.alias.clone());
    let auth = auth_for(&host);
    let mut session = AvashSession::connect(&addr, host.port.unwrap_or(22), &auth)
        .await
        .map_err(|e| e.to_string())?;
    let pty = session
        .open_pty(cols, rows, "xterm-256color")
        .await
        .map_err(|e| e.to_string())?;

    let input = pty.in_tx.clone();
    let mut out_rx: Receiver<Vec<u8>> = pty.out_rx;
    let sid = id;

    // Pump out → event front ; la session vit dans le pump.
    let app2 = app.clone();
    let _pump = tokio::spawn(async move {
        loop {
            match out_rx.recv().await {
                Some(bytes) => {
                    let _ = app2.emit("pty-output", serde_json::json!({
                        "id": sid,
                        "data": String::from_utf8_lossy(&bytes),
                    }));
                }
                None => break,
            }
        }
        let _ = session.disconnect().await;
    });

    state
        .inner
        .lock()
        .unwrap()
        .insert(id, SessionHandle { input, alias });
    Ok(())
}

/// Écrit le clavier du front dans le canal PTY.
#[tauri::command]
pub async fn pty_write(
    state: tauri::State<'_, SessionStore>,
    id: u64,
    data: String,
) -> Result<(), String> {
    let input = {
        let store = state.inner.lock().unwrap();
        store.get(&id).map(|h| h.input.clone())
    };
    if let Some(input) = input {
        input
            .send(data.into_bytes())
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Redimensionne le PTY (resize fenêtre / onglet) — v0.3.
#[tauri::command]
pub async fn pty_resize(
    _state: tauri::State<'_, SessionStore>,
    _id: u64,
    _cols: u32,
    _rows: u32,
) -> Result<(), String> {
    Ok(())
}

/// Ferme une session (fermeture d'onglet).
#[tauri::command]
pub async fn pty_close(state: tauri::State<'_, SessionStore>, id: u64) -> Result<(), String> {
    state.inner.lock().unwrap().remove(&id);
    Ok(())
}