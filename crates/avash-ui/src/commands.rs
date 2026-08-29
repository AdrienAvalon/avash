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
    /// Cible conservee pour rouvrir une session SFTP sans redemander les
    /// identifiants a l'utilisateur.
    pub target: Target,
}

/// Ou et comment se connecter.
///
/// Deux origines possibles : un alias de `~/.ssh/config`, ou une saisie
/// directe (adresse, utilisateur, mot de passe ou cle). Les deux chemins
/// produisent le meme Target, donc la suite du code ne les distingue pas.
#[derive(Clone)]
pub struct Target {
    pub addr: String,
    pub port: u16,
    pub user: String,
    pub key_path: Option<std::path::PathBuf>,
    /// ⚠️ En memoire vive uniquement, le temps de la session. Jamais ecrit
    /// sur disque, jamais renvoye au front, jamais journalise.
    pub password: Option<String>,
    /// Libelle affiche : l'alias, ou `user@hote` pour une saisie directe.
    pub label: String,
}

/// `Debug` ecrit a la main : un `derive` afficherait le mot de passe en clair
/// dans les traces, les messages de panique et les logs de test.
impl std::fmt::Debug for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Target")
            .field("addr", &self.addr)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("key_path", &self.key_path)
            .field("password", &self.password.as_ref().map(|_| "<masqué>"))
            .field("label", &self.label)
            .finish()
    }
}

impl Target {
    /// Resout un alias declare dans `~/.ssh/config`.
    fn from_alias(alias: &str) -> Result<Self, String> {
        let host = find_host(alias)?;
        let addr = host.hostname.clone().unwrap_or_else(|| host.alias.clone());
        let port = host.port.unwrap_or(22);
        let user = host.user.clone().unwrap_or_else(whoami::username);
        // Mot de passe deja memorise ? Le trousseau evite de le redemander.
        // Une absence n'est pas une erreur : l'interface fera la saisie.
        let password = avash::secrets::load(&avash::secrets::account_id(&user, &addr, port));
        Ok(Self {
            port,
            user,
            key_path: host.identity_file.as_ref().map(std::path::PathBuf::from),
            password,
            label: host.alias.clone(),
            addr,
        })
    }

    /// Connexion saisie a la main, sans passer par `~/.ssh/config`.
    fn manual(
        addr: String,
        port: Option<u16>,
        user: String,
        password: Option<String>,
        key_path: Option<String>,
    ) -> Result<Self, String> {
        let addr = addr.trim().to_string();
        if addr.is_empty() {
            return Err("L'adresse du serveur est vide.".into());
        }
        let user = user.trim().to_string();
        if user.is_empty() {
            return Err("Le nom d'utilisateur est vide.".into());
        }
        let key_path = key_path
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
            .map(std::path::PathBuf::from);
        // Une cle inexistante donnerait une erreur d'authentification obscure ;
        // autant le dire tout de suite et nommer le chemin fautif.
        if let Some(k) = &key_path {
            if !k.exists() {
                return Err(format!("Clé introuvable : {}", k.display()));
            }
        }
        let password = password.filter(|p| !p.is_empty());
        if password.is_none() && key_path.is_none() {
            return Err("Renseigne un mot de passe ou une clé privée.".into());
        }
        let port = port.unwrap_or(22);
        Ok(Self {
            label: format!("{user}@{addr}"),
            addr,
            port,
            user,
            key_path,
            password,
        })
    }

    fn auth(&self) -> avash::ssh::ClientAuth {
        avash::ssh::ClientAuth {
            user: self.user.clone(),
            key_path: self.key_path.clone(),
            password: self.password.clone(),
        }
    }
}

fn find_host(alias: &str) -> Result<SshHost, String> {
    parse_ssh_config()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|h| h.alias == alias)
        .ok_or_else(|| format!("Hôte introuvable : {alias}"))
}

/// Liste les hôtes de ~/.ssh/config.
#[tauri::command]
pub fn list_hosts() -> Result<Vec<SshHost>, String> {
    parse_ssh_config().map_err(|e| e.to_string())
}

/// Exécution one-shot (écho de test / commandes rapides).
#[tauri::command]
pub async fn run_command(alias: String, command: String) -> Result<String, String> {
    let target = Target::from_alias(&alias)?;
    let mut session = AvashSession::connect(&target.addr, target.port, &target.auth())
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
async fn open_on_target(
    app: AppHandle,
    state: &tauri::State<'_, SessionStore>,
    id: u64,
    target: Target,
    cols: u32,
    rows: u32,
) -> Result<String, String> {
    let mut session = AvashSession::connect(&target.addr, target.port, &target.auth())
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

    // ⚠️ ENREGISTRER AVANT DE LANCER LE PUMP.
    //
    // Des l'ouverture, le shell distant interroge le terminal (DA1, couleur de
    // fond, position du curseur) et attend les reponses avant d'afficher son
    // invite. xterm.js y repond, mais ses reponses passent par `pty_write`, qui
    // cherche la session dans ce store. Si le pump emettait avant l'insertion,
    // ces reponses tomberaient sur "Session inconnue" et seraient perdues : le
    // shell resterait bloque, et l'utilisateur verrait un terminal vide.
    //
    // Le front numerote par ailleurs ses onglets avec un compteur qui repart a
    // 1 a chaque rechargement de fenetre, alors que le backend garde ses
    // sessions : sans eviction, l'ancien pump continuerait d'emettre sous le
    // meme id. Lacher le SessionHandle ferme ses canaux et termine ce pump.
    let label = target.label.clone();
    let evicted = state.inner.lock().unwrap().insert(
        id,
        SessionHandle {
            input,
            resize,
            sftp: Mutex::new(None),
            target,
        },
    );
    if let Some(old) = evicted {
        drop(old);
    }

    // Pump out → event front ; la session vit dans le pump.
    //
    // Les blocs arrivant du canal SSH sont souvent minuscules — 1, 4, 38,
    // 101 octets — et chacun coûterait un message JSON, un aller-retour IPC
    // et une écriture xterm. On les regroupe donc sur une courte fenêtre :
    // le débit s'effondre en nombre de messages sans que la latence devienne
    // perceptible (COALESCE_MS reste sous la durée d'une image à 60 Hz).
    const COALESCE_MS: u64 = 8;
    const FLUSH_BYTES: usize = 16 * 1024;
    let app2 = app.clone();
    let _pump = tokio::spawn(async move {
        let mut decoder = Utf8Stream::default();
        let mut buffer = String::new();
        let mut deadline: Option<tokio::time::Instant> = None;

        loop {
            // Tant que le tampon attend, on borne l'attente a l'echeance :
            // sans cela un octet isole resterait bloque jusqu'au suivant.
            let recu = match deadline {
                Some(d) => match tokio::time::timeout_at(d, out_rx.recv()).await {
                    Ok(v) => v,
                    Err(_) => {
                        if !buffer.is_empty() {
                            let _ = app2.emit(
                                "pty-output",
                                serde_json::json!({ "id": sid, "data": buffer }),
                            );
                            buffer.clear();
                        }
                        deadline = None;
                        continue;
                    }
                },
                None => out_rx.recv().await,
            };

            let Some(bytes) = recu else { break };
            let text = decoder.push(&bytes);
            if text.is_empty() {
                continue; // sequence UTF-8 encore incomplete
            }
            buffer.push_str(&text);

            // Gros volume : inutile d'attendre, on ecoule tout de suite.
            if buffer.len() >= FLUSH_BYTES {
                let _ = app2.emit(
                    "pty-output",
                    serde_json::json!({ "id": sid, "data": buffer }),
                );
                buffer.clear();
                deadline = None;
            } else if deadline.is_none() {
                deadline = Some(
                    tokio::time::Instant::now() + tokio::time::Duration::from_millis(COALESCE_MS),
                );
            }
        }
        // Ne pas perdre ce qui restait au moment de la fermeture.
        if !buffer.is_empty() {
            let _ = app2.emit(
                "pty-output",
                serde_json::json!({ "id": sid, "data": buffer }),
            );
        }
        // La session distante s'est terminee (exit, coupure reseau, kill).
        // Sans cet evenement, l'onglet resterait muet et l'utilisateur ne
        // saurait pas s'il attend encore ou si tout est fini.
        let _ = session.disconnect().await;
        let _ = app2.emit("pty-closed", serde_json::json!({ "id": sid }));
    });

    Ok(label)
}

/// Ouvre une session sur un hote declare dans `~/.ssh/config`.
///
/// `password` sert au second essai : un hote sans `IdentityFile` n'a aucun
/// moyen de s'authentifier, l'interface redemande alors la saisie.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn pty_open(
    app: AppHandle,
    state: tauri::State<'_, SessionStore>,
    id: u64,
    alias: String,
    password: Option<String>,
    cols: u32,
    rows: u32,
) -> Result<String, String> {
    let mut target = Target::from_alias(&alias)?;
    target.password = password.filter(|p| !p.is_empty());
    open_on_target(app, &state, id, target, cols, rows).await
}

/// L'hote a-t-il de quoi s'authentifier sans demander de saisie ?
///
/// Permet a l'interface de reclamer le mot de passe AVANT de tenter une
/// connexion vouee a l'echec, plutot qu'apres. `from_alias` ayant deja
/// consulte le trousseau, un mot de passe memorise compte comme suffisant.
#[tauri::command]
pub fn host_needs_password(alias: String) -> Result<bool, String> {
    let t = Target::from_alias(&alias)?;
    Ok(t.key_path.is_none() && t.password.is_none())
}

/// Ouvre une session sur une adresse saisie a la main, sans `~/.ssh/config`.
///
/// Le mot de passe reste en memoire vive le temps de la session : il sert a
/// rouvrir un canal SFTP sur le meme serveur sans redemander la saisie.
/// Il n'est ni ecrit sur disque, ni renvoye au front, ni journalise.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn pty_open_manual(
    app: AppHandle,
    state: tauri::State<'_, SessionStore>,
    id: u64,
    addr: String,
    port: Option<u16>,
    user: String,
    password: Option<String>,
    key_path: Option<String>,
    cols: u32,
    rows: u32,
) -> Result<String, String> {
    let target = Target::manual(addr, port, user, password, key_path)?;
    open_on_target(app, &state, id, target, cols, rows).await
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
        let sftp = h.sftp.into_inner().unwrap();
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
    // On rejoue la meme cible : une connexion saisie a la main n'existe pas
    // dans ~/.ssh/config, on ne peut donc pas la retrouver par son alias.
    let target = {
        let store = state.inner.lock().unwrap();
        store
            .get(&id)
            .map(|h| h.target.clone())
            .ok_or_else(|| format!("Session {id} inconnue"))?
    };
    let session = AvashSession::connect(&target.addr, target.port, &target.auth())
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
    let n = sftp
        .download(&remote, std::path::Path::new(&local))
        .await
        .map_err(|e| e.to_string())?;
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
    fn target_depuis_alias_reprend_user_port_et_cle() {
        let _g = with_ssh_config(
            "Host prod\n  HostName 10.0.0.1\n  User deploy\n  Port 2222\n  IdentityFile /tmp/k\n",
        );
        let t = Target::from_alias("prod").unwrap();
        assert_eq!(t.addr, "10.0.0.1");
        assert_eq!(t.user, "deploy");
        assert_eq!(t.port, 2222);
        assert_eq!(t.key_path.as_deref(), Some(std::path::Path::new("/tmp/k")));
        assert!(t.password.is_none(), "aucun mot de passe depuis un alias");
        assert_eq!(t.label, "prod");
    }

    #[test]
    fn target_depuis_alias_retombe_sur_les_defauts() {
        let _g = with_ssh_config("Host simple\n  HostName 10.0.0.9\n");
        let t = Target::from_alias("simple").unwrap();
        assert_eq!(t.port, 22, "port par defaut");
        assert_eq!(t.user, whoami::username(), "utilisateur courant par defaut");
        assert!(t.key_path.is_none());
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
    // ---------- Target::manual ----------

    #[test]
    fn manual_accepte_adresse_user_et_mot_de_passe() {
        let t = Target::manual(
            "10.0.0.5".into(),
            Some(2222),
            "adrien".into(),
            Some("secret".into()),
            None,
        )
        .unwrap();
        assert_eq!(t.addr, "10.0.0.5");
        assert_eq!(t.port, 2222);
        assert_eq!(t.user, "adrien");
        assert_eq!(t.password.as_deref(), Some("secret"));
        assert_eq!(t.label, "adrien@10.0.0.5", "libelle affiche dans l'onglet");
    }

    #[test]
    fn manual_utilise_22_par_defaut() {
        let t = Target::manual("srv".into(), None, "u".into(), Some("p".into()), None).unwrap();
        assert_eq!(t.port, 22);
    }

    #[test]
    fn manual_rogne_les_espaces_de_saisie() {
        // Un copier-coller traine souvent une espace : elle casserait la
        // resolution DNS avec un message incomprehensible.
        let t = Target::manual(
            "  10.0.0.5  ".into(),
            None,
            " adrien ".into(),
            Some("p".into()),
            None,
        )
        .unwrap();
        assert_eq!(t.addr, "10.0.0.5");
        assert_eq!(t.user, "adrien");
    }

    #[test]
    fn manual_refuse_une_adresse_vide() {
        let e = Target::manual("   ".into(), None, "u".into(), Some("p".into()), None).unwrap_err();
        assert!(e.contains("adresse"), "{e}");
    }

    #[test]
    fn manual_refuse_un_utilisateur_vide() {
        let e = Target::manual("srv".into(), None, "".into(), Some("p".into()), None).unwrap_err();
        assert!(e.contains("utilisateur"), "{e}");
    }

    #[test]
    fn manual_exige_un_mot_de_passe_ou_une_cle() {
        // Sans l'un des deux, l'authentification echouerait cote serveur avec
        // un message opaque : autant le dire avant de tenter la connexion.
        let e = Target::manual("srv".into(), None, "u".into(), None, None).unwrap_err();
        assert!(e.contains("mot de passe") && e.contains("clé"), "{e}");
        // Une chaine vide vaut absence.
        let e = Target::manual(
            "srv".into(),
            None,
            "u".into(),
            Some(String::new()),
            Some(String::new()),
        )
        .unwrap_err();
        assert!(e.contains("mot de passe"), "{e}");
    }

    #[test]
    fn manual_signale_une_cle_introuvable() {
        let e = Target::manual(
            "srv".into(),
            None,
            "u".into(),
            None,
            Some("/chemin/qui/n/existe/pas".into()),
        )
        .unwrap_err();
        assert!(e.contains("introuvable"), "{e}");
        assert!(
            e.contains("/chemin/qui/n/existe/pas"),
            "le chemin fautif doit etre nomme : {e}"
        );
    }

    #[test]
    fn manual_accepte_une_cle_existante_sans_mot_de_passe() {
        let key = std::env::temp_dir().join(format!("avash-key-{}", std::process::id()));
        std::fs::write(&key, b"factice").unwrap();
        let t = Target::manual(
            "srv".into(),
            None,
            "u".into(),
            None,
            Some(key.to_string_lossy().into_owned()),
        )
        .unwrap();
        assert_eq!(t.key_path.as_deref(), Some(key.as_path()));
        assert!(t.password.is_none());
        let _ = std::fs::remove_file(&key);
    }
    #[test]
    fn debug_ne_divulgue_jamais_le_mot_de_passe() {
        let t = Target::manual(
            "srv".into(),
            None,
            "u".into(),
            Some("tres-secret".into()),
            None,
        )
        .unwrap();
        let rendu = format!("{t:?}");
        assert!(
            !rendu.contains("tres-secret"),
            "le mot de passe ne doit jamais apparaitre dans une trace : {rendu}"
        );
        assert!(rendu.contains("masqué"), "{rendu}");
    }
}

// ---------- Clés SSH ----------

/// Liste les clés de `~/.ssh` utilisables pour un déploiement.
#[tauri::command]
pub fn keys_list() -> Result<Vec<avash::keys::KeyEntry>, String> {
    avash::keys::list_keys().map_err(|e| format!("{e:#}"))
}

/// Génère une paire ed25519 dans `~/.ssh`.
#[tauri::command]
pub fn key_generate(
    name: String,
    comment: Option<String>,
) -> Result<avash::keys::KeyEntry, String> {
    let comment = comment.unwrap_or_else(|| {
        // Un commentaire par defaut identifie la machine d'origine dans
        // authorized_keys — utile quand on revoque des annees plus tard.
        format!(
            "{}@{}",
            whoami::username(),
            whoami::fallible::hostname().unwrap_or_else(|_| "avash".into())
        )
    });
    avash::keys::generate(&name, &comment).map_err(|e| format!("{e:#}"))
}

/// Installe une clé publique dans l'`authorized_keys` d'un serveur.
///
/// C'est l'équivalent de `ssh-copy-id` : on se connecte une dernière fois par
/// mot de passe, on ajoute la clé, et les connexions suivantes s'en passent.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn key_deploy(
    addr: String,
    port: Option<u16>,
    user: String,
    password: String,
    public_line: String,
) -> Result<String, String> {
    let cmd = avash::keys::deploy_command(&public_line).map_err(|e| format!("{e:#}"))?;
    // Le deploiement se fait forcement par mot de passe : si la cle etait
    // deja acceptee, l'operation n'aurait pas lieu d'etre.
    let target = Target::manual(addr, port, user, Some(password), None)?;
    let mut session = AvashSession::connect(&target.addr, target.port, &target.auth())
        .await
        .map_err(|e| format!("{e:#}"))?;
    let (stdout, code) = session.run(&cmd).await.map_err(|e| format!("{e:#}"))?;
    let _ = session.disconnect().await;
    if code != 0 {
        return Err(format!(
            "Le serveur a refusé l'installation (code {code}) : {}",
            stdout.trim()
        ));
    }
    avash::keys::interpret_deploy(&stdout)
        .map(|m| m.to_string())
        .map_err(|e| format!("{e:#}"))
}

/// Enregistre une connexion manuelle dans `~/.ssh/config`.
///
/// L'hôte devient alors utilisable avec `ssh`, `scp`, `rsync` — pas seulement
/// dans Avash. Le mot de passe n'est jamais écrit : ce fichier est en clair.
#[tauri::command]
pub fn host_save(
    alias: String,
    addr: String,
    port: Option<u16>,
    user: String,
    key_path: Option<String>,
) -> Result<SshHost, String> {
    let host = SshHost {
        alias: alias.trim().to_string(),
        hostname: Some(addr.trim().to_string()),
        user: Some(user.trim().to_string()).filter(|u| !u.is_empty()),
        port,
        identity_file: key_path
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty()),
        proxy_jump: None,
        tags: vec![],
    };
    avash::append_host(&host).map_err(|e| format!("{e:#}"))?;
    Ok(host)
}

// ---------- Mots de passe memorises ----------

/// Mémorise un mot de passe dans le trousseau du système.
///
/// Jamais dans `~/.ssh/config` : ce fichier est en clair. Le trousseau
/// (KWallet, GNOME Keyring, Credential Manager, Trousseau macOS) gère le
/// chiffrement, le déverrouillage et la révocation.
#[tauri::command]
pub fn password_save(
    addr: String,
    port: Option<u16>,
    user: String,
    password: String,
) -> Result<(), String> {
    let id = avash::secrets::account_id(user.trim(), addr.trim(), port.unwrap_or(22));
    avash::secrets::save(&id, &password).map_err(|e| format!("{e:#}"))
}

/// Oublie un mot de passe mémorisé.
#[tauri::command]
pub fn password_forget(addr: String, port: Option<u16>, user: String) -> Result<(), String> {
    let id = avash::secrets::account_id(user.trim(), addr.trim(), port.unwrap_or(22));
    avash::secrets::forget(&id).map_err(|e| format!("{e:#}"))
}

/// Un mot de passe est-il déjà mémorisé pour cet hôte ?
#[tauri::command]
pub fn password_known(addr: String, port: Option<u16>, user: String) -> bool {
    let id = avash::secrets::account_id(user.trim(), addr.trim(), port.unwrap_or(22));
    avash::secrets::load(&id).is_some()
}
