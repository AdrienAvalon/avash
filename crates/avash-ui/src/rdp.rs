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
    // Sous Windows l'exécutable porte une extension : sans elle, le fichier
    // posé à côté de l'application (avash-rdp.exe) n'était jamais trouvé et
    // toute connexion RDP échouait.
    let nom = format!("avash-rdp{}", std::env::consts::EXE_SUFFIX);
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // À côté de l'exe (installation / bundle / version portable).
            let side = dir.join(&nom);
            if side.exists() {
                return side;
            }
            // En dev : le sidecar est un projet séparé. Depuis target/release/,
            // remonter jusqu'à la racine du dépôt.
            if let Some(root) = dir.parent().and_then(std::path::Path::parent) {
                let devside = root.join("rdp-sidecar/target/release").join(&nom);
                if devside.exists() {
                    return devside;
                }
            }
        }
    }
    std::path::PathBuf::from("rdp-sidecar/target/release").join(nom)
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

    // Mot de passe transmis par stdin plutôt qu'en argument : évite sa fuite via
    // /proc/<pid>/cmdline, lisible par les autres utilisateurs locaux.
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt as _;
        stdin
            .write_all(format!("{password}\n").as_bytes())
            .await
            .map_err(|e| format!("Envoi du mot de passe au sidecar : {e}"))?;
    }

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

// ---------- Connexions RDP enregistrées ----------

use avash::rdphost::{self, RdpHost};

#[tauri::command]
pub fn rdp_hosts() -> Result<Vec<RdpHost>, String> {
    rdphost::load_hosts().map_err(|e| e.to_string())
}

/// Cree (`id` absent) ou modifie une connexion RDP enregistree.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub fn rdp_host_save(
    id: Option<String>,
    name: String,
    host: String,
    port: u16,
    user: String,
    width: u16,
    height: u16,
    folder: Option<String>,
) -> Result<RdpHost, String> {
    let mut h = RdpHost::new(&name, &host, port, &user, width, height);
    if let Some(id) = id.filter(|i| !i.is_empty()) {
        h.id = id;
    }
    h.folder = avash::folders::normalize(&folder.unwrap_or_default());
    rdphost::upsert_host_in(&rdphost::hosts_path(), h.clone()).map_err(|e| e.to_string())?;
    Ok(h)
}

/// Range un bureau RDP dans un dossier (déplacement).
#[tauri::command]
pub fn rdp_host_set_folder(id: String, folder: String) -> Result<(), String> {
    let mut all = rdphost::load_hosts().map_err(|e| e.to_string())?;
    let norm = avash::folders::normalize(&folder);
    let h = all
        .iter_mut()
        .find(|h| h.id == id)
        .ok_or("Bureau RDP introuvable.")?;
    h.folder.clone_from(&norm);
    rdphost::save_hosts_to(&rdphost::hosts_path(), &all).map_err(|e| e.to_string())?;
    if !norm.is_empty() {
        avash::folders::create(&norm).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Supprime une connexion enregistree et oublie son mot de passe.
#[tauri::command]
pub fn rdp_host_delete(id: String) -> Result<(), String> {
    // Retrouver l'hote pour oublier son mot de passe avant suppression.
    if let Ok(hosts) = rdphost::load_hosts() {
        if let Some(h) = hosts.iter().find(|h| h.id == id) {
            let _ = avash::secrets::forget(&rdphost::keyring_account(&h.user, &h.host, h.port));
        }
    }
    rdphost::remove_host_in(&rdphost::hosts_path(), &id).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn rdp_password_save(
    host: String,
    port: u16,
    user: String,
    password: String,
) -> Result<(), String> {
    let id = rdphost::keyring_account(&user, &host, port);
    avash::secrets::save(&id, &password).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
#[must_use]
pub fn rdp_password_load(host: String, port: u16, user: String) -> Option<String> {
    avash::secrets::load(&rdphost::keyring_account(&user, &host, port))
}

#[tauri::command]
pub fn rdp_password_forget(host: String, port: u16, user: String) -> Result<(), String> {
    let id = rdphost::keyring_account(&user, &host, port);
    avash::secrets::forget(&id).map_err(|e| format!("{e:#}"))
}
