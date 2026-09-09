//! Avash — moteur SSH v0.3 : sessions exécution + PTY interactif complet.
//! v0.1 : connect/auth/exec. v0.2 : `request_pty`.
//! v0.3 : write stdin réel, `window_change` (resize), `known_hosts` strict.

use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Marqueur place en tete du message quand seule l'absence de mot de passe
/// explique l'echec. L'interface le reconnait pour proposer une saisie.
/// Nom de l'utilisateur courant, avec repli.
///
/// Verdict d'une clé d'hôte présentée par un serveur, au regard de `known_hosts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerdictCle {
    /// Rien d'enregistré pour cet hôte : premier contact, la clé est à apprendre.
    PremierContact,
    /// La clé présentée figure parmi celles enregistrées.
    Connue,
    /// Des clés sont enregistrées pour cet hôte, mais aucune ne correspond.
    Changee { ligne: usize },
}

/// Compare la clé présentée à celles enregistrées pour cet hôte.
///
/// **L'algorithme n'entre volontairement pas en compte**, et c'est tout l'objet
/// de cette fonction. `check_known_hosts` de russh répond « hôte inconnu »
/// lorsque l'algorithme diffère :
///
/// ```text
/// match (pubkey.algorithm() == recorded.algorithm(), *pubkey == recorded) {
///     (true, true) => Ok(true), (true, false) => Err(KeyChanged), _ => Ok(false) }
/// ```
///
/// Un intercepteur n'avait donc qu'à annoncer un autre type de clé pour être
/// pris pour un premier contact, appris en silence, puis recevoir le mot de
/// passe. Ici, dès qu'une clé est enregistrée pour l'hôte, **toute clé
/// différente est un changement**, quel qu'en soit l'algorithme.
#[must_use]
pub fn juger_cle_hote(
    enregistrees: &[(usize, russh::keys::PublicKey)],
    presentee: &russh::keys::PublicKey,
) -> VerdictCle {
    let Some((premiere_ligne, _)) = enregistrees.first() else {
        return VerdictCle::PremierContact;
    };
    if enregistrees.iter().any(|(_, k)| k == presentee) {
        VerdictCle::Connue
    } else {
        VerdictCle::Changee {
            ligne: *premiere_ligne,
        }
    }
}

/// `~/.ssh/known_hosts` existe-t-il sans être lisible ?
///
/// russh renvoie une liste vide dès qu'il n'arrive pas à ouvrir le fichier
/// (droits retirés, remplacé par un répertoire, erreur d'E/S) — ce que le reste
/// du code prendrait pour « hôte inconnu », et qui ferait accepter n'importe
/// quelle clé. On préfère refuser.
fn known_hosts_illisible() -> Option<String> {
    let chemin = chemin_known_hosts()?; // pas de répertoire personnel : signalé plus loin
    verdict_known_hosts_illisible(&chemin)
}

/// Le verdict à opposer à un `known_hosts` présent mais inexploitable, en
/// nommant la cause : le geste de réparation n'est pas le même pour un fichier
/// aux droits retirés (`chmod`) et pour un tube nommé, un socket ou un
/// `known_hosts` pointé sur `/dev/null` (retirer, recréer). Réserve de la
/// relecture du 9 septembre 2026 : le message disait « n'est pas lisible »
/// dans les deux cas. `None` pour un fichier absent (premier contact) ou
/// lisible. Le `stat` seul décide du premier cas : rien n'est ouvert, donc
/// rien ne peut se bloquer.
fn verdict_known_hosts_illisible(chemin: &std::path::Path) -> Option<String> {
    let infos = std::fs::metadata(chemin).ok()?;
    if !infos.is_file() {
        return Some(
            "~/.ssh/known_hosts n'est pas un fichier ordinaire (tube nommé, socket, \
             périphérique ou répertoire) : impossible de vérifier l'identité du \
             serveur. Connexion refusée."
                .into(),
        );
    }
    if fichier_present_mais_illisible(chemin) {
        return Some(
            "~/.ssh/known_hosts existe mais n'est pas lisible (droits du fichier ?) : \
             impossible de vérifier l'identité du serveur. Connexion refusée."
                .into(),
        );
    }
    None
}

/// Le fichier `known_hosts`, résolu une seule fois pour tout le monde.
///
/// russh résout ce chemin de son côté avec `std::env::home_dir()`, qui sous
/// Windows consulte `USERPROFILE` là où `dirs::home_dir()` interroge le dossier
/// de profil du système. Les deux peuvent différer : nous inspections alors un
/// fichier pendant que russh en lisait un autre, et la vérification de clé
/// d'hôte ne vérifiait plus rien. On lui passe donc le chemin explicitement.
///
/// Public pour que les tests visent **le même fichier** que le code : y écrire
/// par la résolution implicite de russh revenait à préparer un décor que la
/// vérification ne regardait pas — le test passait alors pour de mauvaises
/// raisons sous Unix, et échouait sous Windows.
#[must_use]
pub fn chemin_known_hosts() -> Option<std::path::PathBuf> {
    crate::repertoire_personnel().map(|h| h.join(".ssh").join("known_hosts"))
}

/// `chemin` est-il un fichier ORDINAIRE, sur lequel une lecture rend la main ?
///
/// Trouvé par l'audit du 9 septembre 2026. Le code se défendait déjà du
/// `known_hosts` remplacé par un répertoire, dont la lecture échoue tout de
/// suite ; il ne voyait pas venir le fichier spécial BLOQUANT. Sous Unix,
/// ouvrir un tube nommé en lecture seule suspend l'appelant dans le noyau tant
/// qu'aucun écrivain ne se présente, ce qui n'arrive jamais tout seul. Or ces
/// lectures partent de `check_server_key`, un handler async : le fil du runtime
/// tokio qui menait la négociation SSH restait figé là, sans erreur, sans
/// délai, l'onglet de connexion bloqué pour toujours.
///
/// `metadata` (un `stat`) n'ouvre rien et ne peut donc pas se bloquer. Tout ce
/// qui n'est pas un fichier ordinaire est écarté sans être ouvert : tube,
/// socket, périphérique. Un `known_hosts` pointé sur `/dev/null` (la manière
/// des uns de désactiver la vérification d'hôte) tombe avec, et la connexion
/// est refusée plutôt que toute clé acceptée : pour un client SSH c'est le bon
/// sens du doute.
fn est_fichier_ordinaire(chemin: &std::path::Path) -> bool {
    std::fs::metadata(chemin).is_ok_and(|infos| infos.is_file())
}

/// L'hôte porte-t-il un marqueur OpenSSH que nous ne savons pas traiter ?
///
/// Rend `None` sur un `known_hosts` qui n'est pas un fichier ordinaire : c'est
/// alors à `known_hosts_illisible` de refuser la connexion, avec le verdict qui
/// nomme la cause. Lire ici valait, sur un tube nommé, un gel silencieux.
fn marqueur_bloquant(hote: &str) -> Option<String> {
    let chemin = chemin_known_hosts()?;
    if !est_fichier_ordinaire(&chemin) {
        return None;
    }
    let contenu = std::fs::read_to_string(chemin).ok()?;
    marqueur_bloquant_dans(&contenu, hote)
}

/// Cherche `@revoked` / `@cert-authority` visant `hote` dans un `known_hosts`.
///
/// Séparé de la lecture pour être exerçable. La correspondance est volontairement
/// **large** : on compare l'hôte à chaque nom de la liste séparée par des
/// virgules, sans traiter les motifs ni les entrées condensées — dans le doute,
/// mieux vaut refuser une connexion légitime que réapprendre une clé marquée.
/// Extrait le nom d'hôte d'un motif de `known_hosts`, en retirant la forme
/// `[hôte]:port` (celle qu'OpenSSH écrit pour un port non standard) et un
/// éventuel `:port` nu.
///
/// Trouvé par l'audit du 7 septembre 2026 : la forme crochetée `[hôte]:port`
/// n'était pas reconnue — seule la comparaison brute et un `split(':')` naïf
/// l'étaient —, si bien qu'une clé `@revoked` sur un port non standard passait
/// pour ne viser aucun hôte connu, était réapprise et acceptée. Un IPv6 littéral
/// nu (plusieurs `:`) n'est pas tronqué.
fn hote_de_motif_known_hosts(motif: &str) -> &str {
    let motif = motif.trim();
    if let Some(reste) = motif.strip_prefix('[') {
        if let Some((h, _)) = reste.split_once("]:") {
            return h;
        }
        return reste.strip_suffix(']').unwrap_or(reste);
    }
    match motif.rsplit_once(':') {
        Some((h, port))
            if !h.contains(':') && !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) =>
        {
            h
        }
        _ => motif,
    }
}

fn marqueur_bloquant_dans(contenu: &str, hote: &str) -> Option<String> {
    for ligne in contenu.lines() {
        let l = ligne.trim();
        if !l.starts_with('@') {
            continue;
        }
        let mut mots = l.split_whitespace();
        let marqueur = mots.next()?;
        if marqueur != "@revoked" && marqueur != "@cert-authority" {
            continue;
        }
        let Some(hotes) = mots.next() else { continue };
        if hotes
            .split(',')
            .any(|h| hote_de_motif_known_hosts(h) == hote)
        {
            return Some(marqueur.to_owned());
        }
    }
    None
}

/// Le fichier est-il là sans qu'on puisse l'ouvrir ?
///
/// Séparé du chemin pour être exerçable sans toucher au `HOME` du processus,
/// que tous les tests partagent. C'est la différence entre refuser une clé
/// qu'on ne peut pas vérifier et l'apprendre en silence.
fn fichier_present_mais_illisible(chemin: &std::path::Path) -> bool {
    // On tente une vraie LECTURE, pas seulement l'ouverture : sous Unix, ouvrir
    // un répertoire réussit (c'est la lecture qui échoue), et un `known_hosts`
    // remplacé par un répertoire passait alors pour « pas de souci ». Trouvé par
    // l'audit du 8 septembre 2026 : le test du cas n'exerçait même pas cette
    // fonction. Un fichier vide se lit sans erreur (0 octet) et n'est donc pas
    // signalé — c'est bien un premier contact, pas un fichier illisible.
    if !chemin.exists() {
        return false;
    }
    // Écarté AVANT toute ouverture : un tube nommé sans écrivain bloquerait le
    // `File::open` ci-dessous pour toujours (audit du 9 septembre 2026).
    if !est_fichier_ordinaire(chemin) {
        return true;
    }
    std::fs::File::open(chemin)
        .and_then(|mut f| {
            use std::io::Read as _;
            let mut octet = [0u8; 1];
            f.read(&mut octet).map(|_| ())
        })
        .is_err()
}

/// `whoami::username()` est faillible depuis la version 2 (compte systeme
/// illisible, environnement minimal). Un client SSH a toujours besoin d'un
/// nom : on retombe sur $USER, puis sur "user", plutot que d'echouer.
#[must_use]
pub fn current_username() -> String {
    whoami::username()
        .ok()
        .or_else(|| std::env::var("USER").ok())
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| "user".to_string())
}

pub const PASSWORD_REQUIRED: &str = "[AVASH_PASSWORD_REQUIRED]";

/// Marqueur place en tete du message quand la cle d'hote a CHANGE. L'interface
/// le reconnait pour proposer d'oublier l'ancienne cle et reessayer.
pub const HOST_KEY_CHANGED: &str = "[AVASH_HOST_KEY_CHANGED]";

/// Longueur maximale d'un texte venu du réseau dans un message d'erreur.
const TEXTE_DISTANT_MAX: usize = 200;

/// Neutralise un texte fourni par la machine d'en face avant de l'insérer dans
/// un message destiné à l'interface.
///
/// Trouvé par l'audit du 9 septembre 2026. Les marqueurs `[AVASH_…]` sont un
/// canal de commande entre le cœur et l'interface : `[AVASH_HOST_KEY_CHANGED]`
/// déclenche la proposition d'OUBLIER une clé d'hôte mémorisée. Le texte d'une
/// invite `keyboard-interactive` était recopié tel quel dans un message qui
/// porte parfois ces marqueurs, si bien qu'un serveur hostile pouvait en écrire
/// un lui-même et faire effacer la confiance TOFU d'un hôte sain. Le même texte
/// atteignait le terminal xterm.js sans filtre, séquences ANSI comprises.
///
/// On coupe donc les trois vecteurs à l'entrée, une seule fois, plutôt que de
/// se fier à chaque affichage en aval : caractères de contrôle, marqueurs
/// internes, longueur.
#[must_use]
pub fn texte_distant_sur(brut: &str) -> String {
    let sans_controle: String = brut
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let sans_marqueur = sans_marqueurs_internes(&sans_controle);
    let propre = sans_marqueur.trim();
    if propre.chars().count() <= TEXTE_DISTANT_MAX {
        return propre.to_string();
    }
    let garde: String = propre.chars().take(TEXTE_DISTANT_MAX).collect();
    format!("{}…", garde.trim_end())
}

/// Retire toute séquence `[AVASH_…]`, y compris mal formée : le préfixe seul
/// suffirait à un futur marqueur, on ne le laisse jamais repasser.
fn sans_marqueurs_internes(texte: &str) -> String {
    const DEBUT: &str = "[AVASH_";
    let mut sortie = String::with_capacity(texte.len());
    let mut reste = texte;
    while let Some(i) = reste.find(DEBUT) {
        sortie.push_str(&reste[..i]);
        let apres = &reste[i + DEBUT.len()..];
        // Un marqueur bien formé se referme sur `]` après des majuscules ; on
        // avale alors le tout. Sinon on n'avale que le préfixe, mais on avance
        // toujours, pour ne jamais boucler.
        reste = match apres.find(']') {
            Some(j)
                if apres[..j]
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c == '_') =>
            {
                &apres[j + 1..]
            }
            _ => apres,
        };
    }
    sortie.push_str(reste);
    sortie
}

#[derive(Clone)]
pub struct ClientAuth {
    pub user: String,
    /// Chemin de la clé privée (OpenSSH). À défaut, l'agent SSH est tenté.
    pub key_path: Option<PathBuf>,
    pub password: Option<String>,
}

/// Raison d'un refus de cle d'hote, partagee entre le handler et l'appelant.
///
/// `check_server_key` ne peut renvoyer qu'un `russh::Error` sans message : sans
/// ce canal, l'utilisateur ne verrait qu'un "Unknown key" opaque alors que
/// c'est l'avertissement le plus important de l'application.
type HostKeyVerdict = Arc<std::sync::Mutex<Option<String>>>;

/// Compteurs d'un tunnel, partages entre le relais et l'interface.
///
/// Atomiques : mis a jour depuis des taches concurrentes, lus a tout moment
/// par un instantane sans verrou.
#[derive(Debug, Default)]
pub struct ForwardCounters {
    /// Connexions en cours.
    pub active: AtomicU64,
    /// Connexions relayees depuis l'ouverture.
    pub total: AtomicU64,
    /// Octets client -> destination.
    pub bytes_up: AtomicU64,
    /// Octets destination -> client.
    pub bytes_down: AtomicU64,
}

impl ForwardCounters {
    /// Relaie `a` <-> `b` jusqu'a la fin de la connexion, en comptant.
    ///
    /// Ecrit a la main plutot que via `copy_bidirectional` : celui-ci ne rend
    /// les volumes qu'a la fin, alors qu'un tunnel porte souvent des
    /// connexions longues (base de donnees, VNC) que l'interface doit voir
    /// progresser.
    pub async fn relay<A, B>(&self, a: &mut A, b: &mut B)
    where
        A: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + ?Sized,
        B: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + ?Sized,
    {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        self.active.fetch_add(1, Ordering::Relaxed);
        self.total.fetch_add(1, Ordering::Relaxed);
        let mut buf_a = vec![0u8; 32 * 1024];
        let mut buf_b = vec![0u8; 32 * 1024];
        let (mut a_done, mut b_done) = (false, false);
        while !(a_done && b_done) {
            tokio::select! {
                r = a.read(&mut buf_a), if !a_done => match r {
                    Ok(0) | Err(_) => {
                        a_done = true;
                        // Fin de lecture d'un cote : on signale la fin
                        // d'ecriture a l'autre, qui repondra par son EOF.
                        let _ = b.shutdown().await;
                    }
                    Ok(n) => {
                        if b.write_all(&buf_a[..n]).await.is_err() { break; }
                        self.bytes_up.fetch_add(n as u64, Ordering::Relaxed);
                    }
                },
                r = b.read(&mut buf_b), if !b_done => match r {
                    Ok(0) | Err(_) => {
                        b_done = true;
                        let _ = a.shutdown().await;
                    }
                    Ok(n) => {
                        if a.write_all(&buf_b[..n]).await.is_err() { break; }
                        self.bytes_down.fetch_add(n as u64, Ordering::Relaxed);
                    }
                },
            }
        }
        self.active.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Destination locale d'une redirection distante (`ssh -R`).
struct RemoteTarget {
    host: String,
    port: u16,
    counters: Arc<ForwardCounters>,
}

/// Redirections distantes actives sur une session : port ecoute par le
/// serveur -> destination locale a joindre pour chaque connexion.
///
/// Le serveur ouvre lui-meme un canal `forwarded-tcpip` a chaque client qui
/// frappe a ce port ; le handler consulte cette table pour savoir vers quelle
/// adresse locale relayer.
type RemoteForwards = Arc<std::sync::Mutex<HashMap<u32, Arc<RemoteTarget>>>>;

/// Relais d'agent (`copy_bidirectional` canal <-> agent local) lancés pendant un
/// `run_avec_agent`, conservés pour être interrompus à la fin de la commande.
///
/// Trouvé par l'audit du 7 septembre 2026 : sans cette poignée, un canal
/// `auth-agent@openssh.com` ouvert par le serveur PENDANT la commande et gardé
/// ouvert survivait à `GardeAgent::drop` (qui ne remettait que le drapeau à
/// false), si bien que le serveur pouvait faire signer l'agent du poste pour
/// toute la durée de l'onglet — la borne « pendant cette commande, et seulement
/// pendant elle » de SECURITY.md n'était donc pas tenue.
type RelaisAgent = Arc<std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>>;

/// Handler d'auth + vérification `known_hosts`.
struct AvashAuth {
    host: String,
    port: u16,
    verdict: HostKeyVerdict,
    forwards: RemoteForwards,
    /// L'agent SSH du poste est-il redirigé vers ce serveur en ce moment ?
    /// Levé par `run_avec_agent` le temps d'une commande, jamais autrement :
    /// un serveur qui ouvrirait un canal d'agent hors de ce moment est refusé.
    agent_redirige: Arc<std::sync::atomic::AtomicBool>,
    /// Relais d'agent lancés pour cette session, interrompus à la fin de chaque
    /// commande par `GardeAgent::drop`.
    relais_agent: RelaisAgent,
}

/// Le socket (ou le tube) de l'agent SSH du poste, tel qu'OpenSSH le désigne.
#[cfg(unix)]
async fn ouvrir_agent_local() -> Option<tokio::net::UnixStream> {
    let chemin = std::env::var_os("SSH_AUTH_SOCK")?;
    tokio::net::UnixStream::connect(chemin).await.ok()
}

/// Transports possibles de l'agent SSH du poste sous Windows.
///
/// Défini hors `cfg(windows)` — c'est de la donnée, exerçable partout — pour que
/// le test verrouille l'ordre d'essai sans compiler la couche Windows.
#[cfg(any(windows, test))]
#[derive(Debug, PartialEq, Eq)]
enum TransportAgentWindows {
    /// Tube nommé du service `ssh-agent` d'OpenSSH.
    TubeOpenSsh,
    /// Pageant, l'agent de `PuTTY`.
    Pageant,
}

/// Ordre d'essai des transports de l'agent sous Windows : tube OpenSSH d'abord,
/// puis Pageant. Le MÊME que suivent l'auth (`authenticate_agent`,
/// `agent_has_identities`), et la source unique de l'ordre de la redirection.
///
/// Trouvé par l'audit du 7 septembre 2026 : `ouvrir_agent_local` n'ouvrait que
/// le tube OpenSSH, jamais Pageant, alors que l'auth essaie déjà les deux. Un
/// poste `PuTTY` où seul Pageant tourne (le cas visé par l'import `PuTTY` et la
/// conversion `.ppk`) s'authentifiait donc bien par l'agent, mais quand
/// `run_avec_agent` prêtait l'agent pour la « copie directe » d'un hôte à un
/// autre, `server_channel_open_agent_forward` répondait `ConnectFailed` au canal
/// `auth-agent@openssh.com` : le `scp` lancé chez la source n'avait aucune clé
/// et échouait en « Permission denied (publickey) », alors que la même clé avait
/// ouvert la session. Type slice (et non tableau fixe) pour que le test constate
/// un ordre incomplet à l'exécution plutôt qu'à la compilation.
#[cfg(any(windows, test))]
const ORDRE_TRANSPORTS_AGENT_WINDOWS: &[TransportAgentWindows] = &[
    TransportAgentWindows::TubeOpenSsh,
    TransportAgentWindows::Pageant,
];

/// Un flux d'octets bidirectionnel vers l'agent, quel que soit son transport.
///
/// `dyn AsyncRead + AsyncWrite` ne se dit pas (deux traits non-auto dans un même
/// objet) : ce trait fourre-tout les réunit pour que le tube OpenSSH et Pageant,
/// de types concrets différents, se rangent dans un même `Box`.
#[cfg(windows)]
trait FluxAgent: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
#[cfg(windows)]
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> FluxAgent for T {}

/// Ouvre le premier transport d'agent disponible, dans l'ordre partagé avec
/// l'auth. Rend un flux BRUT boxé (relayé octet à octet par
/// `copy_bidirectional`), et non l'`AgentClient` de la couche protocole que
/// `connect_pageant` construit : le canal `auth-agent@openssh.com` attend le
/// dialogue d'agent tel quel, pas une couche par-dessus.
#[cfg(windows)]
async fn ouvrir_agent_local() -> Option<Box<dyn FluxAgent>> {
    for transport in ORDRE_TRANSPORTS_AGENT_WINDOWS {
        match transport {
            TransportAgentWindows::TubeOpenSsh => {
                if let Ok(pipe) =
                    tokio::net::windows::named_pipe::ClientOptions::new().open(OPENSSH_AGENT_PIPE)
                {
                    return Some(Box::new(pipe));
                }
            }
            // `PageantStream` est le transport brut de Pageant (celui que russh
            // ouvre lui-même sous `connect_pageant`) : un flux tokio à relayer,
            // pas l'`AgentClient` protocole.
            TransportAgentWindows::Pageant => {
                if let Ok(flux) = pageant::PageantStream::new().await {
                    return Some(Box::new(flux));
                }
            }
        }
    }
    None
}

impl russh::client::Handler for AvashAuth {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        // Un certificat se valide contre une autorite, pas contre known_hosts.
        // On n'en gere pas encore : refuser vaut mieux qu'accepter a l'aveugle.
        let server_public_key = match server_public_key {
            russh::keys::PublicKeyOrCertificate::PublicKey { key, .. } => key,
            russh::keys::PublicKeyOrCertificate::Certificate(_) => {
                *self.verdict.lock().unwrap() = Some(
                    "Ce serveur présente un certificat SSH. Avash ne sait pas \
                     encore les valider et refuse la connexion."
                        .into(),
                );
                return Err(russh::Error::UnknownKey);
            }
        };
        // TOFU (Trust On First Use), avec la distinction que fait OpenSSH :
        // hôte inconnu  -> on apprend la clé (premier contact) ;
        // clé CHANGÉE   -> on refuse, sans jamais réapprendre en silence.
        //
        // On lit nous-mêmes les clés enregistrées plutôt que de nous fier au
        // booléen de `check_known_hosts` : celui-ci confond « algorithme
        // différent » avec « hôte inconnu » (voir `juger_cle_hote`).
        // Marqueurs OpenSSH : russh les ignore, sa correspondance d'hôte étant
        // une simple égalité de chaîne. Une ligne `@revoked srv …` était donc
        // découpée en hôte « @revoked », qui ne correspond à rien — verdict
        // « premier contact », et **la clé révoquée était réapprise et acceptée
        // sans un mot**, là où ssh(1) refuse catégoriquement. On ne sait pas
        // valider une autorité de certification non plus : dans les deux cas,
        // on refuse plutôt que de faire semblant.
        if let Some(marqueur) = marqueur_bloquant(&self.host) {
            *self.verdict.lock().unwrap() = Some(format!(
                "~/.ssh/known_hosts porte « {marqueur} » pour {}. Avash ne sait pas \
                 traiter ce marqueur et refuse plutôt que de l'ignorer — ce qui \
                 reviendrait à réapprendre une clé que vous avez marquée.",
                self.host
            ));
            return Err(russh::Error::UnknownKey);
        }
        if let Some(verdict) = known_hosts_illisible() {
            *self.verdict.lock().unwrap() = Some(verdict);
            return Err(russh::Error::UnknownKey);
        }
        let Some(chemin) = chemin_known_hosts() else {
            *self.verdict.lock().unwrap() = Some(
                "Répertoire personnel introuvable : impossible de vérifier \
                 l'identité du serveur. Connexion refusée."
                    .into(),
            );
            return Err(russh::Error::UnknownKey);
        };
        let enregistrees =
            match russh::keys::known_hosts::known_host_keys_path(&self.host, self.port, &chemin) {
                Ok(k) => k,
                Err(e) => {
                    *self.verdict.lock().unwrap() =
                        Some(format!("Vérification de la clé d'hôte impossible : {e}"));
                    return Err(russh::Error::UnknownKey);
                }
            };
        match juger_cle_hote(&enregistrees, server_public_key) {
            // Hôte connu, clé identique.
            VerdictCle::Connue => Ok(true),

            // Hôte inconnu : premier contact, on mémorise.
            VerdictCle::PremierContact => {
                // Trouvé par l'audit du 7 septembre 2026 : quand l'écriture de
                // known_hosts échoue (`~/.ssh` en lecture seule sur un poste
                // géré, dossier synchronisé, ou known_hosts en 0444), c'était le
                // SEUL refus de `check_server_key` à ne poser aucun verdict.
                // `connect` rendait alors le « Unknown server key » brut de russh
                // (ou, côté GUI, le seul « Connexion SSH à srv:22 »), et l'on ne
                // pouvait distinguer un hôte inconnu d'un fichier inécrivable. On
                // reste fail-closed (là où ssh(1) avertit et continue) mais on
                // nomme la cause.
                if let Err(reason) =
                    apprendre_cle_hote(&chemin, &self.host, self.port, server_public_key)
                {
                    *self.verdict.lock().unwrap() = Some(reason);
                    return Err(russh::Error::UnknownKey);
                }
                Ok(true)
            }

            // La clé d'hôte a changé : réinstallation du serveur, ou interception.
            // Dans le doute on refuse — c'est à l'utilisateur de trancher.
            VerdictCle::Changee { ligne: line } => {
                let fp = server_public_key.fingerprint(russh::keys::HashAlg::Sha256);
                *self.verdict.lock().unwrap() = Some(format!(
                    "{HOST_KEY_CHANGED} LA CLÉ D'HÔTE A CHANGÉ pour {}:{}.\n\n\
                     Soit le serveur a été réinstallé, soit quelqu'un intercepte \
                     la connexion.\n\n\
                     Nouvelle empreinte présentée :\n{fp}\n\n\
                     Ancienne clé : ligne {} de ~/.ssh/known_hosts.",
                    self.host, self.port, line
                ));
                Err(russh::Error::UnknownKey)
            }
        }
    }

    /// Le serveur relaie une connexion recue sur un port redirige (`-R`).
    ///
    /// Canal d'agent ouvert par le serveur : relayé vers l'agent du poste
    /// **seulement** pendant une commande lancée par `run_avec_agent`. Hors de
    /// ce moment, un serveur qui le tente est refusé : l'agent signe avec les
    /// clés du poste, et c'est l'utilisateur qui décide quand le prêter.
    ///
    /// La poignée du relais est conservée dans `relais_agent` pour que
    /// `GardeAgent::drop` l'interrompe à la fin de la commande : sans quoi un
    /// canal ouvert pendant la commande et gardé ouvert par le serveur
    /// continuerait de faire signer l'agent après elle (audit du 7 septembre
    /// 2026). Interrompre la tâche lâche le flux du canal, dont le `Drop`
    /// (russh `ChannelCloseOnDrop`) envoie `Close` au serveur.
    async fn server_channel_open_agent_forward(
        &mut self,
        channel: russh::Channel<russh::client::Msg>,
        reply: russh::client::ChannelOpenHandle,
        _session: &mut russh::client::Session,
    ) -> Result<(), Self::Error> {
        if !self
            .agent_redirige
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        }
        let Some(mut agent) = ouvrir_agent_local().await else {
            reply.reject(russh::ChannelOpenFailure::ConnectFailed).await;
            return Ok(());
        };
        reply.accept().await;
        let handle = tokio::spawn(async move {
            let mut flux = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut flux, &mut agent).await;
        });
        if let Ok(mut relais) = self.relais_agent.lock() {
            // Purge les relais déjà terminés d'eux-mêmes (canal fermé côté
            // serveur) pour ne pas accumuler sur une session longue.
            relais.retain(|h| !h.is_finished());
            relais.push(handle);
        }
        Ok(())
    }

    /// On ne relaie que vers une destination enregistree par `remote_forward` :
    /// un serveur malveillant ne peut pas nous faire ouvrir une connexion
    /// locale arbitraire.
    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: russh::Channel<russh::client::Msg>,
        _connected_address: &str,
        connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: russh::client::ChannelOpenHandle,
        _session: &mut russh::client::Session,
    ) -> Result<(), Self::Error> {
        let dest = self.forwards.lock().unwrap().get(&connected_port).cloned();
        let Some(target) = dest else {
            reply.reject(russh::ChannelOpenFailure::ConnectFailed).await;
            return Ok(());
        };
        // On accepte AVANT de joindre la destination : le serveur attend une
        // reponse rapide, et un echec local ferme simplement le canal.
        reply.accept().await;
        tokio::spawn(async move {
            let Ok(mut local) =
                tokio::net::TcpStream::connect((target.host.as_str(), target.port)).await
            else {
                let _ = channel.close().await;
                return;
            };
            let mut remote = channel.into_stream();
            target.counters.relay(&mut remote, &mut local).await;
        });
        Ok(())
    }
}

/// Un rebond (bastion) d'une chaine `ProxyJump`.
#[derive(Clone)]
pub struct Hop {
    pub addr: String,
    pub port: u16,
    pub auth: ClientAuth,
}

pub struct AvashSession {
    session: russh::client::Handle<AvashAuth>,
    forwards: RemoteForwards,
    /// Partagé avec le Handler : levé le temps d'un `run_avec_agent`.
    agent_redirige: Arc<std::sync::atomic::AtomicBool>,
    /// Partagé avec le Handler : relais d'agent en cours, interrompus à la fin
    /// de chaque commande par la garde.
    relais_agent: RelaisAgent,
    /// Rebonds gardes vivants : le transport de cette session passe par leurs
    /// canaux ; les lacher couperait la connexion. Jamais relu, seulement
    /// possede — d'ou l'allow.
    #[allow(dead_code)]
    jumps: Vec<AvashSession>,
}

/// Tube nommé exposé par l'agent d'OpenSSH pour Windows (service `ssh-agent`).
#[cfg(windows)]
const OPENSSH_AGENT_PIPE: &str = r"\\.\pipe\openssh-ssh-agent";

// L'agent SSH n'a pas le même transport selon la plateforme : socket Unix
// désigné par SSH_AUTH_SOCK sur Unix, tube nommé OpenSSH ou Pageant (PuTTY)
// sous Windows. Les deux fonctions ci-dessous portent donc toute la logique,
// génériques sur le transport ; seules les fonctions de connexion, côté
// appelant, sont spécifiques à la plateforme.

/// L'agent détient-il au moins une identité utilisable ?
async fn agent_porte_une_identite<S>(agent: &mut russh::keys::agent::client::AgentClient<S>) -> bool
where
    S: russh::keys::agent::client::AgentStream + Send + Unpin,
{
    agent
        .request_identities()
        .await
        .is_ok_and(|ids| !ids.is_empty())
}

/// Présente chaque identité de l'agent à la session, jusqu'à ce que l'une soit
/// acceptée. L'agent signe le défi : la clé privée ne quitte jamais l'agent.
async fn agent_authentifie<S>(
    session: &mut russh::client::Handle<AvashAuth>,
    user: &str,
    agent: &mut russh::keys::agent::client::AgentClient<S>,
) -> bool
where
    // AgentStream implique AsyncRead + AsyncWrite ; avec Send + Unpin, russh
    // fournit alors automatiquement le signataire attendu par la session.
    S: russh::keys::agent::client::AgentStream + Send + Unpin,
{
    let Ok(identities) = agent.request_identities().await else {
        return false;
    };
    for id in identities {
        // On ne gere que les cles publiques ; les certificats plus tard.
        let russh::keys::agent::AgentIdentity::PublicKey { key, .. } = id else {
            continue;
        };
        let hash = matches!(key.algorithm(), russh::keys::Algorithm::Rsa { .. })
            .then_some(russh::keys::HashAlg::Sha256);
        if let Ok(res) = session
            .authenticate_publickey_with(user, key, hash, agent)
            .await
        {
            if res.success() {
                return true;
            }
        }
    }
    false
}

/// Referme la redirection d'agent avec la commande qui l'avait ouverte, même
/// si elle sort en erreur.
struct GardeAgent {
    drapeau: Arc<std::sync::atomic::AtomicBool>,
    relais: RelaisAgent,
}
impl Drop for GardeAgent {
    fn drop(&mut self) {
        self.drapeau
            .store(false, std::sync::atomic::Ordering::Relaxed);
        // Trouvé par l'audit du 7 septembre 2026 : abaisser le drapeau ne
        // suffisait pas. Un canal d'agent ouvert PENDANT la commande et gardé
        // ouvert par le serveur avait déjà son relais lancé ; le drapeau ne
        // gouverne que l'OUVERTURE de nouveaux canaux, pas ceux déjà relayés.
        // On interrompt donc chaque relais : `abort()` (synchrone) lâche le flux
        // du canal, dont le `Drop` envoie `Close` au serveur et referme aussi le
        // lien vers l'agent. La commande fixe ainsi bien la borne du prêt.
        if let Ok(mut relais) = self.relais.lock() {
            for h in relais.drain(..) {
                h.abort();
            }
        }
    }
}

/// Bras de `tokio::select!` de `run_avec_agent` : ne se résout que lorsque
/// l'interface lève le drapeau d'annulation. On scrute plutôt qu'on n'attend un
/// `Notify` parce que le drapeau (un `AtomicBool` partagé) est déjà l'outil que
/// tout le reste des transferts consulte.
///
/// Prend l'`Option` et non `&Annulation` : `tokio::select!` évalue l'expression
/// asynchrone de chaque bras même quand sa condition `if` est fausse, donc un
/// `unwrap()` au point d'appel paniquerait sur une commande non annulable. Sans
/// drapeau, ce futur patiente indéfiniment et ne remporte jamais le `select!`.
async fn attendre_leve(annulation: Option<&crate::sftp::Annulation>) {
    match annulation {
        Some(a) => {
            while !a.load(Ordering::Relaxed) {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }
        None => std::future::pending::<()>().await,
    }
}

/// Bras `tokio::select!` de la boucle d'exécution bornée : ne se résout qu'à
/// l'échéance fournie. Sans échéance (`run` non borné), il patiente
/// indéfiniment et ne remporte jamais le `select!`, comme `attendre_leve`.
async fn attendre_echeance(echeance: Option<tokio::time::Instant>) {
    match echeance {
        Some(t) => tokio::time::sleep_until(t).await,
        None => std::future::pending::<()>().await,
    }
}

impl AvashSession {
    /// Config russh commune (keepalive : detecte une coupure NAT au lieu de
    /// laisser une session zombie).
    fn config() -> Arc<russh::client::Config> {
        Arc::new(russh::client::Config {
            keepalive_interval: Some(std::time::Duration::from_secs(30)),
            keepalive_max: 3,
            // russh laisse l'algorithme de Nagle actif par défaut
            // (`nodelay: false`, client/mod.rs:2287). Sur une session
            // interactive c'est exactement le mauvais choix : un petit segment
            // — une frappe — est retenu tant que le précédent n'est pas
            // acquitté, ce qui ajoute jusqu'à un aller-retour d'accusé retardé
            // à l'écho du shell. OpenSSH pose TCP_NODELAY sans condition pour
            // cette raison. Sans effet en réseau local, très sensible au-delà.
            nodelay: true,
            ..Default::default()
        })
    }

    fn handler(
        host: &str,
        port: u16,
    ) -> (
        AvashAuth,
        HostKeyVerdict,
        RemoteForwards,
        Arc<std::sync::atomic::AtomicBool>,
        RelaisAgent,
    ) {
        let verdict: HostKeyVerdict = Arc::new(std::sync::Mutex::new(None));
        let forwards: RemoteForwards = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let agent_redirige = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let relais_agent: RelaisAgent = Arc::new(std::sync::Mutex::new(Vec::new()));
        let handler = AvashAuth {
            host: host.to_string(),
            port,
            verdict: verdict.clone(),
            forwards: forwards.clone(),
            agent_redirige: agent_redirige.clone(),
            relais_agent: relais_agent.clone(),
        };
        (handler, verdict, forwards, agent_redirige, relais_agent)
    }

    /// Connexion directe (TCP), sans rebond.
    pub async fn connect(host: &str, port: u16, auth: &ClientAuth) -> Result<Self> {
        let (handler, verdict, forwards, agent_redirige, relais_agent) = Self::handler(host, port);
        let mut session = match russh::client::connect(Self::config(), (host, port), handler).await
        {
            Ok(s) => s,
            Err(e) => {
                // Un refus de cle d'hote porte un message explicite : on le
                // remonte tel quel plutot que le "Unknown key" de russh.
                if let Some(reason) = verdict.lock().unwrap().take() {
                    return Err(anyhow!(reason));
                }
                return Err(e).with_context(|| format!("Connexion SSH à {host}:{port}"));
            }
        };
        Self::authenticate(&mut session, auth).await?;
        Ok(Self {
            session,
            forwards,
            agent_redirige,
            relais_agent,
            jumps: Vec::new(),
        })
    }

    /// Connexion a travers zero, un ou plusieurs rebonds (`ProxyJump`).
    ///
    /// Chaque rebond est joint par un canal `direct-tcpip` ouvert sur le
    /// precedent : exactement ce que fait `ssh -J`. La cle d'hote de CHAQUE
    /// maillon est verifiee (`known_hosts`), pas seulement celle de la cible.
    pub async fn connect_via(
        hops: &[Hop],
        host: &str,
        port: u16,
        auth: &ClientAuth,
    ) -> Result<Self> {
        let Some((first, rest)) = hops.split_first() else {
            return Self::connect(host, port, auth).await;
        };
        let mut chain: Vec<AvashSession> = Vec::new();
        let mut current = Self::connect(&first.addr, first.port, &first.auth)
            .await
            .with_context(|| format!("Rebond {}:{}", first.addr, first.port))?;
        for hop in rest {
            let next = current
                .connect_hop(&hop.addr, hop.port, &hop.auth)
                .await
                .with_context(|| format!("Rebond {}:{}", hop.addr, hop.port))?;
            chain.push(current);
            current = next;
        }
        let mut target = current.connect_hop(host, port, auth).await?;
        chain.push(current);
        target.jumps = chain;
        Ok(target)
    }

    /// Ouvre une session SSH sur `host:port` a travers le canal de CE rebond.
    async fn connect_hop(&self, host: &str, port: u16, auth: &ClientAuth) -> Result<Self> {
        let channel = self
            .session
            .channel_open_direct_tcpip(host, u32::from(port), "127.0.0.1", 0)
            .await
            .with_context(|| format!("Le rebond n'a pas pu joindre {host}:{port}"))?;
        let (handler, verdict, forwards, agent_redirige, relais_agent) = Self::handler(host, port);
        let mut session =
            match russh::client::connect_stream(Self::config(), channel.into_stream(), handler)
                .await
            {
                Ok(s) => s,
                Err(e) => {
                    if let Some(reason) = verdict.lock().unwrap().take() {
                        return Err(anyhow!(reason));
                    }
                    return Err(e)
                        .with_context(|| format!("Connexion SSH à {host}:{port} via le rebond"));
                }
            };
        Self::authenticate(&mut session, auth).await?;
        Ok(Self {
            session,
            forwards,
            agent_redirige,
            relais_agent,
            jumps: Vec::new(),
        })
    }

    async fn authenticate(
        session: &mut russh::client::Handle<AvashAuth>,
        auth: &ClientAuth,
    ) -> Result<()> {
        // Raison pour laquelle la clé n'a pas servi (chiffrée par phrase de
        // passe, format non géré, droits) — gardée pour l'erreur finale, pas
        // fatale en soi. Trouvé par l'audit du 7 septembre 2026 : un `?` sur le
        // chargement coupait la connexion avant même d'essayer l'agent ou le
        // mot de passe, alors que `ssh` (clé dans l'agent) fonctionnait.
        let mut raison_cle: Option<String> = None;
        if let Some(key_path) = &auth.key_path {
            match russh::keys::load_secret_key(key_path, None) {
                Ok(key) => {
                    // Pour une cle RSA, `None` demanderait le hash historique
                    // SHA-1 (ssh-rsa), refuse par les serveurs OpenSSH recents.
                    // On presente donc rsa-sha2-256. Ignore pour les autres
                    // types (nos cles generees sont ed25519).
                    let hash = matches!(key.algorithm(), russh::keys::Algorithm::Rsa { .. })
                        .then_some(russh::keys::HashAlg::Sha256);
                    let key = russh::keys::PrivateKeyWithHashAlg::new(Arc::new(key), hash);
                    if session
                        .authenticate_publickey(&auth.user, key)
                        .await?
                        .success()
                    {
                        return Ok(());
                    }
                }
                Err(e) => {
                    // La clé n'est pas utilisable ici : on passe à l'agent puis
                    // au mot de passe, comme OpenSSH le fait pour une identité
                    // qu'il ne peut pas charger (« no such identity »).
                    raison_cle = Some(format!(
                        "la clé {} n'a pas pu être utilisée : {e:#}",
                        key_path.display()
                    ));
                }
            }
        }
        // Agent SSH : comme OpenSSH, on tente les cles chargees dans l'agent
        // (ssh-agent, gpg-agent, Pageant/pipe sous Windows via connect_env).
        // Une cle deverrouillee une fois, ou sur token materiel (YubiKey),
        // evite de saisir quoi que ce soit.
        if Self::authenticate_agent(session, &auth.user).await? {
            return Ok(());
        }
        // Ce que le serveur accepte encore, tel qu'il le dit lui-même. Sans
        // cela l'échec était muet sur sa cause : « authentification échouée »,
        // et à l'utilisateur de deviner s'il s'était trompé de mot de passe ou
        // si sa méthode n'était tout simplement pas proposée.
        let mut restantes: Vec<&'static str> = Vec::new();
        if let Some(password) = &auth.password {
            let issue = session.authenticate_password(&auth.user, password).await?;
            if issue.success() {
                return Ok(());
            }
            if let russh::client::AuthResult::Failure {
                remaining_methods, ..
            } = &issue
            {
                restantes = remaining_methods.iter().map(<&str>::from).collect();
            }
            // `password` refusé ne veut pas dire « mauvais mot de passe ».
            // Un hôte joint à un annuaire (SSSD/PAM) désactive très souvent
            // `PasswordAuthentication` et fait conduire la conversation par PAM,
            // en `keyboard-interactive`. OpenSSH bascule tout seul ; nous ne
            // savions pas, et l'utilisateur voyait « authentification échouée »
            // avec un mot de passe pourtant juste.
            if Self::authenticate_clavier(session, &auth.user, password).await? {
                return Ok(());
            }
        }
        // Marqueur reconnu par l'interface : elle demande alors le mot de
        // passe et retente, plutot que d'afficher un echec sans recours.
        // La raison de l'échec de la clé n'apparaît qu'ici, en fin de course,
        // pour ne pas masquer un succès par l'agent ou le mot de passe.
        let detail_cle = raison_cle
            .as_deref()
            .map(|r| format!(" ({r})"))
            .unwrap_or_default();
        if auth.password.is_none() {
            return Err(anyhow!(
                "{PASSWORD_REQUIRED} Aucune méthode d'authentification n'a abouti pour « {} ». \
                 Un mot de passe est nécessaire.{detail_cle}",
                auth.user
            ));
        }
        if restantes.is_empty() {
            return Err(anyhow!(
                "Authentification échouée pour {}.{detail_cle}",
                auth.user
            ));
        }
        Err(anyhow!(
            "Authentification échouée pour {}. Le serveur propose encore : {}.{detail_cle}",
            auth.user,
            restantes.join(", ")
        ))
    }

    /// Le message d'échec qui nomme l'invite à laquelle Avash ne sait pas
    /// répondre. Extrait de `authenticate_clavier` pour être testable : le
    /// texte de l'invite vient du SERVEUR, donc d'une source non fiable.
    fn message_prompt_non_supporte(prompt: &str) -> String {
        format!(
            "Le serveur demande « {} », qui n'est pas un mot de passe \
             (la réponse s'afficherait en clair). Avash ne sait pas \
             encore répondre à une authentification à plusieurs facteurs.",
            texte_distant_sur(prompt)
        )
    }

    /// Authentification `keyboard-interactive`, en répondant le mot de passe.
    ///
    /// C'est le mécanisme par lequel un serveur délègue la conversation à PAM :
    /// il pose des questions, le client répond. Le cas courant est une invite
    /// unique et masquée (« Password: »), à laquelle on répond le mot de passe
    /// déjà saisi.
    ///
    /// **On ne répond pas à n'importe quoi.** Une invite en clair (`echo`),
    /// c'est-à-dire dont la réponse s'affiche, n'est pas un mot de passe : ce
    /// peut être un code à usage unique, une question de sécurité, un choix de
    /// second facteur. Y envoyer le mot de passe le livrerait en clair à
    /// l'écran du serveur, et n'aboutirait pas. Dans ce cas on renonce en
    /// nommant ce que le serveur demandait, ce qui vaut mieux qu'un échec muet.
    async fn authenticate_clavier(
        session: &mut russh::client::Handle<AvashAuth>,
        user: &str,
        password: &str,
    ) -> Result<bool> {
        use russh::client::KeyboardInteractiveAuthResponse as Reponse;

        // Le serveur peut enchaîner plusieurs tours ; on borne pour ne pas
        // tourner indéfiniment face à un serveur qui pose sans fin.
        const TOURS_MAX: usize = 8;
        let mut reponse = session
            .authenticate_keyboard_interactive_start(user.to_owned(), None)
            .await?;
        for _ in 0..TOURS_MAX {
            match reponse {
                Reponse::Success => return Ok(true),
                Reponse::Failure { .. } => return Ok(false),
                Reponse::InfoRequest { prompts, .. } => {
                    // Un tour sans question : le serveur se contente d'afficher
                    // quelque chose (bannière PAM). On répond une liste vide.
                    if let Some(clair) = prompts.iter().find(|p| p.echo) {
                        return Err(anyhow!(Self::message_prompt_non_supporte(&clair.prompt)));
                    }
                    let reponses = vec![password.to_owned(); prompts.len()];
                    reponse = session
                        .authenticate_keyboard_interactive_respond(reponses)
                        .await?;
                }
            }
        }
        Ok(false)
    }

    /// L'agent SSH expose-t-il au moins une identite ? Permet a l'interface de
    /// ne PAS reclamer de mot de passe quand l'agent peut authentifier.
    pub async fn agent_has_identities() -> bool {
        use russh::keys::agent::client::AgentClient;
        #[cfg(unix)]
        {
            let Ok(mut agent) = AgentClient::connect_env().await else {
                return false;
            };
            agent_porte_une_identite(&mut agent).await
        }
        #[cfg(windows)]
        {
            if let Ok(mut agent) = AgentClient::connect_named_pipe(OPENSSH_AGENT_PIPE).await {
                if agent_porte_une_identite(&mut agent).await {
                    return true;
                }
            }
            let Ok(mut agent) = AgentClient::connect_pageant().await else {
                return false;
            };
            agent_porte_une_identite(&mut agent).await
        }
    }

    /// Tente l'authentification via l'agent SSH. `Ok(true)` si une cle de
    /// l'agent a ete acceptee. Une absence d'agent n'est pas une erreur.
    async fn authenticate_agent(
        session: &mut russh::client::Handle<AvashAuth>,
        user: &str,
    ) -> Result<bool> {
        use russh::keys::agent::client::AgentClient;
        #[cfg(unix)]
        {
            let Ok(mut agent) = AgentClient::connect_env().await else {
                return Ok(false);
            };
            Ok(agent_authentifie(session, user, &mut agent).await)
        }
        #[cfg(windows)]
        {
            if let Ok(mut agent) = AgentClient::connect_named_pipe(OPENSSH_AGENT_PIPE).await {
                if agent_authentifie(session, user, &mut agent).await {
                    return Ok(true);
                }
            }
            let Ok(mut agent) = AgentClient::connect_pageant().await else {
                return Ok(false);
            };
            Ok(agent_authentifie(session, user, &mut agent).await)
        }
    }

    /// Exécution one-shot : sortie (stdout ET stderr mêlés, `ExtendedData`
    /// compris) + exit code. Le mélange est voulu — la sonde d'OS
    /// (`osinfo::parse_probe_output`) doit d'ailleurs tolérer le bruit de
    /// stderr d'un shell distant.
    ///
    /// Un code de sortie n'est rendu que si le serveur en a envoyé un : une
    /// commande tuée par un signal, ou un canal fermé sans statut, rend une
    /// erreur et non un `0` de complaisance (audit du 9 septembre 2026).
    pub async fn run(&mut self, command: &str) -> Result<(String, u32)> {
        self.executer_borne(command, None).await
    }

    /// Comme [`run`](Self::run), mais borné dans le temps : au terme de `delai`,
    /// on ferme le canal exec et l'on rend une erreur, au lieu de laisser la
    /// commande courir.
    ///
    /// Trouvé par l'audit du 7 septembre 2026 : la sonde d'OS s'appuyait sur un
    /// `tokio::time::timeout` posé PAR-DESSUS `run`. À l'échéance, le futur de
    /// `run` était lâché avec son canal — mais `russh::Channel` n'envoie pas de
    /// `CHANNEL_CLOSE` à sa chute, et la boucle de session du client réalimente la
    /// fenêtre AVANT de livrer les données : un serveur qui débite lentement
    /// (moins d'un mébioctet en quatre secondes, jamais le plafond) continuait
    /// donc d'inonder toute la vie de l'onglet. Le délai bornait l'attente, pas
    /// le flux. En bornant à l'intérieur, on ferme le canal avant de rendre :
    /// `close()` retire le canal de la table du client, la fenêtre cesse d'être
    /// réalimentée, et le serveur se bloque de lui-même.
    pub async fn run_borne(
        &mut self,
        command: &str,
        delai: std::time::Duration,
    ) -> Result<(String, u32)> {
        self.executer_borne(command, Some(tokio::time::Instant::now() + delai))
            .await
    }

    /// Boucle d'exécution one-shot partagée par `run` (échéance `None`) et
    /// `run_borne` (échéance `Some`).
    async fn executer_borne(
        &mut self,
        command: &str,
        echeance: Option<tokio::time::Instant>,
    ) -> Result<(String, u32)> {
        const PLAFOND: usize = 1024 * 1024;
        let mut channel = self.session.channel_open_session().await?;
        channel.exec(false, command).await?;
        let mut stdout = String::new();
        let mut exit_code = 0u32;
        let mut statut_recu = false;
        let mut signal_recu: Option<String> = None;

        // ⚠️ Ne PAS casser sur Eof : dans le protocole SSH, `exit-status`
        // arrive APRES l'EOF. Casser sur Eof renverrait donc toujours 0, quel
        // que soit le vrai code de sortie — verifie contre un vrai serveur.
        // On laisse la boucle courir jusqu'a la fermeture du canal (wait()
        // rend None), ou jusqu'a Close.
        //
        // La sortie était accumulée sans plafond. Un serveur hostile — dont la
        // clé d'hôte est déjà connue, donc TOFU satisfait — n'avait qu'à
        // répondre `cat /dev/zero` à la sonde d'OS lancée à chaque ouverture
        // d'onglet : plusieurs gigaoctets alloués d'un bloc, et c'est tout Avash
        // qui tombe, avec tous ses autres onglets, tunnels et transferts. Aucun
        // appelant de `run` n'attend plus que quelques kilo-octets.
        let mut tronquee = false;
        let mut expire = false;
        loop {
            tokio::select! {
                msg = channel.wait() => {
                    let Some(msg) = msg else { break };
                    match msg {
                        russh::ChannelMsg::Data { ref data }
                        | russh::ChannelMsg::ExtendedData { ref data, .. } => {
                            if stdout.len() >= PLAFOND {
                                tronquee = true;
                                break;
                            }
                            stdout.push_str(&String::from_utf8_lossy(data));
                        }
                        russh::ChannelMsg::ExitStatus { exit_status } => {
                            exit_code = exit_status;
                            statut_recu = true;
                        }
                        // Commande tuée par un signal côté distant : le serveur
                        // envoie `exit-signal` et JAMAIS `exit-status`
                        // (RFC 4254 §6.10). On note le signal et on sort par le
                        // bas, pour fermer le canal comme toute autre sortie.
                        russh::ChannelMsg::ExitSignal {
                            ref signal_name, ..
                        } => {
                            signal_recu = Some(format!("{signal_name:?}"));
                            break;
                        }
                        russh::ChannelMsg::Close => break,
                        _ => {}
                    }
                }
                // Sans échéance, ce bras ne se résout jamais et la boucle se
                // comporte comme l'ancienne. Le futur `wait()` abandonné, on
                // ferme le canal HORS du `select!` (pas de double emprunt).
                () = attendre_echeance(echeance) => {
                    expire = true;
                    break;
                }
            }
        }
        // Trouvé par l'audit du 7 septembre 2026 : on sortait au plafond sans
        // fermer le canal. Or `russh::Channel` n'envoie pas de CHANNEL_CLOSE à
        // sa chute, et le client réalimente la fenêtre quoi qu'il arrive : le
        // serveur pouvait donc continuer de nous inonder à plein débit toute la
        // vie de la session, un cœur brûlé par onglet. On ferme sur TOUTE sortie
        // (plafond, échéance, EOF, Close) : `close()` retire le canal de la
        // table du client, la fenêtre n'est plus réalimentée, le serveur cale de
        // lui-même. Inoffensif quand le canal est déjà clos.
        let _ = channel.close().await;
        if expire {
            anyhow::bail!("La commande distante n'a pas répondu dans le délai imparti.");
        }
        if let Some(signal) = signal_recu {
            anyhow::bail!("La commande distante a été interrompue par un signal ({signal}).");
        }
        if tronquee {
            // Au plafond on coupe la lecture avant tout statut : on rend ce
            // qu'on a lu, la mention de troncature disant déjà l'issue.
            stdout.push_str("\n[sortie tronquée : plafond de 1 Mio atteint]\n");
            return Ok((stdout, exit_code));
        }
        // Trouvé par l'audit du 9 septembre 2026 : `exit_code` valait 0 par
        // défaut et n'était renseigné que par `ExitStatus`. Un canal fermé sans
        // statut rendait donc 0, « réussi », alors que la commande avait été
        // tuée ou le lien coupé : `key_deploy` annonçait un déploiement de clé
        // abouti, et l'exécution de commande affichait « [exit 0] ». Le
        // correctif existait pour `run_avec_agent` depuis le 7 septembre, il
        // n'avait jamais été porté sur cette boucle-ci.
        if !statut_recu {
            anyhow::bail!(
                "La commande distante s'est terminée sans code de sortie \
                 (canal fermé prématurément) : issue inconnue."
            );
        }
        Ok((stdout, exit_code))
    }

    /// Exécution one-shot avec l'agent SSH du poste redirigé vers le serveur,
    /// le temps de la commande : c'est ce qui permet à ce serveur de se
    /// connecter à un autre avec les clés du poste (copie directe d'un hôte à
    /// un autre, `scp` lancé là-bas). Sortie standard et d'erreur mêlées,
    /// bornées à un mébioctet, et le code de sortie.
    ///
    /// La redirection n'est ouverte que pendant cet appel : avant, après, ou
    /// pour toute autre commande, un canal d'agent demandé par le serveur est
    /// refusé. Et la fin de la commande ne se contente pas d'interdire les
    /// canaux suivants : elle interrompt aussi ceux ouverts pendant elle (voir
    /// `GardeAgent::drop`), pour qu'un serveur ne garde pas l'agent prêté au-delà
    /// en laissant un canal ouvert.
    ///
    /// `annulation` (quand elle est fournie) rend la commande interruptible :
    /// l'interface la lève pour annuler une copie directe. Trouvé par l'audit
    /// du 7 septembre 2026 : sans elle, la copie directe (scp lancé chez la
    /// source par cet appel) n'était pas interruptible — le front montrait un
    /// bouton « Annuler » inerte, et pendant une copie de plusieurs Go la
    /// session source restait verrouillée et l'agent prêté du début à la fin.
    /// On scrute donc le drapeau entre deux blocs et, quand il est levé, on
    /// ferme le canal exec : le serveur envoie SIGHUP au scp distant, la garde
    /// referme la redirection d'agent en sortant, et l'appelant reçoit `ANNULE`.
    pub async fn run_avec_agent(
        &self,
        command: &str,
        annulation: Option<&crate::sftp::Annulation>,
    ) -> Result<(String, u32)> {
        const PLAFOND: usize = 1024 * 1024;
        let drapeau = self.agent_redirige.clone();
        drapeau.store(true, std::sync::atomic::Ordering::Relaxed);
        // Quoi qu'il arrive, la redirection se referme avec la commande : le
        // drapeau retombe ET les relais d'agent déjà ouverts sont interrompus.
        let _garde = GardeAgent {
            drapeau,
            relais: self.relais_agent.clone(),
        };

        let mut channel = self.session.channel_open_session().await?;
        channel
            .agent_forward(true)
            .await
            .context("demande de redirection d'agent")?;
        // Trouvé par l'audit du 7 septembre 2026 : `agent_forward(true)` ne fait
        // qu'ENVOYER la requête (`want_reply = true`) ; le verdict du serveur
        // arrive plus tard en `ChannelMsg::Success`/`Failure`. La boucle exec
        // plus bas l'ignorait (`_ => {}`), si bien qu'un serveur durci
        // `AllowAgentForwarding no` (qui répond CHANNEL_FAILURE) était avalé :
        // Avash lançait quand même `scp … autre:` là-bas, qui échouait
        // « Permission denied (publickey) » en accusant la clé, jamais le refus
        // de redirection. On consomme donc le verdict AVANT `exec`.
        //
        // Attente BORNÉE, et silence = on continue : tous les serveurs
        // n'émettent pas ce verdict sur le canal. russh 0.63 côté serveur y
        // répond même par un message GLOBAL (REQUEST_SUCCESS/FAILURE) que le
        // client ne voit jamais sur le canal, et un pair non conforme peut se
        // taire malgré `want_reply`. À l'expiration on poursuit comme avant
        // plutôt que de rendre la copie directe inutilisable contre ces pairs.
        let refuse = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                match channel.wait().await {
                    Some(russh::ChannelMsg::Success) => break false,
                    Some(russh::ChannelMsg::Failure) => break true,
                    // Canal clos avant tout verdict : on laisse `exec` échouer.
                    Some(russh::ChannelMsg::Close | russh::ChannelMsg::Eof) | None => break false,
                    // Ajustement de fenêtre, etc. : on attend encore le verdict.
                    Some(_) => {}
                }
            }
        })
        .await
        .unwrap_or(false);
        if refuse {
            anyhow::bail!(
                "Le serveur refuse la redirection d'agent (AllowAgentForwarding). \
                 La copie directe d'hôte à hôte a besoin de vos clés là-bas."
            );
        }
        channel.exec(false, command).await?;
        let mut sortie = String::new();
        let mut exit_code = 0u32;
        let mut statut_recu = false;
        let mut signal_recu: Option<String> = None;
        let mut tronquee = false;
        let mut annule = false;
        loop {
            tokio::select! {
                msg = channel.wait() => {
                    let Some(msg) = msg else { break };
                    match msg {
                        russh::ChannelMsg::Data { ref data }
                        | russh::ChannelMsg::ExtendedData { ref data, .. } => {
                            if sortie.len() >= PLAFOND {
                                tronquee = true;
                                break;
                            }
                            sortie.push_str(&String::from_utf8_lossy(data));
                        }
                        russh::ChannelMsg::ExitStatus { exit_status } => {
                            exit_code = exit_status;
                            statut_recu = true;
                        }
                        // Commande tuée par un signal : pas de code de sortie, mais un
                        // échec bien réel — surtout pour une copie directe (scp).
                        // Trouvé par l'audit du 9 septembre 2026 : ce bras rendait
                        // l'erreur par un `return` direct, seul chemin de sortie à
                        // sauter la fermeture du canal promise plus bas. On note le
                        // signal et on sort par le bas, comme toutes les autres.
                        russh::ChannelMsg::ExitSignal {
                            ref signal_name, ..
                        } => {
                            signal_recu = Some(format!("{signal_name:?}"));
                            break;
                        }
                        russh::ChannelMsg::Close => break,
                        _ => {}
                    }
                }
                // Sans drapeau, ce bras patiente sans jamais se résoudre :
                // `channel.wait()` seul reste, comme avant. On ferme le canal
                // hors du `select!`, une fois le futur `wait()` abandonné, pour
                // ne pas emprunter `channel` deux fois.
                () = attendre_leve(annulation) => {
                    annule = true;
                    break;
                }
            }
        }
        // On ferme le canal sur TOUTE sortie de la boucle, pas seulement à
        // l'annulation. Trouvé par l'audit du 7 septembre 2026 : au plafond de
        // 1 Mio on sortait sans fermer, et `russh::Channel` n'envoie pas de
        // CHANNEL_CLOSE à sa chute pendant que le client réalimente la fenêtre —
        // le serveur pouvait donc continuer de nous inonder toute la vie de la
        // session. `close()` retire le canal de la table du client, la fenêtre
        // n'est plus réalimentée, le serveur cale. Inoffensif si déjà clos.
        let _ = channel.close().await;
        if annule {
            anyhow::bail!(crate::sftp::ANNULE);
        }
        if let Some(signal) = signal_recu {
            return Err(anyhow!(
                "La commande distante a été interrompue par un signal ({signal})."
            ));
        }
        if tronquee {
            sortie.push_str("\n[sortie tronquée : plafond de 1 Mio atteint]\n");
            return Ok((sortie, exit_code));
        }
        // Trouvé par l'audit du 7 septembre 2026 : `exit_code` valait 0 par
        // défaut et n'était renseigné que par `ExitStatus`. Un canal fermé sans
        // statut (lien coupé, processus disparu) rendait donc 0 — « réussi » —
        // alors que la copie directe (scp), seul appelant, était interrompue en
        // plein transfert. Sans statut de sortie, on ne conclut pas au succès.
        if !statut_recu {
            return Err(anyhow!(
                "La commande distante s'est terminée sans code de sortie \
                 (canal fermé prématurément) : issue inconnue."
            ));
        }
        Ok((sortie, exit_code))
    }

    /// Ouvre un canal PTY interactif.
    /// Le front écrit dans `in_tx` (touches clavier), lit `out_rx` (flux terminal),
    /// et appelle `resize_tx` pour `window_change`.
    pub async fn open_pty(&mut self, cols: u32, rows: u32, term: &str) -> Result<PtyChannel> {
        let channel = self.session.channel_open_session().await?;
        // `want_reply = false` : on n'attend PAS de verdict du serveur, donc ce
        // contexte ne peut couvrir qu'une erreur d'ENVOI de la requête, jamais
        // un refus du serveur. Trouvé par l'audit du 7 septembre 2026 : l'ancien
        // « Demande PTY refusée » mentait sur ce qu'il détecte (le canal exec de
        // `run_avec_agent` avait le même angle mort sur `agent_forward`).
        channel
            .request_pty(false, term, cols, rows, 0, 0, &[])
            .await
            .context("envoi de la demande PTY")?;
        channel.request_shell(false).await?;

        // Le canal est partagé entre le pump de sortie et le writer d'entrée :
        // russh::Channel est clonable via son sender interne ? Non — mais on
        // dédouble : le stream into_stream() possède le canal. On garde donc
        // une approche à deux moitiés : wait() pour la sortie, data() pour l'entrée
        // n'est pas possible sur le même objet possédé. Solution russh idiomatique :
        // cloner le canal (russh 0.45 : Channel implémente Clone ? Non).
        // → On utilise une seule tâche qui possède le canal et traite via select.
        let (in_tx, mut in_rx) = mpsc::channel::<Vec<u8>>(256);
        let (out_tx, out_rx) = mpsc::channel::<Vec<u8>>(256);
        let (resize_tx, mut resize_rx) = mpsc::channel::<(u32, u32)>(16);

        let mut pump_channel = channel;
        let pump = tokio::spawn(async move {
            // Le resize est optionnel : sa fermeture ne doit pas tuer la session,
            // mais son bras select! doit etre desactive (voir plus bas).
            let mut resize_closed = false;
            loop {
                tokio::select! {
                    // Sortie du serveur → front
                    msg = pump_channel.wait() => {
                        match msg {
                            Some(russh::ChannelMsg::Data { ref data }) => {
                                if out_tx.send(data.to_vec()).await.is_err() { break; }
                            }
                            Some(russh::ChannelMsg::ExtendedData { ref data, .. }) => {
                                if out_tx.send(data.to_vec()).await.is_err() { break; }
                            }
                            Some(russh::ChannelMsg::Eof | russh::ChannelMsg::Close) | None => break,
                            Some(_) => {}
                        }
                    }
                    // Clavier du front → stdin serveur
                    maybe = in_rx.recv() => {
                        match maybe {
                            Some(bytes) => {
                                let mut cursor = std::io::Cursor::new(bytes);
                                if pump_channel.data(&mut cursor).await.is_err() { break; }
                            }
                            None => break,
                        }
                    }
                    // Resize du front → window_change.
                    // La garde `if !resize_closed` est indispensable : un canal
                    // ferme rend Ready(None) immediatement et sans fin, et ce
                    // bras ferait tourner la boucle a vide a 100 % de CPU.
                    // On desactive donc le bras plutot que d'ignorer le None.
                    maybe = resize_rx.recv(), if !resize_closed => {
                        match maybe {
                            Some((c, r)) => {
                                if pump_channel.window_change(c, r, 0, 0).await.is_err() { break; }
                            }
                            None => resize_closed = true,
                        }
                    }
                }
            }
            let _ = pump_channel.close().await;
        });

        Ok(PtyChannel {
            out_rx,
            in_tx,
            resize_tx,
            _pump: pump,
        })
    }

    pub async fn disconnect(&self) -> Result<()> {
        self.session
            .disconnect(russh::Disconnect::ByApplication, "au revoir", "")
            .await?;
        Ok(())
    }

    // ---------- Redirections de port ----------

    /// Ouvre un canal `direct-tcpip` : le serveur joint `host:port` pour nous
    /// (`ssh -L` et `-D`). Le canal se manipule comme un flux TCP.
    pub async fn open_direct_tcpip(
        &self,
        host: &str,
        port: u16,
        originator: std::net::SocketAddr,
    ) -> Result<russh::Channel<russh::client::Msg>> {
        self.session
            .channel_open_direct_tcpip(
                host,
                u32::from(port),
                originator.ip().to_string(),
                u32::from(originator.port()),
            )
            .await
            .with_context(|| format!("Le serveur n'a pas pu joindre {host}:{port}"))
    }

    /// Demande au serveur d'ecouter sur `bind_addr:port` (`ssh -R`) et de
    /// relayer chaque connexion vers `local_host:local_port` chez nous.
    ///
    /// Rend le port effectivement ecoute (le serveur en choisit un si `port`
    /// vaut 0).
    pub async fn remote_forward(
        &self,
        bind_addr: &str,
        port: u16,
        local_host: &str,
        local_port: u16,
        counters: Arc<ForwardCounters>,
    ) -> Result<u16> {
        // Enregistre avant la demande : une connexion peut arriver des que le
        // serveur ecoute, avant meme que sa reponse nous parvienne.
        let dest = Arc::new(RemoteTarget {
            host: local_host.to_string(),
            port: local_port,
            counters,
        });
        self.forwards
            .lock()
            .unwrap()
            .insert(u32::from(port), dest.clone());
        let bound = self
            .session
            .tcpip_forward(bind_addr, u32::from(port))
            .await
            .with_context(|| format!("Le serveur refuse d'écouter sur {bind_addr}:{port}"))?;
        // Port 0 : le serveur a choisi, on retient le vrai numero.
        let bound = if port == 0 && bound != 0 {
            let mut f = self.forwards.lock().unwrap();
            f.remove(&0);
            f.insert(bound, dest);
            u16::try_from(bound).unwrap_or(0)
        } else {
            port
        };
        Ok(bound)
    }

    /// La connexion au serveur est-elle tombee (reseau, keepalive, kill) ?
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.session.is_closed()
    }

    /// Annule une redirection distante.
    pub async fn cancel_remote_forward(&self, bind_addr: &str, port: u16) -> Result<()> {
        self.forwards.lock().unwrap().remove(&u32::from(port));
        self.session
            .cancel_tcpip_forward(bind_addr, u32::from(port))
            .await?;
        Ok(())
    }

    /// Ouvre un canal dédié au sous-système SFTP (canal indépendant, session intacte).
    pub async fn open_sftp_channel(&mut self) -> Result<russh::Channel<russh::client::Msg>> {
        let channel = self.session.channel_open_session().await?;
        channel.request_subsystem(true, "sftp").await?;
        Ok(channel)
    }
}

/// Canal PTY exposé au front : sortie terminal + entrée clavier + resize.
pub struct PtyChannel {
    pub out_rx: mpsc::Receiver<Vec<u8>>,
    pub in_tx: mpsc::Sender<Vec<u8>>,
    pub resize_tx: mpsc::Sender<(u32, u32)>,
    _pump: tokio::task::JoinHandle<()>,
}

/// Oublie la (les) cle(s) d'hote memorisee(s) pour `host:port` : retire les
/// lignes correspondantes de `~/.ssh/known_hosts`. Le prochain contact
/// re-apprendra la nouvelle cle (TOFU). Rend le nombre de lignes retirees.
pub fn forget_host_key(host: &str, port: u16) -> Result<usize> {
    let path = crate::repertoire_personnel()
        .ok_or_else(|| anyhow!("Répertoire personnel introuvable"))?
        .join(".ssh/known_hosts");
    forget_host_key_at(host, port, &path)
}

/// Apprend la clé d'hôte au premier contact en l'ajoutant à `chemin`
/// (`known_hosts`). Cœur testable sur un chemin explicite, comme
/// [`forget_host_key_at`] : rend, en cas d'échec d'écriture, le verdict prêt à
/// poser (avec la cause `{e}` : EACCES, ENOSPC et EROFS restent distincts) au
/// lieu de laisser remonter le « Unknown server key » brut de russh.
fn apprendre_cle_hote(
    chemin: &Path,
    host: &str,
    port: u16,
    cle: &russh::keys::PublicKey,
) -> std::result::Result<(), String> {
    russh::keys::known_hosts::learn_known_hosts_path(host, port, cle, chemin).map_err(|e| {
        format!(
            "Impossible d'enregistrer la clé de {host}:{port} dans {} : {e}. \
             Connexion refusée.",
            chemin.display()
        )
    })
}

/// Coeur testable de [`forget_host_key`], sur un fichier `known_hosts`
/// explicite (evite toute dependance a `HOME` dans les tests).
pub fn forget_host_key_at(host: &str, port: u16, path: &Path) -> Result<usize> {
    let lines: Vec<usize> = russh::keys::known_hosts::known_host_keys_path(host, port, path)
        .map_err(|e| anyhow!("Lecture de known_hosts : {e}"))?
        .into_iter()
        .map(|(line, _key)| line)
        .collect();
    if lines.is_empty() {
        return Ok(0);
    }
    let content =
        std::fs::read_to_string(path).with_context(|| format!("Lecture de {}", path.display()))?;
    // known_host_keys numérote à partir de 1, mais — trouvé par l'audit du
    // 7 septembre 2026 — russh ne compte PAS les lignes de commentaire (`# …`) :
    // son `continue` saute l'incrément. Un simple `enumerate()` sur les lignes
    // physiques les compte, lui : dès qu'un commentaire précède les entrées, la
    // numérotation se décale et l'on retirait la clé d'un AUTRE hôte (son TOFU
    // repartait de zéro) au lieu de celle demandée. On reproduit donc ici la
    // règle de comptage de russh : un commentaire n'a pas de numéro logique et
    // se conserve ; toute autre ligne porte le numéro courant, puis l'incrémente.
    let to_drop: std::collections::HashSet<usize> = lines.into_iter().collect();
    let mut numero = 1usize;
    let kept: Vec<&str> = content
        .lines()
        .filter(|l| {
            if l.as_bytes().first() == Some(&b'#') {
                return true; // commentaire : jamais numéroté, toujours conservé
            }
            let a_retirer = to_drop.contains(&numero);
            numero += 1;
            !a_retirer
        })
        .collect();
    let mut out = kept.join("\n");
    if content.ends_with('\n') {
        out.push('\n');
    }
    // Une coupure ici laissait un known_hosts vide : toutes les clés apprises
    // disparaissaient et les connexions suivantes réapprenaient en silence —
    // une interception passait alors inaperçue.
    crate::ecrire_atomiquement(path, out.as_bytes())?;
    Ok(to_drop.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_username_ne_rend_jamais_vide() {
        // Un client SSH a toujours besoin d'un nom : le repli garantit une
        // valeur non vide meme sans compte systeme lisible.
        assert!(!current_username().is_empty());
    }
}

#[cfg(test)]
mod tests_transport_agent {
    use super::{TransportAgentWindows, ORDRE_TRANSPORTS_AGENT_WINDOWS};

    /// La redirection d'agent sous Windows doit essayer Pageant, et pas seulement
    /// le tube OpenSSH. Trouvé par l'audit du 7 septembre 2026 :
    /// `ouvrir_agent_local` (cfg windows) n'ouvrait que le tube OpenSSH, alors que
    /// l'authentification essaie tube OpenSSH PUIS Pageant. Un poste `PuTTY` où seul
    /// Pageant tourne s'authentifiait donc, mais voyait la « copie directe » d'un
    /// hôte à un autre refusée « Permission denied (publickey) », le canal
    /// `auth-agent@openssh.com` rejeté (`ConnectFailed`) faute de joindre Pageant.
    ///
    /// On ne peut pas éprouver le vrai choix de transport sans Windows et un
    /// Pageant vivant (ce serait un test bout en bout) ; on verrouille ici la
    /// source unique de l'ordre d'essai que suit `ouvrir_agent_local`. Sans le
    /// correctif, cette liste ne contenait que le tube OpenSSH et l'assertion
    /// échoue.
    #[test]
    fn la_redirection_d_agent_windows_essaie_pageant_apres_le_tube_openssh() {
        assert_eq!(
            ORDRE_TRANSPORTS_AGENT_WINDOWS,
            &[
                TransportAgentWindows::TubeOpenSsh,
                TransportAgentWindows::Pageant,
            ],
            "la redirection doit essayer les deux transports, dans le même ordre que l'auth"
        );
    }
}

#[cfg(test)]
mod tests_marqueurs {
    use super::marqueur_bloquant_dans;

    /// `ssh(1)` refuse catégoriquement une clé marquée `@revoked`. russh, lui,
    /// découpe la ligne en hôte « @revoked » — qui ne correspond à rien — et
    /// rend une liste vide : verdict « premier contact », clé révoquée
    /// réapprise et acceptée sans un mot.
    #[test]
    fn une_cle_revoquee_est_signalee() {
        let c = "@revoked srv.exemple.com ssh-ed25519 AAAA\n";
        assert_eq!(
            marqueur_bloquant_dans(c, "srv.exemple.com").as_deref(),
            Some("@revoked")
        );
    }

    /// Une autorité de certification, que nous ne savons pas valider non plus.
    #[test]
    fn une_autorite_de_certification_est_signalee() {
        let c = "@cert-authority *.interne,srv.exemple.com ssh-rsa AAAA\n";
        assert_eq!(
            marqueur_bloquant_dans(c, "srv.exemple.com").as_deref(),
            Some("@cert-authority")
        );
    }

    #[test]
    fn un_hote_sans_marqueur_ne_bloque_rien() {
        let c = "@revoked autre.exemple.com ssh-ed25519 AAAA\n\
                 srv.exemple.com ssh-ed25519 BBBB\n";
        assert_eq!(marqueur_bloquant_dans(c, "srv.exemple.com"), None);
    }

    #[test]
    fn un_fichier_ordinaire_ne_bloque_rien() {
        for c in [
            "",
            "srv ssh-ed25519 AAAA\n",
            "# commentaire\n",
            "@inconnu srv k v\n",
        ] {
            assert_eq!(marqueur_bloquant_dans(c, "srv"), None, "{c:?}");
        }
    }

    /// La forme crochetée `[hôte]:port` (port non standard, écrite par OpenSSH)
    /// doit être reconnue comme visant l'hôte, tout comme la forme nue et la
    /// forme `hôte:port` sans crochets. Trouvé par l'audit du 7 septembre 2026 :
    /// seule la forme non crochetée l'était, la clé `@revoked` d'un serveur sur
    /// un port non standard était donc réapprise.
    #[test]
    fn la_forme_avec_port_est_reconnue() {
        for c in [
            "@revoked [srv.exemple.com]:2222 ssh-ed25519 AAAA\n",
            "@revoked srv.exemple.com:2222 ssh-ed25519 AAAA\n",
            "@revoked autre,[srv.exemple.com]:2222 ssh-ed25519 AAAA\n",
        ] {
            assert_eq!(
                marqueur_bloquant_dans(c, "srv.exemple.com").as_deref(),
                Some("@revoked"),
                "{c:?}"
            );
        }
    }

    /// Un IPv6 littéral nu ne doit pas être tronqué à son premier `:` (sinon un
    /// `@revoked` le visant ne serait jamais reconnu, ou le serait à tort pour
    /// un autre hôte).
    #[test]
    fn un_ipv6_litteral_n_est_pas_tronque() {
        let c = "@revoked 2001:db8::1 ssh-ed25519 AAAA\n";
        assert_eq!(
            marqueur_bloquant_dans(c, "2001:db8::1").as_deref(),
            Some("@revoked")
        );
        assert_eq!(marqueur_bloquant_dans(c, "2001"), None);
    }
}

#[cfg(test)]
mod tests_texte_distant {
    use super::{AvashSession, HOST_KEY_CHANGED, PASSWORD_REQUIRED};

    /// Trouvé par l'audit du 9 septembre 2026 (constat critique). Le texte de
    /// l'invite `keyboard-interactive` vient du SERVEUR et était recopié tel
    /// quel dans le message d'erreur. Or l'interface décide d'un geste lourd,
    /// proposer d'OUBLIER la clé d'hôte mémorisée, sur la simple présence de
    /// `[AVASH_HOST_KEY_CHANGED]` quelque part dans ce message (`isHostKeyChanged`,
    /// web/filters.ts). Un serveur hostile n'avait donc qu'à poser une invite
    /// contenant ce marqueur pour faire proposer à l'utilisateur d'effacer la
    /// confiance TOFU d'un hôte dont la clé n'avait pas changé, et ouvrir la
    /// voie à une interception silencieuse à la connexion suivante. Le marqueur
    /// est un canal de contrôle entre le cœur et l'interface : rien qui vienne
    /// du réseau ne doit pouvoir l'écrire.
    #[test]
    fn une_invite_hostile_ne_peut_pas_forger_un_marqueur() {
        for hostile in [
            "[AVASH_HOST_KEY_CHANGED] Le service a migré, confirmez pour continuer",
            "Code : [AVASH_PASSWORD_REQUIRED]",
            "a[AVASH_ANNULE]b",
        ] {
            let message = AvashSession::message_prompt_non_supporte(hostile);
            assert!(
                !message.contains(HOST_KEY_CHANGED),
                "une invite du serveur ne doit pas pouvoir forger {HOST_KEY_CHANGED} : {message}"
            );
            assert!(
                !message.contains(PASSWORD_REQUIRED),
                "une invite du serveur ne doit pas pouvoir forger {PASSWORD_REQUIRED} : {message}"
            );
            assert!(
                !message.contains("[AVASH_"),
                "aucun marqueur interne ne doit survivre à l'invite : {message}"
            );
        }
    }

    /// Même invite hostile, autre effet : les séquences d'échappement ANSI
    /// atteignaient le terminal xterm.js par le message d'erreur (le front
    /// écrit `why` tel quel). Un serveur pouvait donc repeindre l'écran de
    /// l'utilisateur, effacer l'alerte affichée au-dessus ou imiter une invite
    /// locale. On neutralise à la source, là où le texte entre.
    #[test]
    fn une_invite_hostile_ne_peut_pas_piloter_le_terminal() {
        let message = AvashSession::message_prompt_non_supporte(
            "\u{1b}[2J\u{1b}[HConnexion sûre\u{7}\r\nsuite",
        );
        assert!(
            !message.contains('\u{1b}'),
            "aucun ESC ne doit subsister : {message:?}"
        );
        assert!(
            !message.contains('\u{7}'),
            "aucun BEL ne doit subsister : {message:?}"
        );
        assert!(
            !message.contains('\n') && !message.contains('\r'),
            "l'invite ne doit pas pouvoir insérer de nouvelle ligne : {message:?}"
        );
    }

    /// Une invite immense noierait le message utile et le journal. On borne,
    /// en le disant plutôt qu'en tronquant en silence.
    #[test]
    fn une_invite_demesuree_est_bornee() {
        let message = AvashSession::message_prompt_non_supporte(&"A".repeat(10_000));
        assert!(
            message.len() < 600,
            "le message doit rester lisible, longueur {}",
            message.len()
        );
        assert!(
            message.contains('…'),
            "la troncature doit se voir : {message}"
        );
    }

    /// Le cas normal ne doit pas être abîmé : une invite ordinaire reste
    /// lisible, accents compris.
    #[test]
    fn une_invite_ordinaire_reste_intacte() {
        let message = AvashSession::message_prompt_non_supporte("  Code à usage unique :  ");
        assert!(
            message.contains("« Code à usage unique : »"),
            "l'invite normale doit rester telle quelle : {message}"
        );
    }
}

#[cfg(test)]
mod tests_known_hosts_illisible {
    use super::{
        apprendre_cle_hote, fichier_present_mais_illisible, marqueur_bloquant,
        verdict_known_hosts_illisible,
    };

    /// Trouvé par l'audit du 7 septembre 2026 : quand `~/.ssh` (ou
    /// `known_hosts`) n'est pas inscriptible, `learn_known_hosts_path` échoue en
    /// EACCES au premier contact. C'était le SEUL refus de `check_server_key`
    /// sans verdict posé, donc `connect` remontait le « Unknown server key » brut
    /// de russh, sans nommer la cause. Le verdict doit désormais citer
    /// `known_hosts` et l'échec d'enregistrement.
    ///
    /// Répertoire scratch en 0o500, et non le HOME virtuel partagé du processus,
    /// dont le `.ssh` sert aux autres tests parallèles. `#[cfg(unix)]` : sous
    /// Windows un répertoire en lecture seule n'empêche pas la création d'un
    /// fichier.
    #[test]
    #[cfg(unix)]
    fn un_known_hosts_inecrivable_donne_un_verdict_clair() {
        use russh::keys::{Algorithm, PrivateKey};
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::testutil::temp_home();
        let repertoire = dir.dir().join("ssh-lecture-seule");
        std::fs::create_dir(&repertoire).unwrap();
        let chemin = repertoire.join("known_hosts");
        std::fs::set_permissions(&repertoire, std::fs::Permissions::from_mode(0o500)).unwrap();
        // Régression vue en CI GitLab (voir `un_fichier_aux_droits_retires...`) :
        // en root (conteneur de l'exécuteur), CAP_DAC_OVERRIDE ignore les droits
        // et l'écriture réussirait — le cas n'existe pas pour lui, on le constate
        // plutôt que d'exiger une erreur que le noyau ne produira pas.
        let droits_appliques = std::fs::File::create(repertoire.join("sonde")).is_err();
        let cle = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)
            .unwrap()
            .public_key()
            .clone();
        let resultat = apprendre_cle_hote(&chemin, "srv", 22, &cle);
        // Restaurer les droits pour que le nettoyage du répertoire temporaire
        // puisse retirer son contenu.
        std::fs::set_permissions(&repertoire, std::fs::Permissions::from_mode(0o700)).unwrap();
        if !droits_appliques {
            eprintln!("droits non appliqués (root ?) : cas sans objet ici");
            return;
        }
        let message = resultat.expect_err("l'écriture dans un répertoire 0o500 doit échouer");
        assert!(
            message.contains("known_hosts") && message.contains("enregistrer"),
            "le verdict doit nommer known_hosts et l'échec d'enregistrement : {message}"
        );
    }

    /// russh rend une liste vide dès qu'il n'arrive pas à ouvrir le fichier —
    /// ce que le reste du code prendrait pour « hôte inconnu », donc pour un
    /// premier contact : **n'importe quelle clé serait acceptée et apprise**.
    /// Ce garde n'avait aucun test.
    #[test]
    fn un_fichier_absent_n_est_pas_un_probleme() {
        let dir = crate::testutil::temp_home();
        assert!(!fichier_present_mais_illisible(
            &dir.dir().join("jamais-cree")
        ));
    }

    #[test]
    fn un_fichier_lisible_n_est_pas_un_probleme() {
        let dir = crate::testutil::temp_home();
        let p = dir.dir().join("known_hosts");
        std::fs::write(&p, "srv ssh-ed25519 AAAA\n").unwrap();
        assert!(!fichier_present_mais_illisible(&p));
    }

    /// Droits retirés : présent, mais impossible à ouvrir.
    #[test]
    #[cfg(unix)]
    fn un_fichier_aux_droits_retires_est_signale() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::testutil::temp_home();
        let p = dir.dir().join("known_hosts");
        std::fs::write(&p, "srv ssh-ed25519 AAAA\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Régression vue en CI GitLab : dans le conteneur de l'exécuteur, les
        // tests tournent en root, qui ouvre un fichier 0o000 sans broncher
        // (CAP_DAC_OVERRIDE). Le cas n'existe pas pour lui : on le constate
        // plutôt que d'exiger une erreur que le noyau ne produira pas.
        let droits_appliques = std::fs::File::open(&p).is_err();
        let verdict = fichier_present_mais_illisible(&p);
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        if !droits_appliques {
            eprintln!("droits non appliqués (root ?) : cas sans objet ici");
            return;
        }
        assert!(verdict, "un known_hosts illisible doit être signalé");
    }

    /// Remplacé par un répertoire : `exists()` est vrai, l'ouverture échoue.
    #[test]
    fn un_repertoire_a_la_place_du_fichier_est_signale() {
        let dir = crate::testutil::temp_home();
        let p = dir.dir().join("known_hosts");
        std::fs::create_dir(&p).unwrap();
        // Sous Unix, ouvrir un répertoire réussit ; c'est la LECTURE qui échoue.
        // La fonction doit donc le signaler comme « présent mais illisible »,
        // pour qu'un known_hosts remplacé par un répertoire refuse la connexion
        // au lieu de la traiter comme un premier contact.
        assert!(p.exists());
        assert!(
            fichier_present_mais_illisible(&p),
            "un répertoire à la place du fichier doit être signalé"
        );
    }

    /// Trouvé par l'audit du 9 septembre 2026 : le code défendait déjà le cas
    /// « `known_hosts` remplacé par un répertoire » (l'ouverture réussit, la
    /// lecture échoue tout de suite) mais pas celui d'un fichier spécial
    /// BLOQUANT. Sous Unix, ouvrir un tube nommé en lecture seule sans écrivain
    /// suspend l'appelant dans le noyau, indéfiniment : `check_server_key` étant
    /// un handler async exécuté sur le runtime tokio, un `~/.ssh/known_hosts`
    /// devenu tube (répertoire personnel abîmé, script de sauvegarde qui laisse
    /// un `mkfifo` là) gelait l'onglet de connexion sans erreur ni délai.
    ///
    /// Le fil séparé n'est pas de la décoration : sans lui, l'échec serait un
    /// test qui ne rend jamais la main plutôt qu'un test rouge.
    #[test]
    #[cfg(unix)]
    fn un_tube_nomme_a_la_place_du_fichier_est_signale_sans_bloquer() {
        let dir = crate::testutil::temp_home();
        let p = dir.dir().join("known_hosts");
        creer_tube_nomme(&p);
        assert!(p.exists(), "le tube nommé doit être vu comme présent");
        let cible = p.clone();
        let verdict = sous_delai(move || fichier_present_mais_illisible(&cible))
            .expect("l'inspection d'un known_hosts en tube nommé ne doit pas se bloquer");
        assert!(
            verdict,
            "un tube nommé à la place du fichier doit être signalé comme illisible"
        );
    }

    /// Même cas, par l'autre porte : `marqueur_bloquant` est appelé AVANT
    /// `known_hosts_illisible` dans `check_server_key`, et lisait le fichier
    /// entier. C'est donc lui qui se bloquait le premier sur un tube nommé.
    /// Trouvé par l'audit du 9 septembre 2026.
    #[test]
    #[cfg(unix)]
    fn la_recherche_de_marqueur_ne_se_bloque_pas_sur_un_tube_nomme() {
        let dir = crate::testutil::temp_home();
        let ssh = dir.dir().join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        creer_tube_nomme(&ssh.join("known_hosts"));
        let marqueur = sous_delai(|| marqueur_bloquant("srv.exemple.com"))
            .expect("la recherche de marqueur ne doit pas se bloquer sur un tube nommé");
        assert_eq!(
            marqueur, None,
            "un fichier illisible ne porte aucun marqueur : c'est le garde \
             « illisible » qui doit refuser la connexion, pas celui-ci"
        );
    }

    /// Rend le résultat de `travail`, ou `None` s'il n'a pas rendu la main dans
    /// le délai. Le fil resté bloqué est abandonné : le processus de test
    /// l'emportera en sortant.
    #[cfg(unix)]
    /// Réserve de la relecture du 9 septembre 2026 : le verdict disait « n'est
    /// pas lisible » aussi bien pour un fichier aux droits retirés que pour un
    /// tube nommé ou un `known_hosts` pointé sur `/dev/null`, alors que le geste
    /// de réparation n'est pas le même. Le verdict nomme désormais la cause.
    #[test]
    #[cfg(unix)]
    fn le_verdict_nomme_un_known_hosts_qui_n_est_pas_un_fichier_ordinaire() {
        let dir = crate::testutil::temp_home();
        let p = dir.dir().join("known_hosts");
        creer_tube_nomme(&p);
        let cible = p.clone();
        let verdict = sous_delai(move || verdict_known_hosts_illisible(&cible))
            .expect("le verdict d'un tube nommé ne doit pas se bloquer");
        let message =
            verdict.expect("un tube nommé à la place du fichier doit produire un verdict");
        assert!(
            message.contains("n'est pas un fichier ordinaire"),
            "le verdict doit nommer la cause : {message}"
        );
    }

    /// Même verdict, autre cause : un fichier ordinaire dont les droits ont été
    /// retirés. Le message parle de droits, pas de fichier spécial.
    #[test]
    #[cfg(unix)]
    fn le_verdict_distingue_un_fichier_ordinaire_aux_droits_retires() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::testutil::temp_home();
        let p = dir.dir().join("known_hosts");
        std::fs::write(&p, "srv ssh-ed25519 AAAA\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).unwrap();
        // En root (conteneur de CI), CAP_DAC_OVERRIDE ignore les droits : le cas
        // n'existe pas pour lui, on le constate au lieu d'exiger l'impossible.
        let droits_appliques = std::fs::File::open(&p).is_err();
        let verdict = verdict_known_hosts_illisible(&p);
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        if !droits_appliques {
            eprintln!("droits non appliqués (root ?) : cas sans objet ici");
            return;
        }
        let message = verdict.expect("un fichier aux droits retirés doit produire un verdict");
        assert!(
            message.contains("droits"),
            "le verdict doit parler des droits : {message}"
        );
        assert!(
            !message.contains("fichier ordinaire"),
            "un fichier ordinaire ne doit pas être décrit comme spécial : {message}"
        );
    }

    /// Absent (premier contact) ou lisible : aucun verdict, la vérification
    /// suit son cours.
    #[test]
    fn aucun_verdict_pour_un_known_hosts_absent_ou_lisible() {
        let dir = crate::testutil::temp_home();
        let p = dir.dir().join("known_hosts");
        assert!(verdict_known_hosts_illisible(&p).is_none());
        std::fs::write(&p, "").unwrap();
        assert!(verdict_known_hosts_illisible(&p).is_none());
    }

    fn sous_delai<T: Send + 'static>(travail: impl FnOnce() -> T + Send + 'static) -> Option<T> {
        let (envoi, reception) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = envoi.send(travail());
        });
        reception
            .recv_timeout(std::time::Duration::from_secs(5))
            .ok()
    }

    /// `mkfifo(3)` : il n'y a pas de tube nommé dans la bibliothèque standard.
    #[cfg(unix)]
    fn creer_tube_nomme(chemin: &std::path::Path) {
        use std::os::unix::ffi::OsStrExt as _;
        let brut = std::ffi::CString::new(chemin.as_os_str().as_bytes()).unwrap();
        let code = unsafe { libc::mkfifo(brut.as_ptr(), 0o600) };
        assert_eq!(code, 0, "mkfifo a échoué sur {}", chemin.display());
    }
}

#[cfg(test)]
mod tests_cle_hote {
    use super::{juger_cle_hote, VerdictCle};
    use russh::keys::{Algorithm, PrivateKey, PublicKey};

    fn cle(algo: Algorithm) -> PublicKey {
        PrivateKey::random(&mut rand::rng(), algo)
            .unwrap()
            .public_key()
            .clone()
    }

    #[test]
    fn rien_d_enregistre_donne_un_premier_contact() {
        let presentee = cle(Algorithm::Ed25519);
        assert_eq!(juger_cle_hote(&[], &presentee), VerdictCle::PremierContact);
    }

    #[test]
    fn la_meme_cle_est_reconnue() {
        let k = cle(Algorithm::Ed25519);
        assert_eq!(juger_cle_hote(&[(3, k.clone())], &k), VerdictCle::Connue);
    }

    #[test]
    fn une_autre_cle_du_meme_algorithme_est_un_changement() {
        let enregistree = cle(Algorithm::Ed25519);
        let presentee = cle(Algorithm::Ed25519);
        assert_eq!(
            juger_cle_hote(&[(7, enregistree)], &presentee),
            VerdictCle::Changee { ligne: 7 }
        );
    }

    /// Le cœur du correctif : un intercepteur qui annonce un AUTRE algorithme
    /// ne doit pas être pris pour un premier contact. C'est exactement ce que
    /// `check_known_hosts` de russh laissait passer — il répond « hôte inconnu »
    /// dès que l'algorithme diffère, ce qui faisait apprendre la clé en silence
    /// puis envoyer le mot de passe.
    #[test]
    fn une_cle_d_un_autre_algorithme_est_un_changement_pas_un_premier_contact() {
        let enregistree = cle(Algorithm::Ed25519);
        let presentee = cle(Algorithm::Rsa { hash: None });
        let verdict = juger_cle_hote(&[(2, enregistree)], &presentee);
        assert_ne!(
            verdict,
            VerdictCle::PremierContact,
            "un algorithme différent ne doit JAMAIS passer pour un premier contact"
        );
        assert_eq!(verdict, VerdictCle::Changee { ligne: 2 });
    }

    #[test]
    fn une_correspondance_parmi_plusieurs_suffit() {
        // Un hôte peut légitimement publier plusieurs clés (une par algorithme).
        let a = cle(Algorithm::Ed25519);
        let b = cle(Algorithm::Ed25519);
        assert_eq!(
            juger_cle_hote(&[(1, a), (2, b.clone())], &b),
            VerdictCle::Connue
        );
    }
}

#[cfg(test)]
mod tests_garde_agent {
    use super::{GardeAgent, RelaisAgent};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    /// Trouvé par l'audit du 7 septembre 2026 : un canal `auth-agent@openssh.com`
    /// ouvert par le serveur PENDANT `run_avec_agent`, puis gardé ouvert, faisait
    /// survivre son relais vers l'agent du poste à la fin de la commande. Le
    /// `Drop` de `GardeAgent` ne remettait que le drapeau à false sans interrompre
    /// les relais déjà lancés : le serveur pouvait continuer à faire signer
    /// l'agent du poste pour toute la durée de l'onglet, alors que SECURITY.md
    /// promet une borne à la seule durée de la commande. On vérifie ici que la
    /// chute de la garde interrompt bien un relais encore vivant (aucun EOF de
    /// part et d'autre) — ce qui, en vrai, lâche le flux du canal et envoie
    /// `Close` au serveur.
    ///
    /// Test unitaire sur le mécanisme corrigé plutôt qu'intégration : le relais
    /// n'est lancé que si `ouvrir_agent_local` joint un agent, or `SSH_AUTH_SOCK`
    /// est un chemin de process global que toute la suite pointe volontairement
    /// sur un socket absent (pour un verdict `ConnectFailed` déterministe et
    /// partagé, cf. `un_canal_d_agent_hors_commande...`) ; y brancher un vrai
    /// agent le temps d'un seul test ferait courir les tests parallèles derrière
    /// son dos. On éprouve donc directement ce que corrige le défaut : la garde
    /// possède les relais et les interrompt à sa chute.
    #[tokio::test]
    async fn la_garde_interrompt_les_relais_d_agent_a_la_fin_de_la_commande() {
        let drapeau = Arc::new(AtomicBool::new(true));
        let relais: RelaisAgent = Arc::new(std::sync::Mutex::new(Vec::new()));

        // Un relais qui ne se termine jamais seul : deux tubes en mémoire dont
        // les pairs sont gardés ouverts (aucun EOF), comme un canal d'agent que
        // le serveur laisse ouvert. Sans interruption, la tâche vit indéfiniment.
        //
        // `tx` est capturé par la tâche : tant que le relais tourne, il le garde
        // vivant. Dès que la tâche est interrompue (son futur lâché), `tx` tombe
        // et le récepteur se résout — peu importe qu'il rende Ok ou une erreur,
        // c'est la RÉSOLUTION qui prouve la fin du relais. Un relais non
        // interrompu garderait `tx` et le récepteur ne se résoudrait jamais : le
        // `timeout` ci-dessous échouerait, ce qui est exactement le défaut.
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let (mut a, _pair_a) = tokio::io::duplex(64);
        let (mut b, _pair_b) = tokio::io::duplex(64);
        let handle = tokio::spawn(async move {
            let _tx = tx;
            let _ = tokio::io::copy_bidirectional(&mut a, &mut b).await;
        });
        relais.lock().unwrap().push(handle);

        // Fin de la commande : la garde tombe. Elle doit abaisser le drapeau ET
        // interrompre le relais encore vivant.
        let garde = GardeAgent {
            drapeau: drapeau.clone(),
            relais: relais.clone(),
        };
        drop(garde);

        assert!(
            !drapeau.load(Ordering::Relaxed),
            "la garde doit abaisser le drapeau de redirection d'agent"
        );
        tokio::time::timeout(Duration::from_secs(2), rx)
            .await
            .expect(
                "le relais doit être interrompu à la chute de la garde, pas survivre à la commande",
            )
            .ok();
        assert!(
            relais.lock().unwrap().is_empty(),
            "la garde doit vider la liste des relais pour ne pas les accumuler"
        );
    }
}
