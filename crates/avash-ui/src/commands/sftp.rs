//! Panneau SFTP : canal sur la session de l'onglet, listing, transferts, dossiers.

use super::SessionStore;
use avash::sftp::SftpHandle;
use tauri::{AppHandle, Emitter};

/// Ouvre (ou réutilise) le canal SFTP d'un onglet, sur la session SSH du
/// terminal. Retourne un Arc partagé avec le store.
pub(crate) async fn sftp_of(
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
    // Sinon : un canal de plus sur la session du terminal, puis stockage. Pas
    // de seconde connexion, donc pas de seconde authentification ni de cible à
    // rejouer — le mot de passe n'a pas été gardé.
    let ouvrir = {
        let store = state.inner.lock().unwrap();
        store
            .get(&id)
            .map(|h| h.ouvrir_sftp.clone())
            .ok_or_else(|| format!("Session {id} inconnue"))?
    };
    let mut fresh = Some(ouvrir().await?);

    // Course : deux commandes SFTP concurrentes sur le meme onglet ont pu, le
    // temps de cette ouverture, en obtenir chacune un. On re-verifie et on
    // stocke atomiquement sous le verrou ; aucun `await` n'y a lieu (les gardes
    // de Mutex ne sont pas Send). Le canal perdant est ferme ensuite, hors du
    // verrou, sinon il resterait ouvert sur le serveur.
    let mut to_close: Option<SftpHandle> = None;
    let chosen: Option<std::sync::Arc<SftpHandle>> = {
        let store = state.inner.lock().unwrap();
        match store.get(&id) {
            None => None,
            Some(h) => {
                let mut slot = h.sftp.lock().unwrap();
                if let Some(existing) = slot.as_ref() {
                    // Un autre appel a gagne : on fermera notre connexion.
                    to_close = fresh.take();
                    Some(existing.clone())
                } else {
                    let arc = std::sync::Arc::new(fresh.take().unwrap());
                    *slot = Some(arc.clone());
                    Some(arc)
                }
            }
        }
    };
    // Handle en trop (course perdue) ou session disparue : on ferme proprement.
    if let Some(f) = to_close.or(fresh) {
        let _ = f.close().await;
    }
    chosen.ok_or_else(|| format!("Session {id} inconnue"))
}

/// Determine le chemin local d'un telechargement.
///
/// Si l'appelant n'impose rien, on derive le nom depuis le chemin distant.
/// Un chemin distant sans nom de fichier exploitable (`/`, `.`, `..`) ne doit
/// PAS retomber silencieusement sur le dossier de telechargement lui-meme :
/// l'ecriture echouerait ensuite avec une erreur obscure.
pub(crate) fn local_target(remote: &str, local: Option<String>) -> Result<String, String> {
    if let Some(l) = local {
        return Ok(l);
    }
    let name = std::path::Path::new(remote)
        .file_name()
        .ok_or_else(|| format!("Chemin distant sans nom de fichier : {remote}"))?;
    Ok(avash::sftp::default_local_dir()
        .join(name)
        .to_string_lossy()
        .into_owned())
}

/// Résout un chemin distant en absolu (`.` → home). Certains serveurs SFTP
/// refusent `read_dir(".")` : on canonicalise d'abord, et le front affiche
/// alors un vrai chemin dans sa barre plutôt qu'un `.` opaque.
#[tauri::command]
pub async fn sftp_realpath(
    state: tauri::State<'_, SessionStore>,
    id: u64,
    path: String,
) -> Result<String, String> {
    let sftp = sftp_of(&state, id).await?;
    Ok(sftp.realpath(&path).await)
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

/// Rapporte la progression d'un transfert au front, sans le noyer : au plus
/// un evenement toutes les 80 ms, plus le dernier.
fn progress_reporter(app: &AppHandle, id: u64, name: &str, kind: &str) -> impl FnMut(u64, u64) {
    let app = app.clone();
    let name = name.to_string();
    let kind = kind.to_string();
    let mut last: Option<std::time::Instant> = None;
    move |done, total| {
        let due = last.is_none_or(|t| t.elapsed() >= std::time::Duration::from_millis(80));
        if due || done == total {
            last = Some(std::time::Instant::now());
            let _ = app.emit(
                "sftp-progress",
                serde_json::json!({ "id": id, "name": name, "kind": kind, "done": done, "total": total }),
            );
        }
    }
}

/// Télécharge un fichier distant → local (dossier Téléchargements par défaut).
#[tauri::command]
pub async fn sftp_download(
    app: AppHandle,
    state: tauri::State<'_, SessionStore>,
    id: u64,
    remote: String,
    local: Option<String>,
) -> Result<String, String> {
    let sftp = sftp_of(&state, id).await?;
    let local = local_target(&remote, local)?;
    let name = std::path::Path::new(&remote)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let report = progress_reporter(&app, id, &name, "download");
    let n = sftp
        .download_with(&remote, std::path::Path::new(&local), report)
        .await
        .map_err(|e| e.to_string())?;
    Ok(format!("{local} ({n} octets)"))
}

/// Téléverse un fichier local dans un dossier distant, sous son propre nom.
/// Rend le chemin distant cree.
#[tauri::command]
pub async fn sftp_upload(
    app: AppHandle,
    state: tauri::State<'_, SessionStore>,
    id: u64,
    local: String,
    remote_dir: String,
) -> Result<String, String> {
    let local_path = std::path::Path::new(&local);
    if local_path.is_dir() {
        return Err(format!(
            "{} est un dossier : seuls les fichiers sont envoyés.",
            local_path.display()
        ));
    }
    let name = local_path
        .file_name()
        .ok_or_else(|| format!("Nom de fichier illisible : {local}"))?
        .to_string_lossy()
        .into_owned();
    let remote = remote_join(&remote_dir, &name);
    let sftp = sftp_of(&state, id).await?;
    let report = progress_reporter(&app, id, &name, "upload");
    sftp.upload_with(local_path, &remote, report)
        .await
        .map_err(|e| e.to_string())?;
    Ok(remote)
}

/// Concatene un dossier distant et un nom, comme le fait le front.
pub(crate) fn remote_join(dir: &str, name: &str) -> String {
    if dir.is_empty() || dir == "." {
        return name.to_string();
    }
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

#[tauri::command]
pub async fn sftp_mkdir(
    state: tauri::State<'_, SessionStore>,
    id: u64,
    path: String,
) -> Result<(), String> {
    let sftp = sftp_of(&state, id).await?;
    sftp.mkdir(&path).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn sftp_remove(
    state: tauri::State<'_, SessionStore>,
    id: u64,
    path: String,
    is_dir: bool,
) -> Result<(), String> {
    let sftp = sftp_of(&state, id).await?;
    sftp.remove(&path, is_dir).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn sftp_rename(
    state: tauri::State<'_, SessionStore>,
    id: u64,
    from: String,
    to: String,
) -> Result<(), String> {
    let sftp = sftp_of(&state, id).await?;
    sftp.rename(&from, &to).await.map_err(|e| e.to_string())
}
