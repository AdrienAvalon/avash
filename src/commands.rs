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


/// Decodeur UTF-8 incremental pour la sortie d'un PTY.
///
/// Le flux arrive par blocs arbitraires : un caractere multi-octets (accent,
/// caractere de tableau, emoji) peut tomber a cheval sur deux blocs.
/// `String::from_utf8_lossy` appliquee bloc par bloc le remplacerait par un
/// U+FFFD. On conserve donc la fin incomplete pour la recoller au bloc suivant.
#[derive(Default)]
pub struct Utf8Stream {
    carry: Vec<u8>,
}

impl Utf8Stream {
    /// Consomme un bloc et rend le texte decodable maintenant.
    pub fn push(&mut self, chunk: &[u8]) -> String {
        self.carry.extend_from_slice(chunk);
        match std::str::from_utf8(&self.carry) {
            Ok(s) => {
                let out = s.to_owned();
                self.carry.clear();
                out
            }
            Err(e) => {
                let valid = e.valid_up_to();
                // Sequence tronquee en fin de bloc : on la garde pour la suite.
                // Sequence reellement invalide : on ne bloque pas le terminal.
                let out = String::from_utf8_lossy(&self.carry[..valid]).into_owned();
                let rest = if e.error_len().is_some() {
                    // Octet invalide : on le saute pour ne pas coincer le flux.
                    self.carry[valid + e.error_len().unwrap_or(1)..].to_vec()
                } else {
                    self.carry[valid..].to_vec()
                };
                self.carry = rest;
                out
            }
        }
    }
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
        let mut decoder = Utf8Stream::default();
        loop {
            match out_rx.recv().await {
                Some(bytes) => {
                    let text = decoder.push(&bytes);
                    if text.is_empty() {
                        continue; // sequence encore incomplete
                    }
                    let _ = app2.emit("pty-output", serde_json::json!({
                        "id": sid,
                        "data": text,
                    }));
                }
                None => break,
            }
        }
        let _ = session.disconnect().await;
    });

    // Le front numerote ses onglets avec un compteur qui repart a 1 a chaque
    // rechargement de la fenetre, alors que le backend garde ses sessions.
    // Sans cette eviction, la session precedente resterait vivante et son pump
    // continuerait d'emettre des `pty-output` portant le meme id : la sortie
    // d'un ancien serveur apparaitrait dans le nouvel onglet.
    // Lacher le SessionHandle ferme ses canaux, ce qui termine l'ancien pump.
    let evicted = state
        .inner
        .lock()
        .unwrap()
        .insert(id, SessionHandle { input, resize, sftp: Mutex::new(None), alias });
    if let Some(old) = evicted {
        drop(old);
    }
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
    // Sans cette erreur, une frappe adressee a une session fermee etait perdue
    // et le front croyait l'avoir transmise.
    let input = input.ok_or_else(|| format!("Session {id} inconnue"))?;
    input
        .send(data.into_bytes())
        .await
        .map_err(|e| e.to_string())?;
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
    let resize = resize.ok_or_else(|| format!("Session {id} inconnue"))?;
    let _ = resize.send((cols, rows)).await;
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


/// Determine le chemin local d'un telechargement.
///
/// Si l'appelant n'impose rien, on derive le nom depuis le chemin distant.
/// Un chemin distant sans nom de fichier exploitable (`/`, `.`, `..`) ne doit
/// PAS retomber silencieusement sur le dossier de telechargement lui-meme :
/// l'ecriture echouerait ensuite avec une erreur obscure.
fn local_target(remote: &str, local: Option<String>) -> Result<String, String> {
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
    let local = local_target(&remote, local)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// HOME est global au processus : deux tests qui le modifient en parallele
    /// se marchent dessus. Ce verrou les serialise (les autres tests restent
    /// paralleles). Sans lui, find_host_* echoue une fois sur deux.
    static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Isole HOME pour ne pas dependre du ~/.ssh/config reel de la machine.
    /// Le HOME precedent est restaure a la destruction du garde.
    fn with_ssh_config(contents: &str) -> HomeGuard {
        let lock = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "avash-ui-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let ssh = dir.join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        std::fs::write(ssh.join("config"), contents).unwrap();
        let previous = std::env::var("HOME").ok();
        std::env::set_var("HOME", &dir);
        HomeGuard {
            previous,
            dir,
            _lock: lock,
        }
    }

    struct HomeGuard {
        previous: Option<String>,
        dir: std::path::PathBuf,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(h) => std::env::set_var("HOME", h),
                None => std::env::remove_var("HOME"),
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    // ---------- local_target ----------

    #[test]
    fn local_target_respecte_le_chemin_impose() {
        let got = local_target("/srv/rapport.md", Some("/tmp/ailleurs.md".into())).unwrap();
        assert_eq!(got, "/tmp/ailleurs.md");
    }

    #[test]
    fn local_target_derive_le_nom_du_fichier_distant() {
        let got = local_target("/srv/data/rapport.md", None).unwrap();
        assert!(
            got.ends_with("rapport.md"),
            "le nom distant doit etre conserve : {got}"
        );
    }

    #[test]
    fn local_target_ne_garde_que_le_dernier_segment() {
        // Un remote contenant ../ ne doit pas remonter dans l'arborescence locale.
        let got = local_target("/srv/../../etc/passwd", None).unwrap();
        assert!(got.ends_with("passwd"), "{got}");
        assert!(!got.contains(".."), "traversee de chemin : {got}");
    }

    #[test]
    fn local_target_refuse_un_chemin_sans_nom_de_fichier() {
        // Regression : file_name() renvoyait None, unwrap_or_default() donnait
        // une chaine vide et la destination devenait le dossier lui-meme.
        for remote in ["/", "..", "/srv/.."] {
            assert!(
                local_target(remote, None).is_err(),
                "{remote} devrait etre refuse"
            );
        }
    }

    // ---------- find_host / auth_for ----------

    #[test]
    fn find_host_trouve_un_alias_declare() {
        let _g = with_ssh_config("Host prod\n  HostName 10.0.0.1\n  User deploy\n  Port 2222\n");
        let h = find_host("prod").expect("alias prod doit etre trouve");
        assert_eq!(h.hostname.as_deref(), Some("10.0.0.1"));
        assert_eq!(h.user.as_deref(), Some("deploy"));
        assert_eq!(h.port, Some(2222));
    }

    #[test]
    fn find_host_signale_un_alias_inconnu() {
        let _g = with_ssh_config("Host prod\n  HostName 10.0.0.1\n");
        let err = find_host("absent").unwrap_err();
        assert!(err.contains("absent"), "message peu clair : {err}");
    }

    #[test]
    fn auth_for_utilise_le_user_declare() {
        let host = SshHost {
            alias: "prod".into(),
            hostname: Some("10.0.0.1".into()),
            user: Some("deploy".into()),
            port: None,
            identity_file: Some("/home/x/.ssh/id_ed25519".into()),
            proxy_jump: None,
            tags: vec![],
        };
        let auth = auth_for(&host);
        assert_eq!(auth.user, "deploy");
        assert_eq!(
            auth.key_path.as_deref(),
            Some(std::path::Path::new("/home/x/.ssh/id_ed25519"))
        );
        assert!(auth.password.is_none(), "aucun mot de passe ne doit etre pose ici");
    }

    #[test]
    fn auth_for_retombe_sur_l_utilisateur_courant() {
        let host = SshHost {
            alias: "prod".into(),
            hostname: Some("10.0.0.1".into()),
            user: None,
            port: None,
            identity_file: None,
            proxy_jump: None,
            tags: vec![],
        };
        let auth = auth_for(&host);
        assert_eq!(auth.user, whoami::username());
        assert!(auth.key_path.is_none());
    }
    // ---------- Utf8Stream ----------

    #[test]
    fn utf8_recolle_un_caractere_coupe_en_deux() {
        // "é" = 0xC3 0xA9 : on coupe entre les deux octets.
        let mut d = Utf8Stream::default();
        assert_eq!(d.push(&[0xC3]), "", "un octet seul n'est pas decodable");
        assert_eq!(d.push(&[0xA9]), "é", "le caractere doit etre recolle");
    }

    #[test]
    fn utf8_gere_une_coupure_au_milieu_d_un_emoji() {
        // 😈 = 4 octets, coupe apres le premier.
        let full = "😈".as_bytes().to_vec();
        let mut d = Utf8Stream::default();
        assert_eq!(d.push(&full[..1]), "");
        assert_eq!(d.push(&full[1..]), "😈");
    }

    #[test]
    fn utf8_texte_coupe_a_chaque_octet_est_restitue_intact() {
        let source = "Déjà vu — 100 % réussi 😈 ✓";
        let mut d = Utf8Stream::default();
        let mut out = String::new();
        for b in source.as_bytes() {
            out.push_str(&d.push(&[*b]));
        }
        assert_eq!(out, source, "le flux doit etre restitue a l'identique");
    }

    #[test]
    fn utf8_ne_bloque_pas_sur_un_octet_invalide() {
        // Un octet illegal ne doit pas figer le terminal : on le saute.
        let mut d = Utf8Stream::default();
        let out = d.push(&[b'a', 0xFF, b'b']);
        assert!(out.starts_with('a'), "{out:?}");
        let suite = d.push(b"c");
        assert!(
            format!("{out}{suite}").contains('c'),
            "le flux doit repartir apres l'octet invalide"
        );
    }

    #[test]
    fn utf8_ascii_passe_sans_latence() {
        let mut d = Utf8Stream::default();
        assert_eq!(d.push(b"ls -la\r\n"), "ls -la\r\n");
    }

}
