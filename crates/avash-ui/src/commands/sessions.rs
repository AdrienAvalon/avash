//! Sessions de terminal : magasin, cible, connexion, relais de sortie, commandes PTY, hôtes et exécution ponctuelle.

use super::enregistreur_de;
use avash::secrets::Zeroizing;
use avash::ssh::AvashSession;
use avash::Verrou as _;
use avash::{parse_ssh_config, sftp::SftpHandle, SshHost};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc::Sender;

/// Numero unique par session ouverte, pour distinguer deux sessions qui
/// partagent le meme id d'onglet (le front renumerote a chaque rechargement
/// de fenetre). Sert a ne pas emettre `pty-closed` depuis une session evincee.
pub(crate) static SESSION_EPOCH: AtomicU64 = AtomicU64::new(1);

/// Message d'annulation volontaire : le front le reconnaît pour ne pas
/// présenter une fermeture d'onglet comme un échec de connexion.
pub const CONNEXION_ANNULEE: &str = "[AVASH_ANNULE]";

#[derive(Default)]
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
    /// Copies directes (scp chez la source) en cours, par onglet source.
    ///
    /// Une copie directe tient le verrou de la session toute sa durée (voir
    /// `executeur`) ; ouvrir le panneau SFTP pendant ce temps attendait sa fin,
    /// des minutes sous « Chargement… ». Trouvé par l'audit du 12 septembre
    /// 2026 (C-SIL-14) : `sftp_of` consulte ce compte et répond tout de suite.
    pub copies_directes: Mutex<HashMap<u64, usize>>,
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

/// Exécute une commande sur la session d'un onglet ; rend sortie et code. Le
/// second argument est le drapeau d'annulation de la copie directe (voir
/// `run_avec_agent`) : `None` quand la commande n'est pas interruptible.
pub type Executeur = std::sync::Arc<
    dyn Fn(
            String,
            Option<avash::sftp::Annulation>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(String, u32), String>> + Send>,
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
    /// Exécute une commande sur la session vivante, agent SSH redirigé.
    pub executer: Executeur,
    /// Libelle affiche : l'alias, ou `user@hote` pour une saisie directe.
    pub label: String,
    /// Où cette session est connectée, pour qu'un autre hôte puisse la
    /// joindre (copie directe) : adresse, port, utilisateur.
    pub cible: (String, u16, String),
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
    /// disque, jamais renvoye au front, jamais journalise. Effacé à la
    /// libération (audit de sécurité du 12 septembre 2026, C-secrets-2) : une
    /// `String` ordinaire laissait ses octets dans le tas, donc dans un vidage
    /// mémoire conservé par systemd-coredump.
    pub password: Option<Zeroizing<String>>,
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
        Self::depuis_alias(alias).map(|(t, _)| t)
    }

    /// Comme `from_alias`, et rend aussi la panne du trousseau s'il y en a eu
    /// une (contrat K1 de l'audit du 12 septembre 2026, C-SIL-8). Le mot de
    /// passe est alors absent, comme sans entrée, et l'interface le demandera ;
    /// mais l'appelant peut dire POURQUOI au lieu de redemander « sans raison ».
    pub(crate) fn depuis_alias(alias: &str) -> Result<(Self, Option<String>), String> {
        // `resoudre_hote_dans` (et non `find_host`) : la connexion doit appliquer
        // les valeurs par défaut d'un bloc à motif (`Host *`), comme `ssh <alias>`.
        // `find_host` reste brut pour le formulaire d'édition, qui ne doit pas
        // matérialiser les valeurs héritées dans le bloc littéral. La
        // configuration est lue UNE fois pour la cible et ses rebonds (contrat
        // K3, C-perf-5) : chaque maillon la relisait, `Include` compris.
        let conf = avash::configuration_resolue().map_err(|e| format!("{e:#}"))?;
        let host = avash::resoudre_hote_dans(&conf, alias)
            .ok_or_else(|| format!("Hôte introuvable : {alias}"))?;
        let (addr, port, user) = cible_de(&host);
        // Mot de passe deja memorise ? Le trousseau evite de le redemander.
        // Une absence n'est pas une erreur : l'interface fera la saisie.
        let (password, panne) =
            match avash::secrets::charger(&avash::secrets::account_id(&user, &addr, port)) {
                Ok(p) => (p, None),
                Err(e) => (None, Some(format!("{e:#}"))),
            };
        let key_path = host.identity_file.as_deref().map(avash::developper_tilde);
        let jumps = resolve_jumps(&conf, host.proxy_jump.as_deref(), key_path.as_ref());
        let t = Self {
            port,
            user,
            key_path,
            password,
            label: host.alias.clone(),
            addr,
            jumps,
        };
        Ok((t, panne))
    }

    /// L'identifiant de trousseau d'un alias (`user@hôte:port`), résolu comme
    /// `from_alias` mais SANS lire le trousseau : supprimer ou modifier un hôte
    /// n'a besoin que de la clé, et chaque lecture pouvait ouvrir la boîte de
    /// déverrouillage du portefeuille pour rien.
    pub(crate) fn identifiant(conf: &str, alias: &str) -> Option<String> {
        let host = avash::resoudre_hote_dans(conf, alias)?;
        let (addr, port, user) = cible_de(&host);
        Some(avash::secrets::account_id(&user, &addr, port))
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
            .map(|k| avash::developper_tilde(&k));
        // Une cle inexistante donnerait une erreur d'authentification obscure ;
        // autant le dire tout de suite et nommer le chemin fautif.
        if let Some(k) = &key_path {
            if !k.exists() {
                return Err(format!("Clé introuvable : {}", k.display()));
            }
        }
        let password = password.filter(|p| !p.is_empty()).map(Zeroizing::new);
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
            self.password = Some(Zeroizing::new(p));
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

/// Adresse, port et utilisateur effectifs d'un hôte résolu : le nom d'hôte
/// sinon l'alias, le port 22 par défaut, l'utilisateur courant faute de `User`.
/// Une seule définition pour la connexion et pour l'identifiant du trousseau :
/// deux lectures divergentes ont déjà cassé « mémoriser » (hôte sans `User`).
pub(crate) fn cible_de(h: &SshHost) -> (String, u16, String) {
    (
        h.hostname.clone().unwrap_or_else(|| h.alias.clone()),
        h.port.unwrap_or(22),
        h.user.clone().unwrap_or_else(avash::ssh::current_username),
    )
}

/// Resout une chaine `ProxyJump` en rebonds concrets.
///
/// Chaque maillon est soit un alias de `~/.ssh/config` (on reprend son
/// hostname/user/port/cle), soit une saisie `user@host:port`. Faute de cle
/// propre, un rebond reutilise la cle de la cible (cas courant : meme cle sur
/// le bastion et le serveur). Les rebonds n'ont pas de mot de passe : ils
/// s'appuient sur une cle (agent a venir). `conf` est la configuration déjà
/// lue par l'appelant (contrat K3).
fn resolve_jumps(
    conf: &str,
    proxy_jump: Option<&str>,
    fallback_key: Option<&std::path::PathBuf>,
) -> Vec<avash::ssh::Hop> {
    let Some(spec) = proxy_jump else {
        return Vec::new();
    };
    avash::split_proxy_jump(spec)
        .into_iter()
        .map(|hop| {
            // Un maillon sans user ni port explicite peut être un alias. On le
            // résout comme la cible (`resoudre_hote_dans`) pour qu'un bastion
            // sans `User` propre hérite de celui du `Host *`, comme `ssh`.
            let alias = if hop.user.is_none() && hop.port.is_none() {
                avash::resoudre_hote_dans(conf, &hop.host)
            } else {
                None
            };
            let (addr, port, user, key_path) = match alias {
                Some(h) => {
                    let (addr, port, user) = cible_de(&h);
                    let cle = h
                        .identity_file
                        .as_deref()
                        .map(avash::developper_tilde)
                        .or_else(|| fallback_key.cloned());
                    (addr, port, user, cle)
                }
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
///
/// Hors du fil principal (audit du 12 septembre 2026, C-perf-10) : sur un
/// profil réseau, la lecture de la configuration et de ses `Include` gelait
/// la fenêtre au démarrage.
#[tauri::command(async)]
pub fn list_hosts() -> Result<Vec<SshHost>, String> {
    parse_ssh_config().map_err(|e| e.to_string())
}

/// Exécution one-shot (écho de test / commandes rapides).
#[tauri::command]
pub async fn run_command(alias: String, command: String) -> Result<String, String> {
    let target = super::bloquant(move || Target::from_alias(&alias)).await?;
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
    ///
    /// Trouvé par l'audit du 7 septembre 2026 : l'ancienne version ne sautait
    /// qu'UNE séquence invalide par appel et différait tout le reste du bloc
    /// dans `carry`. Sur un flux d'octets invalides (`cat` d'un binaire), `carry`
    /// gonflait sans borne — copie O(n) à chaque bloc, donc O(n²) — et l'onglet
    /// finissait par mourir ; le texte suivant un octet invalide n'apparaissait
    /// qu'au bloc d'après. On parcourt désormais tout le tampon, on remplace
    /// chaque séquence invalide par U+FFFD au fil de l'eau, et l'on ne conserve
    /// dans `carry` qu'une éventuelle séquence multi-octets TRONQUÉE en fin de
    /// bloc (au plus 3 octets), à recoller au bloc suivant.
    pub fn push(&mut self, chunk: &[u8]) -> String {
        // Chemin rapide (audit du 12 septembre 2026, C-perf-8) : sans reliquat
        // et sur un bloc valide, cas de presque tous les blocs, on décode
        // directement, sans recopier le bloc dans `carry`.
        if self.carry.is_empty() {
            if let Ok(s) = std::str::from_utf8(chunk) {
                return s.to_owned();
            }
        }
        self.carry.extend_from_slice(chunk);
        let mut out = String::new();
        let mut consumed = 0; // octets de `carry` déjà traités
        loop {
            let (valid, error_len) = match std::str::from_utf8(&self.carry[consumed..]) {
                Ok(s) => {
                    out.push_str(s);
                    self.carry.clear();
                    return out;
                }
                // `Utf8Error` ne retient pas le tampon : on en sort les indices,
                // ce qui libère l'emprunt avant de muter `carry`.
                Err(e) => (e.valid_up_to(), e.error_len()),
            };
            if let Ok(s) = std::str::from_utf8(&self.carry[consumed..consumed + valid]) {
                out.push_str(s);
            }
            let Some(len) = error_len else {
                // Séquence tronquée en toute fin : on ne garde qu'elle.
                self.carry.drain(..consumed + valid);
                return out;
            };
            // Octet(s) réellement invalide(s) au milieu du bloc : un U+FFFD, et
            // on poursuit le décodage du reste dans le même appel.
            out.push('\u{FFFD}');
            consumed += valid + len;
        }
    }
}

/// Cet id d'onglet porte-t-il desormais une session plus recente que `epoch` ?
///
/// Le front renumerote ses onglets a chaque rechargement de fenetre : un id
/// peut donc etre reattribue alors que l'ancienne session vit encore ; fermer
/// alors l'ancienne fermerait le nouvel onglet.
///
/// `clore_session` fait desormais ce test SOUS le meme verrou que le retrait
/// (l'audit du 7 septembre 2026 a montre qu'un test relache puis un retrait
/// repris laissaient une session s'inserer entre les deux). Cette fonction ne
/// sert plus qu'aux tests, qui verifient l'invariant d'eviction a part.
#[cfg(test)]
pub(crate) fn is_superseded<R: tauri::Runtime>(app: &AppHandle<R>, sid: u64, epoch: u64) -> bool {
    use tauri::Manager as _;
    app.state::<SessionStore>()
        .inner
        .verrou()
        .get(&sid)
        .is_some_and(|h| h.epoch != epoch)
}

/// Sonde le système distant et émet `host-os` (le front affiche son logo).
/// Un canal exec à part, borné dans le temps ; la sortie du PTY s'accumule
/// dans son propre canal pendant ce temps, rien n'est perdu.
///
/// La session est tenue le temps de la sonde : une ouverture de panneau SFTP
/// demandée pendant ces quelques centaines de millisecondes attend son tour.
///
/// Le délai est passé À `run_borne`, non posé par-dessus avec un `timeout`.
/// Trouvé par l'audit du 7 septembre 2026 : un `timeout` externe bornait
/// l'attente mais laissait le canal ouvert à l'échéance, et un serveur qui
/// débite lentement continuait d'inonder la session longue de l'onglet. En
/// bornant à l'intérieur, `run_borne` ferme le canal avant de rendre, ce qui
/// coupe le flux ; le verrou de session reste pris DANS la fonction bornée, il
/// est donc bien relâché à l'échéance.
async fn probe_and_emit_os<R: tauri::Runtime>(
    app: AppHandle<R>,
    sid: u64,
    label: String,
    session: SessionPartagee,
) {
    let probe = session
        .lock()
        .await
        .run_borne(
            avash::osinfo::PROBE_COMMAND,
            std::time::Duration::from_secs(4),
        )
        .await;
    match probe {
        Ok((out, _)) => {
            if let Some(os) = avash::osinfo::parse_probe_output(&out) {
                emettre(
                    &app,
                    "host-os",
                    serde_json::json!({ "id": sid, "label": label, "os": os }),
                );
            }
        }
        // Sans logo, rien ne dit pourquoi : le journal le garde (C-SIL-2).
        Err(e) => tracing::info!(
            onglet = sid,
            "sonde du système distant sans réponse : {e:#}"
        ),
    }
}

/// Émet un événement vers le front ; un refus est journalisé au lieu d'être
/// avalé (audit du 12 septembre 2026, C-SIL-2) : chaque `let _ = app.emit(..)`
/// perdait en silence une sortie de terminal ou une fermeture d'onglet.
pub(crate) fn emettre<R: tauri::Runtime, S: serde::Serialize + Clone>(
    app: &AppHandle<R>,
    evenement: &str,
    charge: S,
) {
    if let Err(e) = app.emit(evenement, charge) {
        tracing::warn!("événement « {evenement} » non émis : {e}");
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
///
/// Trouvé par l'audit du 7 septembre 2026 : le test d'époque et le retrait se
/// faisaient sous DEUX prises de verrou distinctes (`is_superseded` relâchait,
/// `remove` reprenait). Entre les deux, `enregistrer_session` pouvait insérer
/// une session plus récente sous le même id : `clore_session` la retirait alors
/// à sa place et émettait `pty-closed` pour elle, coupant une session que le
/// front croyait vivante (renumérotation après rechargement de la webview). Le
/// test d'époque et le retrait tiennent désormais le même verrou.
pub(crate) fn clore_session<R: tauri::Runtime>(app: &AppHandle<R>, sid: u64, epoch: u64) {
    clore_session_avec(app, sid, epoch, true);
}

/// `clore_session`, avec ou sans finalisation de l'enregistrement : pendant le
/// déroulement d'une panique (garde `FinDeSession`), écrire dans le fichier
/// pourrait paniquer une seconde fois, ce qui interrompt le processus entier.
fn clore_session_avec<R: tauri::Runtime>(
    app: &AppHandle<R>,
    sid: u64,
    epoch: u64,
    finaliser: bool,
) {
    use tauri::Manager as _;
    let store = app.state::<SessionStore>();
    let retire = {
        let mut inner = store.inner.verrou();
        // Cet id porte-t-il déjà une session plus récente ? Si oui, on la laisse.
        if inner.get(&sid).is_some_and(|h| h.epoch != epoch) {
            return;
        }
        inner.remove(&sid)
    };
    // La poignée retirée est finalisée puis lâchée HORS du verrou (comme
    // `enregistrer_session`) : son `Drop` ferme des canaux et pourrait
    // retoucher le magasin. On n'annonce `pty-closed` que si l'on a réellement
    // retiré quelque chose : sinon on fermerait un onglet qu'on n'a pas fermé.
    if let Some(h) = retire {
        if finaliser {
            finaliser_enregistrement(app, sid, &h.enregistreur);
        }
        emettre(app, "pty-closed", serde_json::json!({ "id": sid }));
    }
}

/// Garde de fin de session, posée en tête de la tâche du relais.
///
/// Trouvé par l'audit du 12 septembre 2026 (C-SIL-9) : tokio avale la panique
/// d'une tâche détachée, et `clore_session` n'était appelée qu'à la fin
/// NORMALE du relais. Une panique laissait la poignée dans le magasin : onglet
/// « connecté » sans session, listé par `open_sessions`, visé par un snippet
/// « toutes les sessions », chaque frappe rendant « channel closed ». Le
/// `Drop` s'exécute aussi pendant le déroulement de pile : la fermeture part
/// quel que soit le chemin de sortie.
struct FinDeSession<R: tauri::Runtime> {
    app: AppHandle<R>,
    sid: u64,
    epoch: u64,
    armee: bool,
}

impl<R: tauri::Runtime> Drop for FinDeSession<R> {
    fn drop(&mut self) {
        if self.armee {
            let panique = std::thread::panicking();
            if panique {
                tracing::error!(
                    onglet = self.sid,
                    "le relais de l'onglet a paniqué : onglet fermé"
                );
            }
            clore_session_avec(&self.app, self.sid, self.epoch, !panique);
        }
    }
}

/// Lance la tâche qui relaie la sortie d'une session vers le front, avec à
/// côté `a_cote` (la sonde d'OS en SSH), puis ferme l'onglet et enfin attend
/// `deconnexion`. La garde `FinDeSession` ferme l'onglet même si le relais
/// panique. Commun aux sessions SSH et série.
pub(crate) fn lancer_relais<R, A, D>(
    app: AppHandle<R>,
    sid: u64,
    epoch: u64,
    out_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    enregistreur: Enregistrement,
    a_cote: A,
    deconnexion: D,
) -> tokio::task::JoinHandle<()>
where
    R: tauri::Runtime,
    A: std::future::Future<Output = ()> + Send + 'static,
    D: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        let mut garde = FinDeSession {
            app: app.clone(),
            sid,
            epoch,
            armee: true,
        };
        tokio::join!(a_cote, relayer_sortie(&app, sid, out_rx, enregistreur));
        // Fin normale : `fermer_onglet_apres_pump` ferme l'onglet AVANT la
        // déconnexion ; la garde n'a plus rien à faire.
        garde.armee = false;
        fermer_onglet_apres_pump(&app, sid, epoch, deconnexion).await;
    })
}

/// Ferme l'onglet une fois le pump terminé : retrait du magasin et `pty-closed`
/// d'abord, envoi du paquet SSH `disconnect` ensuite.
///
/// Trouvé par l'audit du 7 septembre 2026 : dans l'ordre inverse, une copie
/// directe (scp) encore en cours retenait le verrou asynchrone de la session
/// (voir `executeur`), et `disconnect().await` — donc le retrait et
/// `pty-closed` — attendait la fin de la copie, potentiellement des minutes.
/// Shell mort, l'onglet restait pourtant listé « connecté » par `open_sessions`,
/// et chaque frappe rendait « channel closed » alors que le voyant disait
/// « live ». `clore_session` ne touche que le verrou synchrone du magasin : le
/// faire en premier ferme l'onglet tout de suite ; la copie va jusqu'à son
/// terme, puis le `disconnect` part.
pub(crate) async fn fermer_onglet_apres_pump<R: tauri::Runtime>(
    app: &AppHandle<R>,
    sid: u64,
    epoch: u64,
    deconnexion: impl std::future::Future<Output = ()>,
) {
    clore_session(app, sid, epoch);
    deconnexion.await;
}

/// Ferme proprement l'enregistrement d'un onglet qu'on quitte, et signale au
/// front si le fichier est resté incomplet.
///
/// Trouvé par l'audit du 7 septembre 2026 : fermer un onglet (fin de session
/// ou `pty_close`) se contentait de retirer la session ; l'enregistreur était
/// alors seulement *lâché*, et le `Drop` de son `BufWriter` avale l'erreur de
/// vidage. Le front promet pourtant « fermer l'onglet ferme le fichier » : on
/// appelle donc `arreter()` explicitement pour récupérer l'erreur éventuelle.
pub(crate) fn finaliser_enregistrement<R: tauri::Runtime>(
    app: &AppHandle<R>,
    sid: u64,
    enregistreur: &Enregistrement,
) {
    let pris = enregistreur.verrou().take();
    if let Some(e) = pris {
        let chemin = e.chemin().display().to_string();
        if let Err(err) = e.arreter() {
            signaler_echec_enregistrement(app, sid, chemin, format!("{err:#}"));
        }
    }
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
        let mut inner = state.inner.verrou();
        state.en_cours.verrou().remove(&id);
        if state.annules.verrou().remove(&id) {
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
    // `{e:#}` et non `to_string()` : un `anyhow::Error` n'affiche par `Display`
    // que son contexte externe. Or `connect_via` enrobe l'échec d'un rebond
    // d'un « Rebond hôte:port », ce qui enterre les marqueurs
    // `[AVASH_HOST_KEY_CHANGED]` / `[AVASH_PASSWORD_REQUIRED]` posés par la
    // couche SSH — l'interface (qui les repère par inclusion) ne proposait alors
    // ni d'oublier la clé changée ni de saisir un mot de passe dès qu'un rebond
    // était en jeu. Le format alterné déroule toute la chaîne, marqueur compris.
    // Trouvé par l'audit du 7 septembre 2026.
    let mut session =
        AvashSession::connect_via(&target.jumps, &target.addr, target.port, &target.auth())
            .await
            .map_err(|e| format!("{e:#}"))?;
    let pty = session
        .open_pty(cols, rows, "xterm-256color")
        .await
        .map_err(|e| format!("{e:#}"))?;
    Ok((session, pty))
}

/// La session SSH d'un onglet, partagée entre le pump du terminal, qui la
/// garde vivante et la ferme à la fin, et le panneau SFTP, qui y ouvre son
/// canal. Un verrou asynchrone : le relais des octets ne le tient jamais.
/// Ouvrir un canal ou lancer la sonde d'OS ne le tient qu'un instant ; une
/// commande à agent redirigé (copie directe) le tient en revanche toute sa
/// durée, ce qui sérialise ces commandes (voir `executeur`). La fermeture de
/// l'onglet, elle, ne passe jamais par ce verrou (voir
/// `fermer_onglet_apres_pump`).
pub(crate) type SessionPartagee = std::sync::Arc<tokio::sync::Mutex<AvashSession>>;

/// Le canal SFTP de l'onglet s'ouvrira sur cette session-là.
pub(crate) fn ouvreur_sftp(session: &SessionPartagee) -> OuvreurSftp {
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

/// Exécute une commande sur la session de l'onglet, avec l'agent SSH du
/// poste redirigé le temps de la commande (copie directe d'un hôte à un
/// autre).
///
/// Trouvé par l'audit du 7 septembre 2026 : le verrou de la session est tenu
/// pendant TOUTE la commande, pas seulement le temps d'ouvrir le canal (le
/// commentaire l'affirmait à tort). C'est voulu et load-bearing : `agent_redirige`
/// est un booléen partagé que la garde de `run_avec_agent` remet à faux en
/// sortant ; deux commandes concurrentes se couperaient l'agent l'une à l'autre.
/// Tenir le verrou les sérialise donc. En contrepartie une copie de plusieurs
/// minutes retient le verrou d'autant, ce qui ne doit PAS retarder la fermeture
/// de l'onglet — d'où `fermer_onglet_apres_pump`, qui retire l'onglet et émet
/// `pty-closed` sans passer par ce verrou.
pub(crate) fn executeur(session: &SessionPartagee) -> Executeur {
    let session = session.clone();
    std::sync::Arc::new(move |commande: String, annulation| {
        let session = session.clone();
        Box::pin(async move {
            let garde = session.lock().await;
            garde
                .run_avec_agent(&commande, annulation.as_ref())
                .await
                .map_err(|e| format!("{e:#}"))
        })
    })
}

/// Nombre de messages `pty-output` émis et non encore accusés par le front,
/// au-delà duquel le relais cesse de lire sa source (contrat K6).
const EN_VOL_MAX: u64 = 4;

/// Sans accusé pendant ce délai, le relais reprend quand même : un front qui
/// n'accuse jamais (ancienne version, écouteur perdu) retombe à un débit
/// plancher au lieu de geler l'onglet (contrat K6).
const FILET: std::time::Duration = std::time::Duration::from_millis(250);

/// Fenêtre de regroupement de la sortie : au plus un message par fenêtre.
const COALESCE: std::time::Duration = std::time::Duration::from_millis(8);

/// Au-delà de ce volume en attente, on émet sans attendre la fenêtre.
const FLUSH_BYTES: usize = 16 * 1024;

/// Les accusés de réception du front, par onglet (contrat K6 de l'audit du
/// 12 septembre 2026, C-front-4 et C-perf-4).
///
/// Un `emit` Tauri n'a ni accusé ni borne : sous `base64 /dev/urandom`, le
/// relais poussait des messages plus vite que xterm.js ne les écrivait, la
/// file de la webview gonflait, et Ctrl+C mettait des secondes à revenir.
#[derive(Default)]
pub struct Accuses {
    inner: Mutex<HashMap<u64, std::sync::Arc<Accuse>>>,
}

/// L'état d'accusé d'un relais : le plus grand `seq` accusé, et de quoi
/// réveiller le relais qui attend.
#[derive(Default)]
pub(crate) struct Accuse {
    dernier: AtomicU64,
    reveil: tokio::sync::Notify,
}

impl Accuse {
    fn dernier(&self) -> u64 {
        self.dernier.load(Ordering::Acquire)
    }
}

/// Le front a écrit le message `seq` de l'onglet `id` dans son terminal.
///
/// Synchrone à dessein : l'appel est fréquent, ne touche qu'à la mémoire, et
/// n'a pas à payer un saut de fil. Un accusé pour un onglet inconnu (fermé
/// entre-temps) est ignoré sans erreur.
#[tauri::command]
pub fn pty_ack(accuses: tauri::State<'_, Accuses>, id: u64, seq: u64) {
    let a = accuses.inner.verrou().get(&id).cloned();
    if let Some(a) = a {
        a.dernier.fetch_max(seq, Ordering::AcqRel);
        a.reveil.notify_one();
    }
}

/// Le message `pty-output` : sérialisé par emprunt, sans recopier le tampon
/// dans une `serde_json::Value` (audit du 12 septembre 2026, C-perf-8 : le
/// `json!` sur `&buffer` en faisait une copie de plus par octet).
#[derive(Clone, serde::Serialize)]
struct SortiePty<'a> {
    id: u64,
    data: &'a str,
    seq: u64,
}

/// Prévient le front qu'un enregistrement est condamné.
fn signaler_echec_enregistrement<R: tauri::Runtime>(
    app: &AppHandle<R>,
    sid: u64,
    chemin: String,
    erreur: String,
) {
    tracing::warn!(onglet = sid, "enregistrement interrompu : {erreur}");
    emettre(
        app,
        "enregistrement-erreur",
        serde_json::json!({ "id": sid, "chemin": chemin, "erreur": erreur }),
    );
}

/// Applique `op` à l'enregistreur de l'onglet s'il y en a un ; une écriture
/// refusée (disque plein) le retire et prévient le front, qui éteint le voyant.
///
/// Trouvé par l'audit du 7 septembre 2026 : l'erreur était avalée, le voyant
/// « rec » restait allumé et « Enregistrement terminé » mentait.
fn dans_l_enregistrement<R: tauri::Runtime>(
    app: &AppHandle<R>,
    sid: u64,
    enregistreur: &Enregistrement,
    op: impl FnOnce(&mut avash::enregistrement::Enregistreur) -> Result<(), String>,
) {
    let echec = {
        let mut slot = enregistreur.verrou();
        let mut echec = None;
        if let Some(e) = slot.as_mut() {
            if let Err(err) = op(e) {
                echec = Some((e.chemin().display().to_string(), err));
            }
        }
        if echec.is_some() {
            *slot = None;
        }
        echec
    };
    if let Some((chemin, erreur)) = echec {
        signaler_echec_enregistrement(app, sid, chemin, erreur);
    }
}

/// Relaie la sortie du terminal vers le front, regroupée, et vers
/// l'enregistrement s'il y en a un.
///
/// Les blocs arrivant du canal SSH sont souvent minuscules — 1, 4, 38,
/// 101 octets — et chacun coûterait un message JSON, un aller-retour IPC
/// et une écriture xterm. On les regroupe donc : au plus un message par
/// fenêtre de `COALESCE`, un gros volume partant sans attendre.
///
/// Regroupement en DÉBUT de fenêtre depuis l'audit du 12 septembre 2026
/// (C-perf-1) : le premier bloc d'une rafale attendait la fin de la fenêtre,
/// si bien que chaque écho de frappe payait 8 ms de plus que le réseau (10 ms
/// d'écho médian mesurés sur la boucle locale, dont 8 de fenêtre). Désormais un
/// bloc qui arrive après une fenêtre calme part aussitôt ; ce qui suit dans la
/// fenêtre attend son échéance et part en un seul message.
///
/// Contre-pression (contrat K6) : chaque message porte un `seq` croissant ;
/// au-delà de `EN_VOL_MAX` messages non accusés par `pty_ack`, le relais cesse
/// de lire sa source, ce qui remonte jusqu'au canal SSH, jusqu'à un accusé ou
/// jusqu'au `FILET`.
pub(crate) async fn relayer_sortie<R: tauri::Runtime>(
    app2: &AppHandle<R>,
    sid: u64,
    mut out_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    enregistreur: Enregistrement,
) {
    use tauri::Manager as _;
    // Sans magasin d'accusés (tests, moteur factice), pas de contre-pression.
    let accuse: Option<std::sync::Arc<Accuse>> = app2.try_state::<Accuses>().map(|a| {
        let neuf = std::sync::Arc::new(Accuse::default());
        a.inner.verrou().insert(sid, neuf.clone());
        neuf
    });
    let mut decoder = Utf8Stream::default();
    let mut buffer = String::new();
    let mut derniere: Option<tokio::time::Instant> = None;
    let mut deadline: Option<tokio::time::Instant> = None;
    let mut seq: u64 = 0;
    // Messages tenus pour accusés par le filet, faute d'accusé du front.
    let mut filet: u64 = 0;

    let emettre_tampon = |buffer: &mut String, seq: &mut u64| {
        *seq += 1;
        emettre(
            app2,
            "pty-output",
            SortiePty {
                id: sid,
                data: buffer,
                seq: *seq,
            },
        );
        buffer.clear();
        // Contrat K4 : l'enregistreur ne vide plus son tampon à chaque ligne ;
        // on le vide au rythme des messages du terminal.
        dans_l_enregistrement(app2, sid, &enregistreur, |e| {
            e.vider().map_err(|err| err.to_string())
        });
    };

    loop {
        if let Some(a) = &accuse {
            if seq.saturating_sub(a.dernier().max(filet)) >= EN_VOL_MAX {
                let fin_filet = tokio::time::Instant::now() + FILET;
                while seq.saturating_sub(a.dernier().max(filet)) >= EN_VOL_MAX {
                    if tokio::time::timeout_at(fin_filet, a.reveil.notified())
                        .await
                        .is_err()
                    {
                        // Passé le filet sans accusé : on absout les messages
                        // envoyés jusqu'ici pour ne pas rester bloqué. Remettre
                        // `filet` à 0 laissait `a.dernier().max(filet)` inchangé
                        // tant qu'aucun accusé n'arrivait jamais, donc la
                        // condition du `while` restait vraie pour toujours —
                        // une boucle active sans la moindre progression,
                        // trouvée le 12 septembre 2026 en tests (deux
                        // `#[tokio::test]` qui ne rendaient plus jamais la
                        // main).
                        filet = seq;
                    }
                }
            }
        }
        // Tant que le tampon attend, on borne l'attente a l'echeance :
        // sans cela un octet isole resterait bloque jusqu'au suivant.
        let recu = match deadline {
            Some(d) => {
                if let Ok(v) = tokio::time::timeout_at(d, out_rx.recv()).await {
                    v
                } else {
                    if !buffer.is_empty() {
                        emettre_tampon(&mut buffer, &mut seq);
                        derniere = Some(tokio::time::Instant::now());
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
        dans_l_enregistrement(app2, sid, &enregistreur, |e| {
            e.sortie(&text).map_err(|err| format!("{err:#}"))
        });
        buffer.push_str(&text);

        let maintenant = tokio::time::Instant::now();
        let fenetre_calme = derniere.is_none_or(|t| maintenant >= t + COALESCE);
        if buffer.len() >= FLUSH_BYTES || fenetre_calme {
            emettre_tampon(&mut buffer, &mut seq);
            derniere = Some(maintenant);
            deadline = None;
        } else if deadline.is_none() {
            deadline = derniere.map(|t| t + COALESCE);
        }
    }
    // Ne pas perdre ce qui restait au moment de la fermeture.
    if !buffer.is_empty() {
        emettre_tampon(&mut buffer, &mut seq);
    }
    if let (Some(a), Some(accuses)) = (&accuse, app2.try_state::<Accuses>()) {
        let mut inner = accuses.inner.verrou();
        if inner
            .get(&sid)
            .is_some_and(|x| std::sync::Arc::ptr_eq(x, a))
        {
            inner.remove(&sid);
        }
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
    state.en_cours.verrou().insert(id);
    let (session, pty) = match etablir(&target, cols, rows).await {
        Ok(v) => v,
        Err(e) => {
            // Une sortie en erreur doit oublier l'annulation éventuelle : elle
            // restait sinon dans l'ensemble pour toujours, et comme le front
            // renumérote ses onglets à partir de 1 à chaque rechargement de
            // fenêtre, la session qui héritait de cet identifiant se connectait
            // puis se voyait répondre « annulée » — onglet figé sur
            // « connexion en cours », sans message ni reconnexion possible.
            let mut en_cours = state.en_cours.verrou();
            en_cours.remove(&id);
            state.annules.verrou().remove(&id);
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
    // Adresse, port, utilisateur : ce qu'un autre hôte doit savoir pour
    // joindre celui-ci (copie directe), sans le mot de passe.
    let cible = (target.addr.clone(), target.port, target.user.clone());
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
            executer: executeur(&session),
            label: label.clone(),
            cible,
            enregistreur: enregistreur.clone(),
        },
    )?;

    // Pump out → event front ; la session vit dans le pump.
    //
    // La sonde d'OS tourne EN MÊME TEMPS que le relais, plus avant lui.
    // Elle ouvre un canal exec, lance un `cat /etc/os-release` distant et
    // attend sa sortie *et* son code de retour : deux à trois allers-retours
    // plus un fork distant. Placée en tête, rien ne s'affichait tant qu'elle
    // n'avait pas rendu la main — quelques centaines de millisecondes d'écran
    // noir sur un lien lointain, jusqu'aux quatre secondes du délai de garde
    // sur un hôte chargé. Rien n'était perdu (le canal tamponne), mais le
    // geste le plus fréquent de l'application paraissait lent.
    //
    // La session distante terminee (exit, coupure, kill), on retire l'onglet
    // et on emet `pty-closed` AVANT d'envoyer le `disconnect` SSH, qui peut
    // attendre une copie directe encore en cours (voir
    // `fermer_onglet_apres_pump`). Le garde d'epoque de `clore_session` evite
    // de fermer un onglet plus recent reattribue au meme id.
    let sonde = probe_and_emit_os(app.clone(), sid, label_for_event, session.clone());
    let deconnexion = async move {
        let _ = session.lock().await.disconnect().await;
    };
    let _pump = lancer_relais(app, sid, epoch, out_rx, enregistreur, sonde, deconnexion);

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
    // Le trousseau se lit hors des fils du runtime (C-SIL-7) ; une panne est
    // signalée une fois par lancement (contrat K1).
    let (mut target, panne) = super::bloquant(move || Target::depuis_alias(&alias)).await?;
    if let Some(m) = panne {
        super::signaler_trousseau_indisponible(&app, &m);
    }
    target.override_password(password);
    open_on_target(app, &state, id, target, cols, rows).await
}

/// L'hote a-t-il de quoi s'authentifier sans demander de saisie ?
///
/// Permet a l'interface de reclamer le mot de passe AVANT de tenter une
/// connexion vouee a l'echec, plutot qu'apres. `from_alias` ayant deja
/// consulte le trousseau, un mot de passe memorise compte comme suffisant.
#[tauri::command]
pub async fn host_needs_password<R: tauri::Runtime>(
    app: AppHandle<R>,
    alias: String,
) -> Result<bool, String> {
    let (t, panne) = super::bloquant(move || Target::depuis_alias(&alias)).await?;
    if let Some(m) = panne {
        super::signaler_trousseau_indisponible(&app, &m);
    }
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
        let store = state.inner.verrou();
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
pub async fn pty_resize<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, SessionStore>,
    id: u64,
    cols: u32,
    rows: u32,
) -> Result<(), String> {
    let resize = {
        let store = state.inner.verrou();
        store.get(&id).map(|h| h.resize.clone())
    };
    let resize = resize.ok_or_else(|| format!("Session {id} inconnue"))?;
    if let Err(e) = resize.send((cols, rows)).await {
        // Le relais est déjà parti : la fermeture de l'onglet suit.
        tracing::info!(onglet = id, "redimensionnement sans destinataire : {e}");
    }
    if let Some(e) = enregistreur_de(&state, id) {
        // Même traitement que le pump : un redimensionnement écrit lui aussi
        // dans l'enregistrement, une écriture refusée doit le condamner et être
        // signalée, sinon `pty_resize` restait le seul à continuer d'écrire
        // dans un enregistreur déjà mort. Trouvé par l'audit du 7 septembre 2026.
        dans_l_enregistrement(&app, id, &e, |enr| {
            enr.redimension(cols, rows)
                .map_err(|err| format!("{err:#}"))
        });
    }
    Ok(())
}
