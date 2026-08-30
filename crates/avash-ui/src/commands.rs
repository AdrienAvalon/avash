//! Commandes Tauri d'Avash : hôtes, one-shot, sessions PTY, SFTP, tunnels.

use avash::snippet::Snippet;
use avash::ssh::AvashSession;
use avash::tunnel::{Tunnel, TunnelDef, TunnelKind, TunnelSnapshot};
use avash::{parse_ssh_config, sftp::SftpHandle, SshHost};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Numero unique par session ouverte, pour distinguer deux sessions qui
/// partagent le meme id d'onglet (le front renumerote a chaque rechargement
/// de fenetre). Sert a ne pas emettre `pty-closed` depuis une session evincee.
static SESSION_EPOCH: AtomicU64 = AtomicU64::new(1);
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc::Sender;

pub struct SessionStore {
    pub inner: Mutex<HashMap<u64, SessionHandle>>,
}

pub struct SessionHandle {
    /// Identite unique de cette session (voir `SESSION_EPOCH`).
    pub epoch: u64,
    /// Clavier du front → canal SSH
    pub input: Sender<Vec<u8>>,
    /// Resize du front → `window_change` SSH
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
    /// Chaine de rebonds (`ProxyJump`), resolue depuis la config. Vide = direct.
    pub jumps: Vec<avash::ssh::Hop>,
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
            .field("jumps", &self.jumps.len())
            .finish()
    }
}

impl Target {
    /// Resout un alias declare dans `~/.ssh/config`.
    fn from_alias(alias: &str) -> Result<Self, String> {
        let host = find_host(alias)?;
        let addr = host.hostname.clone().unwrap_or_else(|| host.alias.clone());
        let port = host.port.unwrap_or(22);
        let user = host
            .user
            .clone()
            .unwrap_or_else(avash::ssh::current_username);
        // Mot de passe deja memorise ? Le trousseau evite de le redemander.
        // Une absence n'est pas une erreur : l'interface fera la saisie.
        let password = avash::secrets::load(&avash::secrets::account_id(&user, &addr, port));
        let key_path = host.identity_file.as_ref().map(std::path::PathBuf::from);
        let jumps = resolve_jumps(host.proxy_jump.as_deref(), key_path.as_ref());
        Ok(Self {
            port,
            user,
            key_path,
            password,
            label: host.alias.clone(),
            addr,
            jumps,
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
            jumps: Vec::new(),
        })
    }

    /// Applique un mot de passe saisi, sans effacer celui du trousseau.
    ///
    /// Regression : `target.password = saisie` ecrasait le mot de passe
    /// memorise par `None` quand l'interface n'en envoyait pas (cas normal
    /// d'un hote deja connu). L'authentification echouait alors, et
    /// l'utilisateur devait retaper un mot de passe pourtant enregistre.
    fn override_password(&mut self, typed: Option<String>) {
        if let Some(p) = typed.filter(|p| !p.is_empty()) {
            self.password = Some(p);
        }
    }

    fn auth(&self) -> avash::ssh::ClientAuth {
        avash::ssh::ClientAuth {
            user: self.user.clone(),
            key_path: self.key_path.clone(),
            password: self.password.clone(),
        }
    }
}

/// Resout une chaine `ProxyJump` en rebonds concrets.
///
/// Chaque maillon est soit un alias de `~/.ssh/config` (on reprend son
/// hostname/user/port/cle), soit une saisie `user@host:port`. Faute de cle
/// propre, un rebond reutilise la cle de la cible (cas courant : meme cle sur
/// le bastion et le serveur). Les rebonds n'ont pas de mot de passe : ils
/// s'appuient sur une cle (agent a venir).
fn resolve_jumps(
    proxy_jump: Option<&str>,
    fallback_key: Option<&std::path::PathBuf>,
) -> Vec<avash::ssh::Hop> {
    let Some(spec) = proxy_jump else {
        return Vec::new();
    };
    let hosts = parse_ssh_config().unwrap_or_default();
    avash::split_proxy_jump(spec)
        .into_iter()
        .map(|hop| {
            // Un maillon sans user ni port explicite peut etre un alias.
            let alias = if hop.user.is_none() && hop.port.is_none() {
                hosts.iter().find(|h| h.alias == hop.host)
            } else {
                None
            };
            let (addr, port, user, key_path) = match alias {
                Some(h) => (
                    h.hostname.clone().unwrap_or_else(|| h.alias.clone()),
                    h.port.unwrap_or(22),
                    h.user.clone().unwrap_or_else(avash::ssh::current_username),
                    h.identity_file
                        .as_ref()
                        .map(std::path::PathBuf::from)
                        .or_else(|| fallback_key.cloned()),
                ),
                None => (
                    hop.host.clone(),
                    hop.port.unwrap_or(22),
                    hop.user
                        .clone()
                        .unwrap_or_else(avash::ssh::current_username),
                    fallback_key.cloned(),
                ),
            };
            avash::ssh::Hop {
                addr,
                port,
                auth: avash::ssh::ClientAuth {
                    user,
                    key_path,
                    password: None,
                },
            }
        })
        .collect()
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
    let mut session =
        AvashSession::connect_via(&target.jumps, &target.addr, target.port, &target.auth())
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

/// Cet id d'onglet porte-t-il desormais une session plus recente que `epoch` ?
///
/// Le front renumerote ses onglets a chaque rechargement de fenetre : un id
/// peut donc etre reattribue alors que l'ancienne session vit encore. Le pump
/// evince s'en sert pour ne pas emettre un `pty-closed` qui fermerait le
/// nouvel onglet.
fn is_superseded(app: &AppHandle, sid: u64, epoch: u64) -> bool {
    use tauri::Manager as _;
    app.state::<SessionStore>()
        .inner
        .lock()
        .unwrap()
        .get(&sid)
        .is_some_and(|h| h.epoch != epoch)
}

/// Sonde le système distant et émet `host-os` (le front affiche son logo).
/// Un canal exec à part, borné dans le temps ; la sortie du PTY s'accumule
/// dans son propre canal pendant ce temps, rien n'est perdu.
async fn probe_and_emit_os(app: &AppHandle, sid: u64, label: String, session: &mut AvashSession) {
    let probe = tokio::time::timeout(
        std::time::Duration::from_secs(4),
        session.run(avash::osinfo::PROBE_COMMAND),
    )
    .await;
    if let Ok(Ok((out, _))) = probe {
        if let Some(os) = avash::osinfo::parse_probe_output(&out) {
            let _ = app.emit(
                "host-os",
                serde_json::json!({ "id": sid, "label": label, "os": os }),
            );
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
    // Regroupement des sorties : voir plus bas, dans la boucle du pump.
    const COALESCE_MS: u64 = 8;
    const FLUSH_BYTES: usize = 16 * 1024;

    let mut session =
        AvashSession::connect_via(&target.jumps, &target.addr, target.port, &target.auth())
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
    let label_for_event = label.clone();
    let epoch = SESSION_EPOCH.fetch_add(1, Ordering::Relaxed);
    let evicted = state.inner.lock().unwrap().insert(
        id,
        SessionHandle {
            epoch,
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
    let app2 = app.clone();
    let pump_epoch = epoch;
    let _pump = tokio::spawn(async move {
        probe_and_emit_os(&app2, sid, label_for_event, &mut session).await;

        let mut decoder = Utf8Stream::default();
        let mut buffer = String::new();
        let mut deadline: Option<tokio::time::Instant> = None;

        loop {
            // Tant que le tampon attend, on borne l'attente a l'echeance :
            // sans cela un octet isole resterait bloque jusqu'au suivant.
            let recu = match deadline {
                Some(d) => {
                    if let Ok(v) = tokio::time::timeout_at(d, out_rx.recv()).await {
                        v
                    } else {
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
                }
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
        // La session distante s'est terminee (exit, coupure, kill). On ne
        // l'annonce que si cet id ne porte pas deja une session plus recente
        // (voir `is_superseded`), sinon on fermerait le nouvel onglet.
        let _ = session.disconnect().await;
        if !is_superseded(&app2, sid, pump_epoch) {
            let _ = app2.emit("pty-closed", serde_json::json!({ "id": sid }));
        }
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
    target.override_password(password);
    open_on_target(app, &state, id, target, cols, rows).await
}

/// L'hote a-t-il de quoi s'authentifier sans demander de saisie ?
///
/// Permet a l'interface de reclamer le mot de passe AVANT de tenter une
/// connexion vouee a l'echec, plutot qu'apres. `from_alias` ayant deja
/// consulte le trousseau, un mot de passe memorise compte comme suffisant.
#[tauri::command]
pub async fn host_needs_password(alias: String) -> Result<bool, String> {
    let t = Target::from_alias(&alias)?;
    // Une cle, un mot de passe memorise, ou un agent qui a des identites :
    // dans les trois cas, inutile de reclamer une saisie a l'avance.
    if t.key_path.is_some() || t.password.is_some() {
        return Ok(false);
    }
    Ok(!AvashSession::agent_has_identities().await)
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

/// Redimensionne le PTY (resize fenêtre / onglet) — `window_change` SSH.
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
        // into_inner() echoue si le mutex a ete empoisonne par un panic
        // ailleurs. Fermer un onglet ne doit jamais planter pour autant :
        // on recupere la valeur malgre l'empoisonnement.
        let sftp = h
            .sftp
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
    let session =
        AvashSession::connect_via(&target.jumps, &target.addr, target.port, &target.auth())
            .await
            .map_err(|e| e.to_string())?;
    let mut fresh = Some(SftpHandle::open(session).await.map_err(|e| e.to_string())?);

    // Course : deux commandes SFTP concurrentes sur le meme onglet ont pu, le
    // temps de cette connexion, en ouvrir chacune une. On re-verifie et on
    // stocke atomiquement sous le verrou ; aucun `await` n'y a lieu (les gardes
    // de Mutex ne sont pas Send). Le handle perdant est ferme ensuite, hors du
    // verrou, sinon la session SSH dupliquee fuirait.
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
fn remote_join(dir: &str, name: &str) -> String {
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

// ---------- Tunnels ----------

/// Tunnels ouverts, par identifiant de definition. Independants des onglets.
pub struct TunnelStore {
    pub inner: Mutex<HashMap<String, Tunnel>>,
}

/// Etat d'un tunnel ouvert, tel que l'interface l'affiche.
#[derive(serde::Serialize)]
pub struct TunnelStatus {
    pub id: String,
    #[serde(flatten)]
    pub snapshot: TunnelSnapshot,
}

fn parse_kind(kind: &str) -> Result<TunnelKind, String> {
    match kind {
        "local" => Ok(TunnelKind::Local),
        "remote" => Ok(TunnelKind::Remote),
        "dynamic" => Ok(TunnelKind::Dynamic),
        other => Err(format!("Type de tunnel inconnu : {other}")),
    }
}

/// Definitions enregistrees, tous hotes confondus.
#[tauri::command]
pub fn tunnel_defs() -> Result<Vec<TunnelDef>, String> {
    avash::tunnel::load_defs().map_err(|e| e.to_string())
}

/// Cree (`id` absent) ou modifie une definition.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub fn tunnel_def_save(
    id: Option<String>,
    alias: String,
    kind: String,
    bind_port: u16,
    target_host: Option<String>,
    target_port: Option<u16>,
    name: Option<String>,
) -> Result<TunnelDef, String> {
    let kind = parse_kind(&kind)?;
    // L'hote doit exister : un tunnel vers un alias fantome echouerait plus
    // tard avec un message moins clair.
    find_host(&alias)?;
    let mut def = TunnelDef::new(
        &alias,
        kind,
        bind_port,
        target_host.as_deref().unwrap_or(""),
        target_port.unwrap_or(0),
        name.as_deref().unwrap_or(""),
    );
    if let Some(id) = id.filter(|i| !i.is_empty()) {
        def.id = id;
    }
    avash::tunnel::upsert_def_in(&avash::tunnel::defs_path(), def.clone())
        .map_err(|e| e.to_string())?;
    Ok(def)
}

/// Supprime une definition ; ferme d'abord le tunnel s'il tourne.
#[tauri::command]
pub async fn tunnel_def_delete(
    tunnels: tauri::State<'_, TunnelStore>,
    id: String,
) -> Result<(), String> {
    let running = tunnels.inner.lock().unwrap().remove(&id);
    if let Some(t) = running {
        t.close().await;
    }
    avash::tunnel::remove_def_in(&avash::tunnel::defs_path(), &id).map_err(|e| e.to_string())?;
    Ok(())
}

/// Ouvre un tunnel. `password` suit la meme convention que `pty_open` : le
/// marqueur `PASSWORD_REQUIRED` dans l'erreur invite l'interface a le
/// demander puis a reessayer.
#[tauri::command]
pub async fn tunnel_start(
    tunnels: tauri::State<'_, TunnelStore>,
    id: String,
    password: Option<String>,
) -> Result<TunnelStatus, String> {
    let def = avash::tunnel::load_defs()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|d| d.id == id)
        .ok_or_else(|| format!("Tunnel inconnu : {id}"))?;
    let mut target = Target::from_alias(&def.alias)?;
    target.override_password(password);
    // Un tunnel deja ouvert (ou mort) sous cet id est remplace : c'est le
    // geste « relancer » de l'interface.
    let previous = tunnels.inner.lock().unwrap().remove(&id);
    if let Some(t) = previous {
        t.close().await;
    }
    // Par les rebonds, comme tous les autres chemins (open_on_target, run_command,
    // sftp_of) : un hôte en ProxyJump n'est pas joignable en direct, et l'erreur
    // renvoyée ne mentionnait même pas le bastion.
    let session =
        AvashSession::connect_via(&target.jumps, &target.addr, target.port, &target.auth())
            .await
            .map_err(|e| e.to_string())?;
    let tunnel = Tunnel::open(session, def)
        .await
        .map_err(|e| e.to_string())?;
    let snapshot = tunnel.snapshot();
    tunnels.inner.lock().unwrap().insert(id.clone(), tunnel);
    Ok(TunnelStatus { id, snapshot })
}

#[tauri::command]
pub async fn tunnel_stop(tunnels: tauri::State<'_, TunnelStore>, id: String) -> Result<(), String> {
    let t = tunnels.inner.lock().unwrap().remove(&id);
    if let Some(t) = t {
        t.close().await;
    }
    Ok(())
}

/// Etat de tous les tunnels ouverts (les morts inclus, marques `alive: false`,
/// jusqu'a ce que l'utilisateur les relance ou les arrete).
#[tauri::command]
#[must_use]
pub fn tunnel_status(tunnels: tauri::State<'_, TunnelStore>) -> Vec<TunnelStatus> {
    tunnels
        .inner
        .lock()
        .unwrap()
        .iter()
        .map(|(id, t)| TunnelStatus {
            id: id.clone(),
            snapshot: t.snapshot(),
        })
        .collect()
}

// ---------- Snippets ----------

/// Une session ouverte, telle que le sélecteur de cibles l'affiche.
#[derive(serde::Serialize)]
pub struct SessionInfo {
    pub id: u64,
    pub label: String,
}

/// Sessions actuellement ouvertes (pour choisir les cibles d'un envoi).
#[tauri::command]
#[must_use]
pub fn open_sessions(state: tauri::State<'_, SessionStore>) -> Vec<SessionInfo> {
    state
        .inner
        .lock()
        .unwrap()
        .iter()
        .map(|(id, h)| SessionInfo {
            id: *id,
            label: h.target.label.clone(),
        })
        .collect()
}

#[tauri::command]
pub fn snippet_list() -> Result<Vec<Snippet>, String> {
    avash::snippet::load_snippets().map_err(|e| e.to_string())
}

/// Variables `{{nom}}` d'une commande, dans l'ordre — pour demander leur
/// valeur avant l'envoi.
#[tauri::command]
#[must_use]
pub fn snippet_vars(command: String) -> Vec<String> {
    avash::snippet::extract_vars(&command)
}

/// Cree (`id` absent) ou modifie un snippet.
#[tauri::command]
pub fn snippet_save(
    id: Option<String>,
    name: String,
    command: String,
    run: bool,
    category: Option<String>,
) -> Result<Snippet, String> {
    let mut snip = Snippet::new(&name, &command, run, category.as_deref().unwrap_or(""));
    if let Some(id) = id.filter(|i| !i.is_empty()) {
        snip.id = id;
    }
    avash::snippet::upsert_snippet_in(&avash::snippet::snippets_path(), snip.clone())
        .map_err(|e| e.to_string())?;
    Ok(snip)
}

#[tauri::command]
pub fn snippet_delete(id: String) -> Result<(), String> {
    avash::snippet::remove_snippet_in(&avash::snippet::snippets_path(), &id)
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Envoie une commande (variables deja substituees cote front) a une ou
/// plusieurs sessions. Rend le nombre de sessions atteintes.
///
/// Une session fermee entre-temps est ignoree ; une erreur n'interrompt pas
/// l'envoi aux autres — la multi-execution ne doit pas s'arreter au premier
/// serveur muet.
#[tauri::command]
pub async fn snippet_send(
    state: tauri::State<'_, SessionStore>,
    session_ids: Vec<u64>,
    command: String,
    run: bool,
) -> Result<usize, String> {
    let payload = avash::snippet::terminal_payload(&command, run).into_bytes();
    let senders: Vec<_> = {
        let store = state.inner.lock().unwrap();
        session_ids
            .iter()
            .filter_map(|id| store.get(id).map(|h| h.input.clone()))
            .collect()
    };
    let mut sent = 0;
    for tx in senders {
        if tx.send(payload.clone()).await.is_ok() {
            sent += 1;
        }
    }
    Ok(sent)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target_with(password: Option<&str>) -> Target {
        Target {
            addr: "h".into(),
            port: 22,
            user: "u".into(),
            key_path: None,
            password: password.map(str::to_string),
            label: "h".into(),
            jumps: Vec::new(),
        }
    }

    #[test]
    fn override_password_garde_le_mot_de_passe_du_trousseau_sans_saisie() {
        let mut t = target_with(Some("du-trousseau"));
        t.override_password(None);
        assert_eq!(t.password.as_deref(), Some("du-trousseau"));
        t.override_password(Some(String::new()));
        assert_eq!(
            t.password.as_deref(),
            Some("du-trousseau"),
            "saisie vide = pas de saisie"
        );
    }

    #[test]
    fn override_password_prefere_la_saisie_quand_il_y_en_a_une() {
        let mut t = target_with(Some("ancien"));
        t.override_password(Some("nouveau".into()));
        assert_eq!(t.password.as_deref(), Some("nouveau"));
        let mut t = target_with(None);
        t.override_password(Some("saisi".into()));
        assert_eq!(t.password.as_deref(), Some("saisi"));
    }

    #[test]
    fn effective_user_retombe_sur_l_utilisateur_courant() {
        // Cle du trousseau coherente entre save et load : un hote sans `User`
        // doit resoudre le meme utilisateur des deux cotes (regression :
        // « mémoriser » etait casse pour ces hotes).
        assert_eq!(effective_user(Some("deploy".into())), "deploy");
        assert_eq!(effective_user(Some("  deploy ".into())), "deploy");
        assert_eq!(effective_user(None), avash::ssh::current_username());
        assert_eq!(
            effective_user(Some(String::new())),
            avash::ssh::current_username()
        );
    }

    #[test]
    fn remote_join_gere_racine_point_et_slash_final() {
        assert_eq!(remote_join("/srv", "a.txt"), "/srv/a.txt");
        assert_eq!(remote_join("/srv/", "a.txt"), "/srv/a.txt");
        assert_eq!(remote_join("/", "a.txt"), "/a.txt");
        assert_eq!(
            remote_join(".", "a.txt"),
            "a.txt",
            "cwd du login : chemin relatif"
        );
        assert_eq!(remote_join("", "a.txt"), "a.txt");
    }

    #[test]
    fn parse_kind_reconnait_les_trois_types_et_refuse_le_reste() {
        assert_eq!(parse_kind("local").unwrap(), TunnelKind::Local);
        assert_eq!(parse_kind("remote").unwrap(), TunnelKind::Remote);
        assert_eq!(parse_kind("dynamic").unwrap(), TunnelKind::Dynamic);
        assert!(parse_kind("socks").is_err());
    }

    /// HOME est global au processus : deux tests qui le modifient en parallele
    /// se marchent dessus. Ce verrou les serialise (les autres tests restent
    /// paralleles). Sans lui, `find_host`_* echoue une fois sur deux.
    static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Isole HOME pour ne pas dependre du ~/.ssh/config reel de la machine.
    /// Le HOME precedent est restaure a la destruction du garde.
    fn with_ssh_config(contents: &str) -> HomeGuard {
        let lock = HOME_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        assert_eq!(
            t.user,
            avash::ssh::current_username(),
            "utilisateur courant par defaut"
        );
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
        let e =
            Target::manual("srv".into(), None, String::new(), Some("p".into()), None).unwrap_err();
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

    #[test]
    fn open_external_refuse_les_schemas_dangereux() {
        // Un lien du terminal ne doit jamais ouvrir file://, javascript:, etc.
        for mauvais in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,<script>",
            "vbscript:x",
            "  file:///home",
        ] {
            assert!(
                open_external(mauvais.into()).is_err(),
                "devrait refuser : {mauvais}"
            );
        }
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
            avash::ssh::current_username(),
            whoami::hostname().unwrap_or_else(|_| "avash".into())
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
        .map(std::string::ToString::to_string)
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
    proxy_jump: Option<String>,
    tags: Option<String>,
) -> Result<SshHost, String> {
    let host = SshHost {
        alias: alias.trim().to_string(),
        hostname: Some(addr.trim().to_string()),
        user: Some(user.trim().to_string()).filter(|u| !u.is_empty()),
        port,
        identity_file: key_path
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty()),
        proxy_jump: proxy_jump
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty()),
        tags: tags
            .unwrap_or_default()
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect(),
        folder: String::new(),
    };
    avash::append_host(&host).map_err(|e| format!("{e:#}"))?;
    Ok(host)
}

// ---------- Mots de passe memorises ----------

/// Mémorise un mot de passe dans le trousseau du système.
///
/// Jamais dans `~/.ssh/config` : ce fichier est en clair. Le trousseau
/// (`KWallet`, GNOME Keyring, Credential Manager, Trousseau macOS) gère le
/// chiffrement, le déverrouillage et la révocation.
/// Utilisateur effectif d'un hote pour la cle du trousseau.
///
/// Doit correspondre EXACTEMENT a ce que `Target::from_alias` utilise pour
/// *relire* le mot de passe : un hote sans directive `User` retombe sur
/// l'utilisateur courant. Sans cette resolution commune, un mot de passe
/// enregistre sous une cle et relu sous une autre ne serait jamais retrouve
/// (bug : « mémoriser » cassé pour tout hote sans `User`).
fn effective_user(user: Option<String>) -> String {
    user.map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
        .unwrap_or_else(avash::ssh::current_username)
}

#[tauri::command]
pub fn password_save(
    addr: String,
    port: Option<u16>,
    user: Option<String>,
    password: String,
) -> Result<(), String> {
    let id = avash::secrets::account_id(&effective_user(user), addr.trim(), port.unwrap_or(22));
    avash::secrets::save(&id, &password).map_err(|e| format!("{e:#}"))
}

/// Oublie la clé d'hôte mémorisée (`known_hosts`) après un changement légitime.
/// Le prochain contact réapprend la nouvelle clé (TOFU).
#[tauri::command]
pub fn known_hosts_forget(addr: String, port: Option<u16>) -> Result<usize, String> {
    avash::ssh::forget_host_key(addr.trim(), port.unwrap_or(22)).map_err(|e| format!("{e:#}"))
}

/// Oublie un mot de passe mémorisé.
#[tauri::command]
pub fn password_forget(
    addr: String,
    port: Option<u16>,
    user: Option<String>,
) -> Result<(), String> {
    let id = avash::secrets::account_id(&effective_user(user), addr.trim(), port.unwrap_or(22));
    avash::secrets::forget(&id).map_err(|e| format!("{e:#}"))
}

/// Un mot de passe est-il déjà mémorisé pour cet hôte ?
#[tauri::command]
#[must_use]
pub fn password_known(addr: String, port: Option<u16>, user: Option<String>) -> bool {
    let id = avash::secrets::account_id(&effective_user(user), addr.trim(), port.unwrap_or(22));
    avash::secrets::load(&id).is_some()
}

/// Ouvre une URL dans le navigateur du système, jamais dans la webview.
///
/// Un lien cliquable du terminal ne doit pas naviguer dans la fenêtre Avash :
/// celle-ci a accès à `invoke`. On délègue au système, et on n'ouvre que des
/// schémas sûrs.
#[tauri::command]
pub fn open_external(url: String) -> Result<(), String> {
    let url = url.trim();
    // Whitelist stricte : ni file://, ni javascript:, ni schéma inconnu.
    let ok = ["http://", "https://", "mailto:", "ftp://"]
        .iter()
        .any(|p| url.starts_with(p));
    if !ok {
        return Err(format!("Schéma d'URL non autorisé : {url}"));
    }
    open::that(url).map_err(|e| format!("Ouverture impossible : {e}"))
}

/// Supprime un hôte de `~/.ssh/config` et oublie son mot de passe mémorisé.
#[tauri::command]
pub fn host_delete(alias: String) -> Result<(), String> {
    // On résout la cible AVANT de supprimer (après, l'hôte n'existe plus et on
    // ne saurait plus quel identifiant du trousseau oublier)... mais on n'oublie
    // le secret qu'APRÈS le succès de la suppression. Dans l'autre ordre, un
    // hôte déclaré via `Include` — que remove_host ne sait pas retirer — faisait
    // perdre le mot de passe alors que l'hôte restait en place.
    let identifiant = Target::from_alias(&alias)
        .ok()
        .map(|t| avash::secrets::account_id(&t.user, &t.addr, t.port));
    avash::remove_host(&alias).map_err(|e| format!("{e:#}"))?;
    if let Some(id) = identifiant {
        let _ = avash::secrets::forget(&id);
    }
    Ok(())
}

/// Renvoie les champs d'un hôte pour pré-remplir le formulaire d'édition.
#[tauri::command]
pub fn host_get(alias: String) -> Result<SshHost, String> {
    find_host(&alias)
}

/// Modifie un hôte enregistré. Si l'alias change, le mot de passe mémorisé
/// est déplacé vers le nouvel identifiant.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn host_update(
    old_alias: String,
    alias: String,
    addr: String,
    port: Option<u16>,
    user: Option<String>,
    key_path: Option<String>,
    proxy_jump: Option<String>,
    tags: Option<String>,
    folder: Option<String>,
) -> Result<(), String> {
    let host = SshHost {
        alias: alias.trim().to_string(),
        hostname: Some(addr.trim().to_string()).filter(|a| !a.is_empty()),
        user: user.map(|u| u.trim().to_string()).filter(|u| !u.is_empty()),
        port,
        identity_file: key_path
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty()),
        proxy_jump: proxy_jump
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty()),
        tags: tags
            .unwrap_or_default()
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect(),
        folder: avash::folders::normalize(&folder.unwrap_or_default()),
    };
    // Identifiant du trousseau AVANT modification : il dérive de user@addr:port,
    // pas de l'alias. Changer l'adresse ou l'utilisateur d'un hôte laissait donc
    // le secret sous l'ancien identifiant — redemandé à chaque connexion, sans
    // explication, l'ancienne entrée restant orpheline dans le trousseau.
    let ancien = Target::from_alias(old_alias.trim())
        .ok()
        .map(|t| avash::secrets::account_id(&t.user, &t.addr, t.port));

    avash::update_host(old_alias.trim(), &host).map_err(|e| format!("{e:#}"))?;

    // Après le succès seulement : on déplace le secret vers le nouvel identifiant.
    if let Some(ancien) = ancien {
        if let Some(nouveau) = Target::from_alias(host.alias.trim())
            .ok()
            .map(|t| avash::secrets::account_id(&t.user, &t.addr, t.port))
        {
            if nouveau != ancien {
                if let Some(secret) = avash::secrets::load(&ancien) {
                    let _ = avash::secrets::save(&nouveau, &secret);
                    let _ = avash::secrets::forget(&ancien);
                }
            }
        }
    }
    Ok(())
}

// ---------- Dossiers de rangement (arbre unifié SSH + RDP) ----------

/// Liste des dossiers connus (registre ; les dossiers dérivés des hôtes sont
/// ajoutés côté front).
#[tauri::command]
pub fn folders_list() -> Result<Vec<String>, String> {
    avash::folders::list().map_err(|e| format!("{e:#}"))
}

/// Crée un dossier (et ses ancêtres).
#[tauri::command]
pub fn folder_create(path: String) -> Result<Vec<String>, String> {
    avash::folders::create(&path).map_err(|e| format!("{e:#}"))
}

/// Supprime un dossier : ses hôtes (et ceux des sous-dossiers) reviennent à la
/// racine, puis le dossier et ses descendants sont retirés du registre.
#[tauri::command]
pub fn folder_delete(path: String) -> Result<Vec<String>, String> {
    avash::folders::delete_core(
        &avash::ssh_config_path(),
        &avash::rdphost::hosts_path(),
        &avash::folders::folders_path(),
        &path,
    )
    .map_err(|e| format!("{e:#}"))
}

/// Renomme un dossier et remappe ses hôtes (et sous-dossiers).
#[tauri::command]
pub fn folder_rename(from: String, to: String) -> Result<Vec<String>, String> {
    avash::folders::rename_core(
        &avash::ssh_config_path(),
        &avash::rdphost::hosts_path(),
        &avash::folders::folders_path(),
        &from,
        &to,
    )
    .map_err(|e| format!("{e:#}"))
}

/// Range un hôte SSH dans un dossier (déplacement). Le dossier cible est
/// enregistré s'il est nouveau.
#[tauri::command]
pub fn host_set_folder(alias: String, folder: String) -> Result<(), String> {
    let norm = avash::folders::normalize(&folder);
    avash::set_host_folder(alias.trim(), &norm).map_err(|e| format!("{e:#}"))?;
    if !norm.is_empty() {
        avash::folders::create(&norm).map_err(|e| format!("{e:#}"))?;
    }
    Ok(())
}

/// État des verrous clavier du poste : bit 1 = numérique, 2 = majuscules,
/// 4 = défilement. `None` quand le système ne sait pas le dire.
///
/// Le navigateur ne révèle ces verrous que sur un événement clavier. Or une
/// session RDP s'ouvre le plus souvent à la souris : sans interrogation du
/// système, le bureau distant démarrerait avec ses propres verrous, et le pavé
/// numérique paraîtrait éteint alors qu'il est allumé côté utilisateur.
#[tauri::command]
#[must_use]
pub fn keyboard_locks() -> Option<u8> {
    lock_bits()
}

#[cfg(target_os = "linux")]
fn lock_bits() -> Option<u8> {
    lock_bits_from_leds(std::path::Path::new("/sys/class/leds"))
}

/// Lit l'état des verrous dans une arborescence de diodes à la façon du noyau.
///
/// Tous les claviers n'exposent pas de diode (claviers virtuels, machines sans
/// témoin lumineux) : sans aucune diode reconnue on rend `None`, pour ne pas
/// imposer un état inventé au bureau distant.
#[cfg(target_os = "linux")]
fn lock_bits_from_leds(racine: &std::path::Path) -> Option<u8> {
    let mut bits = 0u8;
    let mut connu = false;
    for e in std::fs::read_dir(racine).ok()?.flatten() {
        let nom = e.file_name().to_string_lossy().to_lowercase();
        let bit = if nom.ends_with("::numlock") {
            1
        } else if nom.ends_with("::capslock") {
            2
        } else if nom.ends_with("::scrolllock") {
            4
        } else {
            continue;
        };
        if let Ok(v) = std::fs::read_to_string(e.path().join("brightness")) {
            connu = true;
            if v.trim() != "0" {
                bits |= bit;
            }
        }
    }
    connu.then_some(bits)
}

#[cfg(windows)]
fn lock_bits() -> Option<u8> {
    // Bit de poids faible de GetKeyState : état de bascule de la touche.
    unsafe extern "system" {
        fn GetKeyState(virtual_key: i32) -> i16;
    }
    const VK_CAPITAL: i32 = 0x14;
    const VK_NUMLOCK: i32 = 0x90;
    const VK_SCROLL: i32 = 0x91;
    let actif = |vk: i32| unsafe { GetKeyState(vk) } & 1 != 0;
    Some(
        u8::from(actif(VK_NUMLOCK))
            | (u8::from(actif(VK_CAPITAL)) << 1)
            | (u8::from(actif(VK_SCROLL)) << 2),
    )
}

#[cfg(not(any(target_os = "linux", windows)))]
fn lock_bits() -> Option<u8> {
    None // macOS : pas d'interface simple, on s'en remet aux événements clavier.
}

#[cfg(all(test, target_os = "linux"))]
mod tests_verrous {
    use super::lock_bits_from_leds;
    use std::path::{Path, PathBuf};

    /// Arborescence de diodes jetable, à la façon de /sys/class/leds.
    /// Le projet n'a pas de dépendance de test pour les répertoires temporaires :
    /// on suit la convention des autres tests et on nettoie à la libération.
    struct Leds(PathBuf);
    impl Leds {
        fn new(entrees: &[(&str, &str)]) -> Self {
            let base = std::env::temp_dir().join(format!(
                "avash-leds-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&base);
            for (nom, valeur) in entrees {
                let p = base.join(nom);
                std::fs::create_dir_all(&p).unwrap();
                std::fs::write(p.join("brightness"), valeur).unwrap();
            }
            std::fs::create_dir_all(&base).unwrap();
            Self(base)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Leds {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn aucune_diode_reconnue_ne_rend_rien() {
        // Mieux vaut ne rien affirmer que d'éteindre le pavé numérique du distant.
        let d = Leds::new(&[("input0::compose", "1")]);
        assert_eq!(lock_bits_from_leds(d.path()), None);
    }

    #[test]
    fn repertoire_absent_ne_rend_rien() {
        assert_eq!(lock_bits_from_leds(Path::new("/n/existe/pas")), None);
    }

    #[test]
    fn diodes_eteintes_donnent_zero() {
        let d = Leds::new(&[("input3::numlock", "0"), ("input3::capslock", "0")]);
        assert_eq!(lock_bits_from_leds(d.path()), Some(0));
    }

    #[test]
    fn chaque_verrou_a_son_bit() {
        assert_eq!(
            lock_bits_from_leds(Leds::new(&[("a::numlock", "1")]).path()),
            Some(1)
        );
        assert_eq!(
            lock_bits_from_leds(Leds::new(&[("a::capslock", "1")]).path()),
            Some(2)
        );
        assert_eq!(
            lock_bits_from_leds(Leds::new(&[("a::scrolllock", "1")]).path()),
            Some(4)
        );
    }

    #[test]
    fn les_verrous_se_combinent_et_plusieurs_claviers_s_agregent() {
        // Deux claviers branchés : l'état allumé de l'un suffit.
        let d = Leds::new(&[
            ("input3::numlock", "0"),
            ("input9::numlock", "1"),
            ("input9::scrolllock", "1"),
        ]);
        assert_eq!(lock_bits_from_leds(d.path()), Some(1 | 4));
    }
}
