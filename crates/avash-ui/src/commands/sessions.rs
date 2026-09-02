//! Sessions de terminal : magasin, cible, connexion, relais de sortie, commandes PTY, hôtes et exécution ponctuelle.

use super::enregistreur_de;
use avash::ssh::AvashSession;
use avash::{parse_ssh_config, sftp::SftpHandle, SshHost};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc::Sender;

/// Numero unique par session ouverte, pour distinguer deux sessions qui
/// partagent le meme id d'onglet (le front renumerote a chaque rechargement
/// de fenetre). Sert a ne pas emettre `pty-closed` depuis une session evincee.
static SESSION_EPOCH: AtomicU64 = AtomicU64::new(1);

/// Message d'annulation volontaire : le front le reconnaît pour ne pas
/// présenter une fermeture d'onglet comme un échec de connexion.
pub const CONNEXION_ANNULEE: &str = "[AVASH_ANNULE]";

pub struct SessionStore {
    pub inner: Mutex<HashMap<u64, SessionHandle>>,
    /// Onglets fermés AVANT que leur session ne soit enregistrée.
    ///
    /// Une connexion SSH (résolution, rebonds, authentification) peut durer
    /// plusieurs secondes. Fermer l'onglet pendant ce temps appelait `pty_close`
    /// sur un identifiant que le magasin ne connaissait pas encore : la
    /// connexion aboutissait ensuite dans le vide, restait ouverte jusqu'à
    /// l'arrêt de l'application, et `open_sessions` la listait toujours — un
    /// snippet « toutes les sessions » partait donc sur un serveur dont
    /// l'utilisateur avait fermé l'onglet.
    pub annules: Mutex<std::collections::HashSet<u64>>,
    /// Connexions réellement en cours d'établissement.
    ///
    /// `pty_close` notait une annulation dès qu'il ne trouvait rien à retirer —
    /// y compris quand il n'y avait jamais eu de connexion en vol. Fermer un
    /// onglet dont la connexion avait échoué laissait donc son identifiant dans
    /// `annules`, définitivement. Or le front renumérote ses onglets à partir
    /// de 1 à chaque rechargement de fenêtre : un onglet ultérieur héritait de
    /// cet identifiant, se connectait pour de bon, et se voyait répondre
    /// « connexion annulée » — figé, sans reconnexion possible. Le trou était
    /// simplement passé de l'autre côté.
    pub en_cours: Mutex<std::collections::HashSet<u64>>,
}

/// De quoi ouvrir le sous-système SFTP sur la connexion SSH de l'onglet.
///
/// Une fermeture plutôt que la session elle-même : le magasin n'a pas à
/// connaître le transport, et les tests construisent une poignée sans serveur.
pub type OuvreurSftp = std::sync::Arc<
    dyn Fn() -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<SftpHandle, String>> + Send>,
        > + Send
        + Sync,
>;

pub struct SessionHandle {
    /// Identite unique de cette session (voir `SESSION_EPOCH`).
    pub epoch: u64,
    /// Clavier du front → canal SSH
    pub input: Sender<Vec<u8>>,
    /// Resize du front → `window_change` SSH
    pub resize: Sender<(u32, u32)>,
    /// Sous-système SFTP ouvert à la demande, par onglet, sur un canal de la
    /// session du terminal — jamais une seconde connexion.
    pub sftp: Mutex<Option<std::sync::Arc<SftpHandle>>>,
    /// Ouvre ce canal SFTP sur la session vivante.
    pub ouvrir_sftp: OuvreurSftp,
    /// Libelle affiche : l'alias, ou `user@hote` pour une saisie directe.
    pub label: String,
    /// Enregistrement asciicast en cours, partagé avec le pump qui y écrit
    /// chaque sortie. `None` hors enregistrement.
    pub enregistreur: Enregistrement,
}

/// L'enregistreur d'un onglet, tenu par le pump et par les commandes.
pub type Enregistrement = std::sync::Arc<Mutex<Option<avash::enregistrement::Enregistreur>>>;

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
    /// ⚠️ En memoire vive uniquement, le temps de la connexion : la cible
    /// n'est pas conservee une fois la session etablie. Jamais ecrit sur
    /// disque, jamais renvoye au front, jamais journalise.
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
    pub(crate) fn from_alias(alias: &str) -> Result<Self, String> {
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
    pub(crate) fn manual(
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
    pub(crate) fn override_password(&mut self, typed: Option<String>) {
        if let Some(p) = typed.filter(|p| !p.is_empty()) {
            self.password = Some(p);
        }
    }

    pub(crate) fn auth(&self) -> avash::ssh::ClientAuth {
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

pub(crate) fn find_host(alias: &str) -> Result<SshHost, String> {
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
    pub(crate) carry: Vec<u8>,
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
pub(crate) fn is_superseded<R: tauri::Runtime>(app: &AppHandle<R>, sid: u64, epoch: u64) -> bool {
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
///
/// La session est tenue le temps de la sonde : une ouverture de panneau SFTP
/// demandée pendant ces quelques centaines de millisecondes attend son tour.
async fn probe_and_emit_os(app: &AppHandle, sid: u64, label: String, session: &SessionPartagee) {
    let probe = tokio::time::timeout(std::time::Duration::from_secs(4), async {
        session.lock().await.run(avash::osinfo::PROBE_COMMAND).await
    })
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

/// Annonce la fin d'une session et la retire du magasin.
///
/// L'entrée survivait jusqu'à ce que l'utilisateur ferme l'onglet : le sélecteur
/// « envoyer à toutes les sessions » proposait donc des serveurs déjà
/// déconnectés, et le nombre annoncé ne correspondait pas à la sélection.
///
/// Rien n'est fait si cet identifiant porte déjà une session plus récente : on
/// fermerait le nouvel onglet.
pub(crate) fn clore_session<R: tauri::Runtime>(app: &AppHandle<R>, sid: u64, epoch: u64) {
    use tauri::Manager as _;
    if is_superseded(app, sid, epoch) {
        return;
    }
    app.state::<SessionStore>()
        .inner
        .lock()
        .unwrap()
        .remove(&sid);
    let _ = app.emit("pty-closed", serde_json::json!({ "id": sid }));
}

/// Enregistre la session, ou signale qu'on l'a annulée entre-temps.
///
/// Le test d'annulation et l'insertion se font **sous le même verrou**.
/// Séparés, un `pty_close` pouvait se glisser entre les deux : il ne trouvait
/// rien à retirer, notait l'annulation, et l'insertion qui suivait laissait une
/// session SSH pleinement établie sans onglet — vivante jusqu'à l'arrêt de
/// l'application, et toujours listée par `open_sessions`, si bien qu'un snippet
/// « toutes les sessions » partait sur un serveur dont l'onglet était fermé.
/// `pty_close` prend les verrous dans le même ordre.
pub(crate) fn enregistrer_session(
    state: &tauri::State<'_, SessionStore>,
    id: u64,
    handle: SessionHandle,
) -> Result<(), String> {
    let evicted = {
        let mut inner = state.inner.lock().unwrap();
        state.en_cours.lock().unwrap().remove(&id);
        if state.annules.lock().unwrap().remove(&id) {
            return Err(CONNEXION_ANNULEE.to_owned());
        }
        inner.insert(id, handle)
    };
    // L'évincé est libéré hors du verrou : le lâcher ferme ses canaux et
    // termine son pump.
    drop(evicted);
    Ok(())
}

/// Établit la session SSH et son canal PTY. Extrait d'`open_on_target` pour que
/// tous ses chemins d'échec passent par un seul point de nettoyage.
async fn etablir(
    target: &Target,
    cols: u32,
    rows: u32,
) -> Result<(AvashSession, avash::ssh::PtyChannel), String> {
    let mut session =
        AvashSession::connect_via(&target.jumps, &target.addr, target.port, &target.auth())
            .await
            .map_err(|e| e.to_string())?;
    let pty = session
        .open_pty(cols, rows, "xterm-256color")
        .await
        .map_err(|e| e.to_string())?;
    Ok((session, pty))
}

/// La session SSH d'un onglet, partagée entre le pump du terminal, qui la
/// garde vivante et la ferme à la fin, et le panneau SFTP, qui y ouvre son
/// canal. Un verrou asynchrone : on ne le tient que pour ouvrir un canal ou
/// lancer la sonde d'OS, jamais pendant le relais des octets.
type SessionPartagee = std::sync::Arc<tokio::sync::Mutex<AvashSession>>;

/// Le canal SFTP de l'onglet s'ouvrira sur cette session-là.
fn ouvreur_sftp(session: &SessionPartagee) -> OuvreurSftp {
    let session = session.clone();
    std::sync::Arc::new(move || {
        let session = session.clone();
        Box::pin(async move {
            let mut garde = session.lock().await;
            SftpHandle::open_on(&mut garde)
                .await
                .map_err(|e| format!("{e:#}"))
        })
    })
}

/// Relaie la sortie du terminal vers le front, regroupée, et vers
/// l'enregistrement s'il y en a un.
///
/// Les blocs arrivant du canal SSH sont souvent minuscules — 1, 4, 38,
/// 101 octets — et chacun coûterait un message JSON, un aller-retour IPC
/// et une écriture xterm. On les regroupe donc sur une courte fenêtre :
/// le débit s'effondre en nombre de messages sans que la latence devienne
/// perceptible (`COALESCE_MS` reste sous la durée d'une image à 60 Hz).
async fn relayer_sortie(
    app2: &AppHandle,
    sid: u64,
    mut out_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    enregistreur: Enregistrement,
) {
    const COALESCE_MS: u64 = 8;
    const FLUSH_BYTES: usize = 16 * 1024;
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
        // L'enregistrement reçoit le texte tel qu'il arrive, avant le
        // regroupement : les temps du fichier sont ceux du serveur.
        if let Some(e) = enregistreur.lock().unwrap().as_mut() {
            let _ = e.sortie(&text);
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
            deadline =
                Some(tokio::time::Instant::now() + tokio::time::Duration::from_millis(COALESCE_MS));
        }
    }
    // Ne pas perdre ce qui restait au moment de la fermeture.
    if !buffer.is_empty() {
        let _ = app2.emit(
            "pty-output",
            serde_json::json!({ "id": sid, "data": buffer }),
        );
    }
}

/// Ouvre une session PTY et démarre le pump out → événements Tauri `pty-output`.
async fn open_on_target(
    app: AppHandle,
    state: &tauri::State<'_, SessionStore>,
    id: u64,
    target: Target,
    cols: u32,
    rows: u32,
) -> Result<String, String> {
    state.en_cours.lock().unwrap().insert(id);
    let (session, pty) = match etablir(&target, cols, rows).await {
        Ok(v) => v,
        Err(e) => {
            // Une sortie en erreur doit oublier l'annulation éventuelle : elle
            // restait sinon dans l'ensemble pour toujours, et comme le front
            // renumérote ses onglets à partir de 1 à chaque rechargement de
            // fenêtre, la session qui héritait de cet identifiant se connectait
            // puis se voyait répondre « annulée » — onglet figé sur
            // « connexion en cours », sans message ni reconnexion possible.
            let mut en_cours = state.en_cours.lock().unwrap();
            en_cours.remove(&id);
            state.annules.lock().unwrap().remove(&id);
            return Err(e);
        }
    };

    let input = pty.in_tx.clone();
    let resize = pty.resize_tx.clone();
    let out_rx = pty.out_rx;
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
    // L'onglet a-t-il été fermé pendant que l'on se connectait ? Si oui, on
    // n'enregistre rien : lâcher `input`/`resize` ferme les canaux, le pump
    // s'arrête et la session SSH se referme d'elle-même.
    let epoch = SESSION_EPOCH.fetch_add(1, Ordering::Relaxed);
    // La cible — mot de passe compris — n'est pas conservée : la session
    // établie suffit à tout ce qui suit, panneau SFTP inclus.
    drop(target);
    let session: SessionPartagee = std::sync::Arc::new(tokio::sync::Mutex::new(session));
    let enregistreur: Enregistrement = std::sync::Arc::new(Mutex::new(None));
    enregistrer_session(
        state,
        id,
        SessionHandle {
            epoch,
            input,
            resize,
            sftp: Mutex::new(None),
            ouvrir_sftp: ouvreur_sftp(&session),
            label: label.clone(),
            enregistreur: enregistreur.clone(),
        },
    )?;

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
        // La sonde d'OS tourne EN MÊME TEMPS que le relais, plus avant lui.
        // Elle ouvre un canal exec, lance un `cat /etc/os-release` distant et
        // attend sa sortie *et* son code de retour : deux à trois allers-retours
        // plus un fork distant. Placée en tête, rien ne s'affichait tant qu'elle
        // n'avait pas rendu la main — quelques centaines de millisecondes d'écran
        // noir sur un lien lointain, jusqu'aux quatre secondes du délai de garde
        // sur un hôte chargé. Rien n'était perdu (le canal tamponne), mais le
        // geste le plus fréquent de l'application paraissait lent.
        // La boucle ne touche pas à `session` : les deux emprunts cohabitent.
        tokio::join!(
            probe_and_emit_os(&app2, sid, label_for_event, &session),
            relayer_sortie(&app2, sid, out_rx, enregistreur)
        );
        // La session distante s'est terminee (exit, coupure, kill). On ne
        // l'annonce que si cet id ne porte pas deja une session plus recente
        // (voir `is_superseded`), sinon on fermerait le nouvel onglet.
        let _ = session.lock().await.disconnect().await;
        clore_session(&app2, sid, pump_epoch);
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
/// Le mot de passe ne sert qu'a la connexion et n'est pas conserve ensuite :
/// le panneau SFTP ouvre son canal sur la session etablie. Il n'est ni ecrit
/// sur disque, ni renvoye au front, ni journalise.
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
    if let Some(e) = enregistreur_de(&state, id) {
        if let Some(enr) = e.lock().unwrap().as_mut() {
            let _ = enr.redimension(cols, rows);
        }
    }
    Ok(())
}
