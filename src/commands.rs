//! Commandes Tauri d'Avash : hôtes, one-shot, sessions PTY, SFTP.

use avash::ssh::AvashSession;
use avash::{parse_ssh_config, sftp::SftpHandle, SshHost};
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc::Sender;

pub struct SessionStore {
    pub inner: Mutex<HashMap<u64, SessionHandle>>,
}

pub struct SessionHandle {
    /// Clavier du front → canal SSH
    pub input: Sender<Vec<u8>>,
    /// Resize du front → window_change SSH
    pub resize: Sender<(u32, u32)>,
    /// Session SFTP dédiée ouverte à la demande (lazy), par onglet.
    pub sftp: Mutex<Option<std::sync::Arc<SftpHandle>>>,
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
    let resize = pty.resize_tx.clone();
    let mut out_rx = pty.out_rx;
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
        .insert(id, SessionHandle { input, resize, sftp: Mutex::new(None), alias });
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

/// Redimensionne le PTY (resize fenêtre / onglet) — window_change SSH.
#[tauri::command]
pub async fn pty_resize(
    state: tauri::State<'_, SessionStore>,
    id: u64,
    cols: u32,
    rows: u32,
) -> Result<(), String> {
    let resize = {
        let store = state.inner.lock().unwrap();
        store.get(&id).map(|h| h.resize.clone())
    };
    if let Some(resize) = resize {
        let _ = resize.send((cols, rows)).await;
    }
    Ok(())
}

/// Ferme une session (fermeture d'onglet). Coupe aussi la session SFTP liée.
#[tauri::command]
pub async fn pty_close(state: tauri::State<'_, SessionStore>, id: u64) -> Result<(), String> {
    let handle = state.inner.lock().unwrap().remove(&id);
    if let Some(h) = handle {
        let sftp = h.sftp.into_inner().unwrap().take();
        if let Some(s) = sftp {
            // Fermeture explicite si on détient la dernière référence.
            if let Ok(owned) = std::sync::Arc::try_unwrap(s) {
                let _ = owned.close().await;
            }
        }
    }
    Ok(())
}

// ---------- SFTP ----------

/// Ouvre (ou réutilise) la session SFTP d'un onglet. Retourne un Arc partagé
/// avec le store — la garde de la session SSH vit dans le store.
async fn sftp_of(
    state: &tauri::State<'_, SessionStore>,
    id: u64,
) -> Result<std::sync::Arc<SftpHandle>, String> {
    // Rapide : déjà ouverte ?
    {
        let store = state.inner.lock().unwrap();
        if let Some(h) = store.get(&id) {
            if let Some(s) = h.sftp.lock().unwrap().as_ref() {
                return Ok(s.clone());
            }
        }
    }
    // Sinon : connexion dédiée puis stockage.
    let alias = {
        let store = state.inner.lock().unwrap();
        store
            .get(&id)
            .map(|h| h.alias.clone())
            .ok_or_else(|| format!("Session {id} inconnue"))?
    };
    let host = find_host(&alias)?;
    let addr = host.hostname.clone().unwrap_or_else(|| host.alias.clone());
    let auth = auth_for(&host);
    let session = AvashSession::connect(&addr, host.port.unwrap_or(22), &auth)
        .await
        .map_err(|e| e.to_string())?;
    let sftp = std::sync::Arc::new(SftpHandle::open(session).await.map_err(|e| e.to_string())?);

    let store = state.inner.lock().unwrap();
    if let Some(h) = store.get(&id) {
        *h.sftp.lock().unwrap() = Some(sftp.clone());
    }
    Ok(sftp)
}

/// Liste un répertoire distant via SFTP.
#[tauri::command]
pub async fn sftp_list(
    state: tauri::State<'_, SessionStore>,
    id: u64,
    path: String,
) -> Result<Vec<avash::sftp::SftpEntry>, String> {
    let sftp = sftp_of(&state, id).await?;
    sftp.list(&path).await.map_err(|e| e.to_string())
}

/// Télécharge un fichier distant → local (dossier Téléchargements par défaut).
#[tauri::command]
pub async fn sftp_download(
    state: tauri::State<'_, SessionStore>,
    id: u64,
    remote: String,
    local: Option<String>,
) -> Result<String, String> {
    let sftp = sftp_of(&state, id).await?;
    let local = local.unwrap_or_else(|| {
        avash::sftp::default_local_dir()
            .join(std::path::Path::new(&remote).file_name().unwrap_or_default())
            .to_string_lossy()
            .into_owned()
    });
    let n = sftp.download(&remote, std::path::Path::new(&local)).await.map_err(|e| e.to_string())?;
    Ok(format!("{local} ({n} octets)"))
}

/// Téléverse un fichier local → distant.
#[tauri::command]
pub async fn sftp_upload(
    state: tauri::State<'_, SessionStore>,
    id: u64,
    local: String,
    remote: String,
) -> Result<u64, String> {
    let sftp = sftp_of(&state, id).await?;
    sftp.upload(std::path::Path::new(&local), &remote)
        .await
        .map_err(|e| e.to_string())
}
