//! Avash — sidecar client RDP (IronRDP), isolé de russh.
//!
//! Sert le bureau distant à Avash via un **WebSocket local binaire** (vrai
//! ArrayBuffer côté webview : pas de base64, pas de JSON — débit maximal, même
//! en 3440×1440). Écoute sur 127.0.0.1:<port aléatoire> et n'accepte qu'un
//! client présentant le bon jeton. Imprime « PORT TOKEN » sur stdout au départ.
//!
//! Messages WebSocket (binaires, auto-délimités) :
//!   sidecar -> app : [1]=CONNECTED w:u16 h:u16 · [2]=FRAME x,y,w,h:u16 + RGBA
//!                     · [7]=STATS fps:u16 kbps:u32 lat:u16 · [8]=CLIPBOARD utf8
//!                     ([3]=ERROR est réservé et géré côté front, mais nous ne
//!                      l'émettons pas : un échec avant connexion sort sur
//!                      stderr, un échec en session ferme le WebSocket et le
//!                      diagnostic est relu par `rdp_diagnostic`)
//!   app -> sidecar : [1]MOUSE_MOVE x,y · [2]BUTTON b,down,x,y · [3]WHEEL delta:i16
//!                     · [4]KEY sc:u16,down · [5]RESIZE w,h · [6]ACK · [8]CLIPBOARD utf8
//!                     · [9]REFRESH · [10]LOCKS bits:u8 · [11]PAUSE pause:u8
//!                     · [12]CLIPBOARD_AUTORISE autorise:u8
//!
//! Usage : avash-rdp --host H [--port 3389] -u USER -p PASS [--width W --height H] [--domain D] [--shot out.png] [--layout fr]

// Lints stylistiques assumés pour ce petit binaire d'orchestration :
// noms de produits en prose (doc_markdown), main() qui séquence tout le
// flux (too_many_lines), et coordonnées/RGBA aux noms courts idiomatiques.
#![allow(
    clippy::doc_markdown,
    clippy::too_many_lines,
    clippy::many_single_char_names
)]

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use ironrdp::cliprdr::backend::CliprdrBackend;
use ironrdp::cliprdr::pdu::{
    ClipboardFormat, ClipboardFormatId, ClipboardGeneralCapabilityFlags, FileContentsRequest,
    FileContentsResponse, FormatDataRequest, FormatDataResponse, LockDataId,
    OwnedFormatDataResponse,
};
use ironrdp::cliprdr::CliprdrClient;
use ironrdp::connector::connection_activation::ConnectionActivationState;
use ironrdp::connector::{self, Credentials};
use ironrdp::core::IntoOwned;
use ironrdp::core::WriteBuf;
use ironrdp::displaycontrol::client::DisplayControlClient;
use ironrdp::displaycontrol::pdu::MonitorLayoutEntry;
use ironrdp::dvc::DrdynvcClient;
use ironrdp::graphics::image_processing::PixelFormat;
use ironrdp::input::{Database, MousePosition, Operation, WheelRotations};
use ironrdp::pdu::gcc::KeyboardType;
use ironrdp::pdu::geometry::InclusiveRectangle;
use ironrdp::pdu::rdp::capability_sets::MajorPlatformType;
use ironrdp::pdu::rdp::client_info::{PerformanceFlags, TimezoneInfo};
use ironrdp::session::image::DecodedImage;
use ironrdp::session::{ActiveStage, ActiveStageBuilder, ActiveStageOutput};
use ironrdp_tokio::single_sequence_step;
use ironrdp_tokio::FramedWrite as _;
use std::io::BufRead as _;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

mod atomique;
mod egfx;
mod magnetoscope;
mod progressif;
mod surface;

/// Presse-papiers local partagé (texte), alimenté par le front, servi au serveur.
type LocalClip = std::sync::Arc<std::sync::Mutex<Option<String>>>;

/// Requêtes du backend CLIPRDR vers la boucle principale. Le backend est
/// encapsulé dans l'ActiveStage et ne peut pas la rappeler : il passe par ce
/// canal, la boucle exécute l'action SVC correspondante.
#[derive(Debug)]
enum ClipReq {
    /// Annoncer au serveur qu'on a du texte (initiate_copy).
    Advertise,
    /// Servir des données réclamées par le serveur (submit_format_data).
    ServeData(OwnedFormatDataResponse),
    /// Réclamer au serveur les données d'un format (initiate_paste).
    RequestPaste(ClipboardFormatId),
    /// Texte reçu du serveur → à pousser vers le presse-papiers du poste.
    RemoteText(String),
}

/// Pont entre le canal CLIPRDR et le presse-papiers du poste (via le front).
/// Texte seulement (CF_UNICODETEXT).
#[derive(Debug)]
struct ClipBackend {
    local_text: LocalClip,
    tx: tokio::sync::mpsc::UnboundedSender<ClipReq>,
    /// Le partage de presse-papiers est-il autorisé ? Piloté par l'interface
    /// (message `[12]`), dans les **deux** sens : le réglage ne gardait que le
    /// sens sortant, alors qu'un bureau hostile pouvait remplacer en boucle le
    /// presse-papiers du poste — on copie une commande depuis sa documentation,
    /// on colle dans son terminal local, on exécute celle de l'attaquant.
    partage: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

ironrdp::core::impl_as_any!(ClipBackend);

impl ClipBackend {
    fn partage_actif(&self) -> bool {
        self.partage.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl CliprdrBackend for ClipBackend {
    #[allow(clippy::unnecessary_literal_bound)]
    fn temporary_directory(&self) -> &str {
        "."
    }
    fn client_capabilities(&self) -> ClipboardGeneralCapabilityFlags {
        ClipboardGeneralCapabilityFlags::empty()
    }
    fn on_ready(&mut self) {
        if self.partage_actif() && self.local_text.lock().is_ok_and(|t| t.is_some()) {
            let _ = self.tx.send(ClipReq::Advertise);
        }
    }
    fn on_request_format_list(&mut self) {
        if self.partage_actif() {
            let _ = self.tx.send(ClipReq::Advertise);
        }
    }
    fn on_process_negotiated_capabilities(&mut self, _caps: ClipboardGeneralCapabilityFlags) {}
    fn on_remote_copy(&mut self, formats: &[ClipboardFormat]) {
        if !self.partage_actif() {
            return; // on ne réclame même pas les données au serveur
        }
        if formats
            .iter()
            .any(|f| f.id == ClipboardFormatId::CF_UNICODETEXT)
        {
            let _ = self
                .tx
                .send(ClipReq::RequestPaste(ClipboardFormatId::CF_UNICODETEXT));
        }
    }
    fn on_format_data_request(&mut self, req: FormatDataRequest) {
        let resp = if self.partage_actif() && req.format == ClipboardFormatId::CF_UNICODETEXT {
            match self.local_text.lock().ok().and_then(|t| t.clone()) {
                Some(text) => FormatDataResponse::new_unicode_string(&text).into_owned(),
                None => FormatDataResponse::new_error().into_owned(),
            }
        } else {
            FormatDataResponse::new_error().into_owned()
        };
        let _ = self.tx.send(ClipReq::ServeData(resp));
    }
    fn on_format_data_response(&mut self, resp: FormatDataResponse<'_>) {
        if !resp.is_error() {
            if let Ok(text) = resp.to_unicode_string() {
                // Plafond anti-abus : un serveur ne sature pas la mémoire via un
                // presse-papiers géant (le texte normal reste très en dessous).
                if text.len() <= 8 * 1024 * 1024 {
                    let _ = self.tx.send(ClipReq::RemoteText(text));
                }
            }
        }
    }
    fn on_file_contents_request(&mut self, _req: FileContentsRequest) {}
    fn on_file_contents_response(&mut self, _resp: FileContentsResponse<'_>) {}
    fn on_lock(&mut self, _id: LockDataId) {}
    fn on_unlock(&mut self, _id: LockDataId) {}
}

/// Filet anti-gel : si un ACK de rendu se perd, on renvoie l'état courant
/// passé ce délai plutôt que de figer l'affichage.
const ACK_TIMEOUT: Duration = Duration::from_millis(250);

struct Args {
    host: String,
    port: u16,
    user: String,
    pass: String,
    domain: Option<String>,
    /// L'utilisateur a accepté de se passer de NLA pour ce serveur.
    sans_nla: bool,
    layout: u32,
    enregistrer: Option<String>,
    width: u16,
    height: u16,
    shot: Option<String>,
}

struct Pa(Vec<String>);
impl Pa {
    fn opt(&self, k: &str) -> Option<String> {
        self.0
            .iter()
            .position(|a| a == k)
            .and_then(|i| self.0.get(i + 1).cloned())
    }
    fn drapeau(&self, k: &str) -> bool {
        self.0.iter().any(|a| a == k)
    }
    fn req2(&self, k1: &str, k2: &str) -> Result<String> {
        self.opt(k1)
            .or_else(|| self.opt(k2))
            .with_context(|| format!("argument requis : {k1}/{k2}"))
    }
}

/// Mot de passe : depuis `-p/--password` s'il est fourni (utile pour `--shot`),
/// sinon lu sur la première ligne de stdin (le parent le transmet ainsi pour
/// ne pas l'exposer dans /proc/<pid>/cmdline).
fn read_password(a: &Pa) -> Result<String> {
    if let Some(p) = a.opt("-p").or_else(|| a.opt("--password")) {
        return Ok(p);
    }
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .context("lecture du mot de passe sur stdin")?;
    Ok(line.trim_end_matches(['\n', '\r']).to_string())
}

fn parse_args() -> Result<Args> {
    let a = Pa(std::env::args().skip(1).collect());
    let pass = read_password(&a)?;
    parse_args_de_pa(&a, pass)
}

/// Variante testable : les arguments et le mot de passe sont fournis, plutôt
/// que lus dans l'environnement et sur l'entrée standard.
#[cfg(test)]
fn parse_args_de(args: &[&str], pass: &str) -> Result<Args> {
    let pa = Pa(args.iter().map(|s| (*s).to_owned()).collect());
    parse_args_de_pa(&pa, pass.to_owned())
}

fn parse_args_de_pa(a: &Pa, pass: String) -> Result<Args> {
    Ok(Args {
        host: a.opt("--host").context("argument requis : --host")?,
        port: a.opt("--port").and_then(|s| s.parse().ok()).unwrap_or(3389),
        user: a.req2("-u", "--username")?,
        pass,
        domain: a.opt("--domain"),
        sans_nla: a.drapeau("--sans-nla"),
        layout: a
            .opt("--layout")
            .and_then(|v| analyser_disposition(&v))
            .unwrap_or_else(disposition_detectee),
        width: a
            .opt("--width")
            .and_then(|s| s.parse().ok())
            .unwrap_or(1280),
        height: a
            .opt("--height")
            .and_then(|s| s.parse().ok())
            .unwrap_or(800),
        shot: a.opt("--shot"),
        enregistrer: a.opt("--enregistrer"),
    })
}

/// Sépare un domaine éventuellement collé au nom d'utilisateur.
/// NLA/CredSSP attend le domaine à part : « DOMAINE\\user » ou « user@domaine »
/// sont acceptés par les utilisateurs, on les découpe ici. `--domain` explicite
/// est prioritaire (le nom est alors laissé intact).
fn split_credentials(user: &str, explicit_domain: Option<&str>) -> (String, Option<String>) {
    if let Some(d) = explicit_domain {
        return (user.to_string(), Some(d.to_string()));
    }
    if let Some((dom, name)) = user.split_once('\\') {
        return (name.to_string(), Some(dom.to_string()));
    }
    if let Some((name, dom)) = user.split_once('@') {
        return (name.to_string(), Some(dom.to_string()));
    }
    (user.to_string(), None)
}

/// Identifiant RDP de disposition clavier pour un code XKB (« fr », « de »…).
///
/// RDP transporte des **scancodes**, pas des caractères : c'est le serveur qui
/// les traduit, d'après la disposition que le client annonce. En annonçant 0,
/// avash laissait le serveur choisir — en pratique l'américain. Sur un clavier
/// AZERTY, taper « a » produisait « q ». Signalé par Adrien sur SLED-15.
///
/// Windows ne s'en plaignait pas : il rend `0` par son propre défaut, souvent
/// aligné sur la session. xrdp, lui, retombe sur l'américain.
fn disposition_pour_code(code: &str) -> Option<u32> {
    // Identifiants Microsoft (« Keyboard Identifiers »).
    Some(match code.split([',', '(']).next()?.trim() {
        "fr" => 0x0000_040C,
        "be" => 0x0000_080C,
        "ca" => 0x0000_0C0C,
        "ch" => 0x0000_100C,
        "de" => 0x0000_0407,
        "at" => 0x0000_0C07,
        "us" => 0x0000_0409,
        "gb" | "uk" => 0x0000_0809,
        "es" => 0x0000_040A,
        "it" => 0x0000_0410,
        "pt" => 0x0000_0816,
        "br" => 0x0000_0416,
        "nl" => 0x0000_0413,
        "dk" => 0x0000_0406,
        "no" => 0x0000_0414,
        "se" => 0x0000_041D,
        "fi" => 0x0000_040B,
        "pl" => 0x0000_0415,
        "cz" => 0x0000_0405,
        "ru" => 0x0000_0419,
        "tr" => 0x0000_041F,
        "jp" => 0x0000_0411,
        _ => return None,
    })
}

/// Accepte un identifiant numérique (« 0x40c », « 1036 ») ou un code (« fr »).
fn analyser_disposition(v: &str) -> Option<u32> {
    let v = v.trim();
    if let Some(hex) = v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
        return u32::from_str_radix(hex, 16).ok();
    }
    if let Ok(n) = v.parse::<u32>() {
        return Some(n);
    }
    disposition_pour_code(v)
}

/// Disposition du poste, ou 0 si on ne sait pas — mieux vaut le défaut du
/// serveur qu'une disposition inventée.
fn disposition_detectee() -> u32 {
    if let Some(v) = std::env::var_os("AVASH_RDP_LAYOUT")
        .and_then(|v| v.into_string().ok())
        .and_then(|v| analyser_disposition(&v))
    {
        return v;
    }
    #[cfg(unix)]
    {
        if let Some(v) = std::env::var_os("XKB_DEFAULT_LAYOUT")
            .and_then(|v| v.into_string().ok())
            .and_then(|v| disposition_pour_code(&v))
        {
            return v;
        }
        // KDE garde la disposition de session ici, que localectl ignore.
        if let Some(v) = repertoire_configuration()
            .map(|c| c.join("kxkbrc"))
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| {
                t.lines()
                    .find_map(|l| l.strip_prefix("LayoutList="))
                    .and_then(disposition_pour_code)
            })
        {
            return v;
        }
        if let Some(v) = std::process::Command::new("localectl")
            .arg("status")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|t| {
                t.lines()
                    .find_map(|l| l.trim().strip_prefix("X11 Layout:"))
                    .and_then(disposition_pour_code)
            })
        {
            return v;
        }
    }
    #[cfg(windows)]
    {
        if let Some(v) = std::process::Command::new("reg")
            .args(["query", r"HKCU\Keyboard Layout\Preload", "/v", "1"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|t| {
                t.split_whitespace()
                    .last()
                    .and_then(|v| u32::from_str_radix(v, 16).ok())
            })
        {
            return v;
        }
    }
    0
}

/// Extrait la valeur d'un jeton de routage.
///
/// Le serveur envoie le jeton complet — `Cookie: msts=2464288595\r\n` — tandis
/// que la bibliothèque ajoute elle-même le préfixe et le terminateur. Le passer
/// tel quel produisait `Cookie: msts=Cookie: msts=…`, que le serveur refusait
/// en fermant la connexion sans un mot.
fn valeur_du_jeton(brut: &[u8]) -> String {
    String::from_utf8_lossy(brut)
        .trim_end_matches(['\r', '\n'])
        .trim_start_matches("Cookie: msts=")
        .to_owned()
}

fn build_config(
    a: &Args,
    redirection: Option<&ironrdp::session::redirection::Redirection>,
) -> connector::Config {
    let (username, domain) = split_credentials(&a.user, a.domain.as_deref());
    connector::Config {
        // Après une redirection, le serveur impose SES identifiants — engendrés
        // pour l'occasion — et non ceux de l'utilisateur. C'est ainsi que GNOME
        // remet la connexion d'un démon à l'autre.
        credentials: match redirection {
            Some(r) if r.utilisateur.is_some() => Credentials::UsernamePassword {
                username: r.utilisateur.clone().unwrap_or_default(),
                password: r
                    .mot_de_passe
                    .as_ref()
                    .map(|o| String::from_utf8_lossy(o).into_owned())
                    .unwrap_or_default(),
            },
            _ => Credentials::UsernamePassword {
                username,
                password: a.pass.clone(),
            },
        },
        domain,
        // `enable_tls` annonce PROTOCOL_SSL au serveur, ce qui — la
        // documentation d'ironrdp le dit mot pour mot — revient à **accepter le
        // repli de NLA vers TLS seul**. Un serveur qui répond « SSL » voyait
        // alors CredSSP sauté (connection.rs : « CredSSP is disabled, skipping
        // NLA ») et le mot de passe partait dans le Client Info PDU, sans
        // authentification mutuelle. C'est précisément au premier contact —
        // le seul moment où le TOFU ne protège pas — que cela coûte le plus.
        // En n'annonçant que HYBRID, un serveur incapable de NLA fait échouer
        // la négociation, ce qui est le bon comportement.
        //
        // `--sans-nla` rétablit l'annonce de SSL, **sur décision explicite de
        // l'utilisateur** et pour ce serveur-là seulement : certains serveurs
        // légitimes n'offrent pas NLA — un xrdp dont le module PAM n'est pas
        // configuré, par exemple. On annonce alors les deux, et le serveur
        // choisit : NLA reste préféré s'il sait le faire.
        enable_tls: a.sans_nla,
        enable_credssp: true,
        keyboard_type: KeyboardType::IbmEnhanced,
        keyboard_subtype: 0,
        keyboard_layout: a.layout,
        keyboard_functional_keys_count: 12,
        ime_file_name: String::new(),
        dig_product_id: String::new(),
        desktop_size: connector::DesktopSize {
            width: a.width,
            height: a.height,
        },
        bitmap: None,
        client_build: 0,
        client_name: "avash-rdp".to_owned(),
        client_dir: "C:\\Windows\\System32\\mstscax.dll".to_owned(),
        platform: MajorPlatformType::UNIX,
        enable_server_pointer: false,
        // Le jeton de routage réoriente la connexion vers la bonne session ;
        // sans lui, le serveur nous renverrait à l'accueil, indéfiniment.
        request_data: redirection
            .and_then(|r| r.jeton.as_deref())
            .map(valeur_du_jeton)
            .map(ironrdp::pdu::nego::NegoRequestData::routing_token),
        autologon: false,
        enable_audio_playback: false,
        compression_type: None,
        pointer_software_rendering: true,
        multitransport_flags: None,
        performance_flags: PerformanceFlags::default(),
        desktop_scale_factor: 0,
        hardware_id: None,
        license_cache: None,
        timezone_info: TimezoneInfo::default(),
        alternate_shell: String::new(),
        work_dir: String::new(),
    }
}

fn server_public_key(cert: &x509_cert::Certificate) -> Result<Vec<u8>> {
    cert.tbs_certificate
        .subject_public_key_info
        .subject_public_key
        .as_bytes()
        .context("clé publique non alignée")
        .map(<[u8]>::to_vec)
}

fn mouse_button(n: u8) -> ironrdp::input::MouseButton {
    use ironrdp::input::MouseButton::{Left, Middle, Right, X1, X2};
    match n {
        1 => Middle,
        2 => Right,
        3 => X1,
        4 => X2,
        _ => Left,
    }
}

/// Verdict d'un certificat de serveur RDP, au regard des empreintes mémorisées.
#[derive(Debug, PartialEq, Eq)]
pub enum VerdictCert {
    /// Rien de mémorisé pour cet hôte : premier contact.
    PremierContact,
    /// L'empreinte présentée correspond à celle mémorisée.
    Connu,
    /// Une empreinte est mémorisée, mais ce n'est pas celle-ci.
    Change { attendue: String },
}

/// Compare l'empreinte présentée à celle mémorisée pour cet hôte.
///
/// Même modèle que le `known_hosts` de SSH. Sans cela, `ironrdp_tls::upgrade`
/// accepte **n'importe quel** certificat (il installe `NoCertificateVerification`)
/// et l'on enchaîne sur CredSSP/NLA — c'est-à-dire qu'on livre les identifiants
/// à qui se présente. L'asymétrie avec le volet SSH était totale.
#[must_use]
pub fn juger_certificat(memorisee: Option<&str>, presentee: &str) -> VerdictCert {
    match memorisee {
        None => VerdictCert::PremierContact,
        Some(m) if m == presentee => VerdictCert::Connu,
        Some(m) => VerdictCert::Change {
            attendue: m.to_owned(),
        },
    }
}

/// Empreinte SHA-256 de la clé publique du serveur, en hexadécimal minuscule.
///
/// On épingle la clé plutôt que le certificat entier : une simple reconduction
/// du certificat, à clé inchangée, ne doit pas déclencher de fausse alerte.
fn empreinte(der: &[u8]) -> String {
    use sha2::Digest as _;
    let condense = sha2::Sha256::digest(der);
    condense.iter().fold(String::new(), |mut acc, o| {
        use std::fmt::Write as _;
        let _ = write!(acc, "{o:02x}");
        acc
    })
}

/// Fichier des empreintes mémorisées, à côté du reste de la configuration.
///
/// Répertoire de configuration, `AVASH_HOME` faisant foi s'il est posé.
///
/// Le cœur honore déjà cette variable ; ce processus, non — et l'écart ne se
/// voyait pas sous Linux, où `config_dir()` suit `XDG_CONFIG_HOME` que le bac à
/// sable des tests pose déjà. Sous Windows, `config_dir()` interroge le shell
/// et ignore aussi bien `HOME` que `XDG_CONFIG_HOME` : la suite bout en bout y
/// aurait écrit dans le fichier de confiance RÉEL de l'utilisateur, y semant
/// des serveurs de test et, pire, l'exposant à voir une empreinte légitime
/// écrasée par celle d'un serveur jetable.
fn repertoire_configuration() -> Option<std::path::PathBuf> {
    if let Some(home) = std::env::var_os("AVASH_HOME") {
        return Some(std::path::PathBuf::from(home).join(".config"));
    }
    dirs::config_dir()
}

/// Sans répertoire de configuration, on **échoue** au lieu de retomber sur le
/// répertoire courant : y semer un fichier de confiance le rendrait inopérant
/// au prochain lancement depuis ailleurs — chaque serveur redeviendrait un
/// premier contact, en silence.
/// Où l'on note les serveurs qui n'ont que le canal graphique pour dessiner.
fn chemin_canal_graphique() -> Option<std::path::PathBuf> {
    Some(
        repertoire_configuration()?
            .join("avash")
            .join("rdp_canal_graphique"),
    )
}

fn chemin_empreintes() -> anyhow::Result<std::path::PathBuf> {
    Ok(repertoire_configuration()
        .context("répertoire de configuration introuvable (HOME/XDG_CONFIG_HOME)")?
        .join("avash")
        .join("rdp_known_hosts"))
}

/// Empreinte mémorisée pour `hote:port`, s'il y en a une.
fn empreinte_memorisee(cle: &str) -> Option<String> {
    let contenu = std::fs::read_to_string(chemin_empreintes().ok()?).ok()?;
    chercher_empreinte(&contenu, cle)
}

/// Cherche l'empreinte de `cle` dans le contenu d'un fichier d'empreintes.
///
/// Séparée de la lecture pour être exerçable : c'est ici que se joue la
/// différence entre « ce serveur est connu » et « premier contact », donc entre
/// refuser un imposteur et l'accepter.
fn chercher_empreinte(contenu: &str, cle: &str) -> Option<String> {
    contenu.lines().find_map(|l| {
        let (h, e) = l.split_once(' ')?;
        (h == cle).then(|| e.trim().to_owned())
    })
}

/// Mémorise l'empreinte d'un hôte au premier contact.
fn memoriser_empreinte(cle: &str, empreinte: &str) -> anyhow::Result<()> {
    let chemin = chemin_empreintes()?;
    if let Some(dir) = chemin.parent() {
        std::fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
    }
    let mut contenu = std::fs::read_to_string(&chemin).unwrap_or_default();
    if !contenu.is_empty() && !contenu.ends_with('\n') {
        contenu.push('\n');
    }
    contenu.push_str(&format!("{cle} {empreinte}\n"));
    // Écriture atomique (voir `atomique`) : c'est ce fichier-ci qui compte le
    // plus — le perdre ramène TOUS les serveurs à « premier contact », et le
    // TOFU cesse de protéger sans que rien ne le signale.
    atomique::ecrire(&chemin, contenu.as_bytes())
        .with_context(|| format!("écriture de {}", chemin.display()))
}

/// Aligne les verrous clavier du bureau distant sur ceux du poste.
///
/// Sans cet événement, la session distante démarre avec ses propres verrous :
/// le pavé numérique paraît inactif alors qu'il est allumé côté utilisateur,
/// qui doit appuyer sur Verr.Num pour « resynchroniser » les deux.
///
/// Bits du message [10] : 1 = numérique, 2 = majuscules, 4 = défilement.
fn lock_sync_event(bits: u8) -> ironrdp::pdu::input::fast_path::FastPathInputEvent {
    ironrdp::input::synchronize_event(
        bits & 0b100 != 0, // défilement
        bits & 0b001 != 0, // numérique
        bits & 0b010 != 0, // majuscules
        false,             // kana : claviers japonais, non géré
    )
}

/// Décode un message d'entrée binaire en opérations IronRDP.
fn input_ops(b: &[u8]) -> Vec<Operation> {
    let u16le = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    match b.first().copied() {
        Some(1) if b.len() >= 5 => vec![Operation::MouseMove(MousePosition {
            x: u16le(1),
            y: u16le(3),
        })],
        Some(2) if b.len() >= 7 => {
            let bt = mouse_button(b[1]);
            let click = if b[2] != 0 {
                Operation::MouseButtonPressed(bt)
            } else {
                Operation::MouseButtonReleased(bt)
            };
            vec![
                Operation::MouseMove(MousePosition {
                    x: u16le(3),
                    y: u16le(5),
                }),
                click,
            ]
        }
        Some(3) if b.len() >= 3 => {
            let d = i16::from_le_bytes([b[1], b[2]]);
            vec![Operation::WheelRotations(WheelRotations {
                is_vertical: true,
                rotation_units: d,
            })]
        }
        Some(4) if b.len() >= 4 => {
            let sc = ironrdp::input::Scancode::from(u16le(1));
            vec![if b[3] != 0 {
                Operation::KeyPressed(sc)
            } else {
                Operation::KeyReleased(sc)
            }]
        }
        _ => Vec::new(),
    }
}

/// Rectangle mis à jour -> message FRAME binaire [2][x][y][w][h][RGBA].
/// Marqueur reconnu par l'interface : le serveur ne sait pas faire de NLA.
///
/// Elle propose alors de se connecter quand même, en expliquant ce que cela
/// coûte, et retient le choix pour ce serveur. Un marqueur plutôt qu'un texte
/// anglais issu d'une dépendance : celui-ci ne changera pas sous nos pieds.
pub const NLA_INDISPONIBLE: &str = "[AVASH_RDP_SANS_NLA]";

/// Nombre maximal de rectangles portés par une trame.
const RECTS_MAX: usize = 8;

/// En-tête d'un rectangle dans le message : x, y, largeur, hauteur (u16).
const ENTETE_RECT: usize = 8;

/// Ajoute un rectangle à la zone sale, en ne fusionnant que si c'est rentable.
///
/// L'ancienne version gardait une **union englobante** : deux petites zones aux
/// coins opposés donnaient un rectangle plein écran. Mesuré contre un vrai xrdp,
/// sur une session animée : 7,94 Mo envoyés pour 4,35 Mo utiles, soit 1,8 fois
/// trop dès que trois zones se rejoignaient.
///
/// La règle est arithmétique, pas heuristique : on ne fusionne que si l'union
/// coûte moins cher que les deux rectangles séparés, en-têtes compris. Deux
/// zones voisines fusionnent donc ; deux zones opposées, jamais.
///
/// Au-delà de `RECTS_MAX`, il faut bien céder : on fusionne alors la paire dont
/// l'union gaspille le moins. Une trame ne peut pas porter un nombre illimité
/// de rectangles.
fn ajouter_rect(zone: &mut Vec<InclusiveRectangle>, r: &InclusiveRectangle) {
    let aire = |a: &InclusiveRectangle| {
        (u64::from(a.right) - u64::from(a.left) + 1) * (u64::from(a.bottom) - u64::from(a.top) + 1)
    };
    let union = |a: &InclusiveRectangle, b: &InclusiveRectangle| InclusiveRectangle {
        left: a.left.min(b.left),
        top: a.top.min(b.top),
        right: a.right.max(b.right),
        bottom: a.bottom.max(b.bottom),
    };
    let cout = |a: &InclusiveRectangle| aire(a) * 4 + ENTETE_RECT as u64;

    for e in zone.iter_mut() {
        let fusion = union(e, r);
        if cout(&fusion) <= cout(e) + cout(r) {
            *e = fusion;
            return;
        }
    }
    zone.push(r.clone());
    while zone.len() > RECTS_MAX {
        let mut choix = (u64::MAX, 0usize, 1usize);
        for i in 0..zone.len() {
            for j in (i + 1)..zone.len() {
                // saturating_sub : deux rectangles qui SE CHEVAUCHENT ont une union
                // plus petite que la somme de leurs aires — la soustraction brute
                // débordait (panique en debug/test, enroulement silencieux en
                // release, où la valeur ~u64::MAX n'était alors jamais choisie et la
                // paire chevauchante ne fusionnait jamais). Saturé à 0, une paire
                // qui se recouvre devient au contraire la moins coûteuse à fusionner
                // — exactement ce qu'on veut.
                let perte = aire(&union(&zone[i], &zone[j]))
                    .saturating_sub(aire(&zone[i]) + aire(&zone[j]));
                if perte < choix.0 {
                    choix = (perte, i, j);
                }
            }
        }
        let (_, i, j) = choix;
        zone[i] = union(&zone[i], &zone[j]);
        zone.remove(j);
    }
}

/// Zone sale -> message binaire. Un seul rectangle garde la forme historique
/// `[2]` ; plusieurs empruntent `[13]`, qui porte leur nombre. Une trame, un
/// accusé de rendu : le cadencement reste exact.
fn frames_msg(image: &DecodedImage, zone: &[InclusiveRectangle]) -> Vec<u8> {
    if let [seul] = zone {
        return frame_msg(image, seul);
    }
    let iw = usize::from(image.width());
    let data = image.data();
    // Capacité calculée d'avance : 2 octets d'en-tête + par rectangle 8 octets de
    // géométrie et w*h*4 de pixels. Sans elle, le Vec repartait de 1 octet et
    // doublait ~20 fois sur un message plein écran (plusieurs Mo), recopiant tout
    // le contenu déjà écrit à chaque fois — le frère `frame_msg` réservait pourtant.
    let capacite = 2 + zone
        .iter()
        .map(|r| {
            let (w, h) = (
                usize::from(r.right - r.left + 1),
                usize::from(r.bottom - r.top + 1),
            );
            8 + w * h * 4
        })
        .sum::<usize>();
    let mut m = Vec::with_capacity(capacite);
    m.push(13u8);
    m.push(u8::try_from(zone.len()).unwrap_or(u8::MAX));
    for r in zone {
        let (x, y) = (r.left, r.top);
        let (w, h) = (r.right - r.left + 1, r.bottom - r.top + 1);
        m.extend_from_slice(&x.to_le_bytes());
        m.extend_from_slice(&y.to_le_bytes());
        m.extend_from_slice(&w.to_le_bytes());
        m.extend_from_slice(&h.to_le_bytes());
        for row in 0..usize::from(h) {
            let start = ((usize::from(y) + row) * iw + usize::from(x)) * 4;
            m.extend_from_slice(&data[start..start + usize::from(w) * 4]);
        }
    }
    m
}

/// Plafond de résolution accepté d'un serveur RDP.
///
/// C'est le serveur qui **confirme** la résolution, et il n'est pas tenu de
/// reprendre celle demandée. Rien ne bornait ce qu'on en faisait :
/// `DecodedImage::new` alloue `largeur × hauteur × 4` octets d'un bloc, soit
/// 17 Gio pour un 65535×65535 annoncé — mort du processus par manque de
/// mémoire, rejouable à volonté par la renégociation `DeactivateAll`. 8192 est
/// déjà la borne appliquée au redimensionnement côté interface.
const TAILLE_MAX: u16 = 8192;

fn taille_sure(w: u16, h: u16) -> anyhow::Result<(u16, u16)> {
    anyhow::ensure!(
        w > 0 && h > 0 && w <= TAILLE_MAX && h <= TAILLE_MAX,
        "Le serveur annonce une résolution inacceptable ({w}x{h})."
    );
    Ok((w, h))
}

fn frame_msg(image: &DecodedImage, r: &ironrdp::pdu::geometry::InclusiveRectangle) -> Vec<u8> {
    let iw = usize::from(image.width());
    let data = image.data();
    let (x, y) = (r.left, r.top);
    let (w, h) = (r.right - r.left + 1, r.bottom - r.top + 1);
    let mut m = Vec::with_capacity(9 + usize::from(w) * usize::from(h) * 4);
    m.push(2);
    m.extend_from_slice(&x.to_le_bytes());
    m.extend_from_slice(&y.to_le_bytes());
    m.extend_from_slice(&w.to_le_bytes());
    m.extend_from_slice(&h.to_le_bytes());
    for row in 0..usize::from(h) {
        let start = ((usize::from(y) + row) * iw + usize::from(x)) * 4;
        m.extend_from_slice(&data[start..start + usize::from(w) * 4]);
    }
    m
}

/// Le serveur a-t-il mis fin à la session après nous avoir authentifiés ?
///
/// Deux formes pour un même événement, et c'est ce qui a trompé Adrien :
///
/// - le serveur envoie un *Disconnect Provider Ultimatum* et nous le lisons ;
/// - il coupe la connexion TCP, et c'est le système qui nous le dit —
///   « connection reset by peer » sous Unix, **os error 10054** sous Windows,
///   qui ne ressemble à rien pour qui le lit.
///
/// Le second cas affichait un code brut là où le premier expliquait. Même
/// cause, même message.
fn session_close_par_le_serveur(texte: &str) -> bool {
    texte.contains("disconnect provider ultimatum") || est_coupure(texte)
}

/// La connexion a-t-elle été coupée brutalement, sans réponse ?
///
/// Sous Windows cela remonte en `os error 10054` (WSAECONNRESET), un code brut
/// qui ne dit rien à qui le lit. Sous Unix, `os error 104`. Une fermeture nette
/// en cours de lecture donne, elle, une fin de flux inattendue.
fn coupure_brutale(e: &connector::ConnectorError) -> bool {
    est_coupure(&chaine_des_causes(e))
}

/// Aplatit un message et toute sa chaîne de causes.
///
/// La phrase utile vit rarement dans l'affichage direct : elle est enfouie dans
/// les causes. Sans ce parcours, la détection ne voit rien.
fn chaine_des_causes(e: &(dyn std::error::Error + 'static)) -> String {
    let mut texte = format!("{e} {e:?}");
    let mut source = e.source();
    while let Some(c) = source {
        texte.push(' ');
        texte.push_str(&c.to_string());
        source = c.source();
    }
    texte
}

/// Le pair a-t-il coupé sans rien dire ?
///
/// Windows remonte `os error 10054` (WSAECONNRESET), Unix `os error 104`. Ces
/// codes bruts ne disent rien à qui les reçoit — c'est exactement ce qu'Adrien a
/// vu en tentant un RDP vers un Windows.
fn est_coupure(texte: &str) -> bool {
    texte.contains("os error 10054")
        || texte.contains("os error 104")
        || texte.contains("connection reset")
        || texte.contains("Connection reset")
        || texte.contains("unexpected end of file")
        || texte.contains("early eof")
        || texte.contains("custom error")
}

/// Version et types de PDU RDSTLS (MS-RDPBCGR 2.2.17).
const RDSTLS_VERSION_1: u16 = 0x0001;
const RDSTLS_TYPE_CAPABILITIES: u16 = 0x0001;
const RDSTLS_TYPE_AUTHREQ: u16 = 0x0002;
const RDSTLS_TYPE_AUTHRSP: u16 = 0x0004;
const RDSTLS_DATA_PASSWORD_CREDS: u16 = 0x0001;

/// Traduit le verdict du serveur d'arrivée.
///
/// Ces identifiants sont engendrés par le serveur lui-même et n'ont qu'un
/// usage : un refus ne vient donc jamais d'une faute de frappe de
/// l'utilisateur, et le message ne doit pas le lui laisser croire.
fn verdict_rdstls(code: u32) -> String {
    let raison = match code {
        0x0000_0005 => "le compte n'a pas le droit d'accéder à ce serveur",
        0x0000_052e => "le serveur d'arrivée ne reconnaît pas les identifiants transmis",
        0x0000_0530 => "le compte est soumis à des plages horaires",
        0x0000_0532 => "le mot de passe du compte a expiré",
        0x0000_0533 => "le compte est désactivé",
        0x0000_0773 => "le mot de passe du compte doit être changé",
        0x0000_0775 => "le compte est verrouillé",
        _ => "raison inconnue",
    };
    format!(
        "Le serveur d'arrivée a refusé la redirection : {raison} (code {code:#010x}). \
         Ces identifiants sont engendrés par le serveur lui-même : ce n'est pas une \
         erreur de saisie, mais un désaccord entre ses deux démons — ou une \
         redirection expirée."
    )
}

/// Authentification RDSTLS (MS-RDPBCGR 2.2.17), après la montée TLS.
///
/// C'est le protocole des connexions **redirigées**. Le serveur d'arrivée
/// n'attend ni CredSSP ni TLS simple : il veut qu'on lui réémette, tels quels,
/// les champs que la redirection nous a remis — identifiant de redirection,
/// nom d'utilisateur, domaine et mot de passe. Ce dernier est chiffré par clé
/// publique ; le client ne le déchiffre pas, il le transporte.
///
/// Sans cet échange, la séquence se poursuit puis le serveur met fin à la
/// session — ce qui ressemble à s'y méprendre à un refus de session.
async fn rdstls_authentifier<S>(
    flux: &mut S,
    r: &ironrdp::session::redirection::Redirection,
) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    /// Longueur sur 16 bits, puis les octets tels quels.
    fn champ(m: &mut Vec<u8>, v: Option<&Vec<u8>>) {
        let v = v.map_or(&[][..], Vec::as_slice);
        m.extend_from_slice(&u16::try_from(v.len()).unwrap_or(0).to_le_bytes());
        m.extend_from_slice(v);
    }

    // Le serveur parle le premier : il annonce ses capacités. Huit octets —
    // Version, PduType, DataType, VersionsPrises — et non six comme on pourrait
    // le déduire d'une lecture rapide de la spécification. Vérifié sur le fil.
    let mut capacites = [0u8; 8];
    flux.read_exact(&mut capacites)
        .await
        .context("capacités RDSTLS")?;
    let type_capacites = u16::from_le_bytes([capacites[2], capacites[3]]);
    anyhow::ensure!(
        type_capacites == RDSTLS_TYPE_CAPABILITIES,
        "Réponse RDSTLS inattendue : type {type_capacites}, capacités attendues."
    );

    let mut m = Vec::with_capacity(256);
    m.extend_from_slice(&RDSTLS_VERSION_1.to_le_bytes());
    m.extend_from_slice(&RDSTLS_TYPE_AUTHREQ.to_le_bytes());
    m.extend_from_slice(&RDSTLS_DATA_PASSWORD_CREDS.to_le_bytes());
    champ(&mut m, r.guid.as_ref());
    champ(&mut m, r.utilisateur_brut.as_ref());
    champ(&mut m, r.domaine_brut.as_ref());
    champ(&mut m, r.mot_de_passe.as_ref());
    flux.write_all(&m).await.context("envoi RDSTLS")?;
    flux.flush().await.ok();

    // Verdict : Version, PduType, DataType, puis le code sur quatre octets.
    let mut rep = [0u8; 10];
    flux.read_exact(&mut rep).await.context("réponse RDSTLS")?;
    let type_reponse = u16::from_le_bytes([rep[2], rep[3]]);
    anyhow::ensure!(
        type_reponse == RDSTLS_TYPE_AUTHRSP,
        "Réponse RDSTLS inattendue : type {type_reponse}, verdict attendu."
    );
    let code = u32::from_le_bytes([rep[6], rep[7], rep[8], rep[9]]);
    anyhow::ensure!(code == 0, "{}", verdict_rdstls(code));
    Ok(())
}

async fn connect(
    a: &Args,
    clip_backend: ClipBackend,
    redirection: Option<&ironrdp::session::redirection::Redirection>,
    graphique: egfx::Politique,
) -> Result<(
    connector::ConnectionResult,
    ironrdp_tokio::TokioFramed<ironrdp_tls::TlsStream<TcpStream>>,
    egfx::CanalPartage,
    egfx::FilePartagee,
)> {
    let tcp = TcpStream::connect((a.host.as_str(), a.port))
        .await
        .with_context(|| format!("connexion TCP à {}:{}", a.host, a.port))?;
    // Nagle OFF : les entrées et les petits rectangles d'écran partent sans délai.
    tcp.set_nodelay(true).ok();
    let client_addr = tcp.local_addr()?;
    let mut framed = ironrdp_tokio::TokioFramed::new(tcp);
    let (egfx, canal_egfx, file_egfx) = egfx::Egfx::nouveau();
    // Canal Display Control (DVC) : permet le redimensionnement natif du
    // bureau distant (le serveur re-rend à la nouvelle résolution).
    let mut dvc = DrdynvcClient::new()
        .with_dynamic_channel(DisplayControlClient::new(|_caps| Ok(Vec::new())));
    // Le canal graphique n'est offert qu'aux serveurs qui ont montré n'en avoir
    // pas d'autre : l'accepter suffit à faire taire un serveur Windows. Voir
    // `egfx::Politique`.
    if graphique == egfx::Politique::Accepter {
        dvc.attach_dynamic_channel(egfx);
    }
    let mut connector = connector::ClientConnector::new(build_config(a, redirection), client_addr)
        .with_static_channel(dvc)
        // Canal CLIPRDR : presse-papiers partagé poste <-> bureau distant (texte).
        .with_static_channel(CliprdrClient::new(Box::new(clip_backend)));
    let should_upgrade = match ironrdp_tokio::connect_begin(&mut framed, &mut connector).await {
        Ok(v) => v,
        // Le serveur a refusé la négociation alors que nous n'annoncions que
        // NLA : il ne sait pas le faire. Ce n'est pas forcément une attaque —
        // un xrdp sans module PAM est dans ce cas — mais ce n'est pas à nous
        // d'en décider en silence. On remonte un marqueur que l'interface
        // reconnaît, pour poser la question à l'utilisateur.
        Err(e)
            if !a.sans_nla && matches!(e.kind(), connector::ConnectorErrorKind::Negotiation(_)) =>
        {
            anyhow::bail!(
                "{NLA_INDISPONIBLE} Ce serveur n'accepte pas l'authentification \
                 réseau (NLA) et exige un simple canal TLS."
            );
        }
        // Coupure brutale pendant la négociation. Windows la remonte comme
        // « os error 10054 », qui ne dit rien à personne — Adrien l'a reçue tel
        // quel. Un serveur qui ferme sans répondre est le plus souvent un
        // serveur qui ne sait pas faire ce qu'on lui demande : ici, NLA. On pose
        // donc la même question que pour un refus explicite, en disant
        // clairement ce qu'on sait et ce qu'on ignore.
        Err(e) if !a.sans_nla && coupure_brutale(&e) => {
            anyhow::bail!(
                "{NLA_INDISPONIBLE} Ce serveur a fermé la connexion sans répondre \
                 à la négociation. C'est le comportement de serveurs qui n'acceptent \
                 pas l'authentification réseau (NLA) — mais un pare-feu ou un service \
                 qui n'est pas du RDP donneraient la même chose."
            );
        }
        Err(e) if coupure_brutale(&e) => {
            anyhow::bail!(
                "Ce serveur a fermé la connexion sans répondre. Vérifiez que le \
                 service RDP écoute bien sur ce port et qu'aucun pare-feu ne s'y \
                 oppose."
            );
        }
        Err(e) => return Err(e).context("début de connexion"),
    };
    let initial = framed.into_inner_no_leftover();
    let (mut upgraded_stream, cert) =
        ironrdp_tls::upgrade(initial, &a.host).await.map_err(|e| {
            // Le serveur a accepté la négociation, puis rompu pendant TLS.
            // Sous Windows cela remonte en « os error 10054 », un code brut que
            // rien ne permet d'interpréter — signalé par Adrien sur un Windows
            // Server. Renoncer à NLA n'y changerait rien : ce repli passe lui
            // aussi par TLS. Le message doit donc envoyer chercher ailleurs.
            if est_coupure(&chaine_des_causes(&e)) {
                anyhow::anyhow!(
                    "Ce serveur a accepté la négociation puis a rompu la connexion \
                     pendant l'établissement du canal chiffré. C'est le plus souvent \
                     un certificat RDP absent ou abîmé côté serveur, ou une couche \
                     de sécurité réglée sur « RDP » au lieu de « SSL ». Renoncer à \
                     l'authentification réseau n'y changerait rien : ce repli passe \
                     lui aussi par TLS."
                )
            } else {
                anyhow::Error::new(e).context("passage TLS")
            }
        })?;
    let pubkey = server_public_key(&cert)?;

    // TOFU sur le certificat, AVANT CredSSP : c'est CredSSP qui transmet les
    // identifiants. Vérifier après reviendrait à les avoir déjà livrés.
    let cle = format!("{}:{}", a.host, a.port);
    let presentee = empreinte(&pubkey);
    match juger_certificat(empreinte_memorisee(&cle).as_deref(), &presentee) {
        VerdictCert::Connu => {}
        VerdictCert::PremierContact => memoriser_empreinte(&cle, &presentee)
            .context("mémorisation de l'empreinte du serveur RDP")?,
        VerdictCert::Change { attendue } => {
            anyhow::bail!(
                "Le certificat de {cle} a changé.\n\nSoit le serveur a été \
                 réinstallé, soit quelqu'un intercepte la connexion.\n\n\
                 Empreinte présentée : {presentee}\nEmpreinte attendue  : {attendue}\n\n\
                 Si le changement est légitime, retirez la ligne « {cle} » de \
                 rdp_known_hosts."
            );
        }
    }

    // Connexion redirigée : l'authentification RDSTLS vient ici, APRÈS la
    // vérification du certificat — elle transporte des identifiants, et les
    // livrer à un serveur non vérifié annulerait la protection qu'on vient
    // d'appliquer.
    if let Some(r) = redirection {
        rdstls_authentifier(&mut upgraded_stream, r).await?;
    }

    let upgraded = ironrdp_tokio::mark_as_upgraded(should_upgrade, &mut connector);
    let mut framed = ironrdp_tokio::TokioFramed::new(upgraded_stream);
    let mut net = ironrdp_tokio::reqwest::ReqwestNetworkClient::new();
    let result = ironrdp_tokio::connect_finalize(
        upgraded,
        connector,
        &mut framed,
        &mut net,
        a.host.clone().into(),
        pubkey,
        None,
    )
    .await
    .map_err(|e| {
        // Cette étape couvre TOUTE la fin de séquence, pas seulement NLA :
        // licence, capacités, activation. Un serveur qui coupe après avoir
        // accepté les identifiants tombait ici sous l'étiquette « CredSSP/NLA »,
        // qui accusait l'authentification alors qu'elle avait réussi.
        // La phrase vit dans la CHAÎNE de causes, pas dans l'affichage direct :
        // il faut la parcourir, sinon la détection ne voit rien.
        let mut texte = format!("{e} {e:?}");
        let mut source: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(&e);
        while let Some(c) = source {
            texte.push(' ');
            texte.push_str(&c.to_string());
            source = c.source();
        }
        if session_close_par_le_serveur(&texte) {
            anyhow::anyhow!(
                "Le serveur a accepté vos identifiants puis a mis fin à la session \
                 avant de l'ouvrir. L'authentification n'est pas en cause : c'est \
                 côté serveur que la session ne démarre pas, et il ne dit pas \
                 pourquoi. Sur un hôte Linux, son journal le dira — \
                 /var/log/xrdp-sesman.log."
            )
        } else {
            anyhow::Error::new(e).context("fin de la séquence de connexion")
        }
    })?;
    Ok((result, framed, canal_egfx, file_egfx))
}

#[tokio::main]
async fn main() -> Result<()> {
    // Traces de diagnostic, sur une variable À NOUS et non sur RUST_LOG : beaucoup
    // l'exportent globalement, et ces traces contiennent le mot de passe en clair
    // — la requête CredSSP le porte encodé en UTF-16, lisible tel quel. Ce qui a
    // servi à trouver un défaut ne doit pas s'activer par accident.
    if let Some(filtre) = std::env::var_os("AVASH_RDP_TRACE").and_then(|v| v.into_string().ok()) {
        // Les traces contiennent le mot de passe en clair (CredSSP le porte encodé
        // en UTF-16, lisible tel quel). Elles NE VONT PAS sur stderr : depuis le
        // journal de diagnostic, l'interface capte stderr, le garde en anneau et
        // l'affiche dans l'incrustation « Connexion RDP fermée » — le mot de passe
        // se retrouverait dans une capture d'écran jointe à un rapport de bug. On
        // les écrit dans un fichier dédié en 0600 et on n'annonce sur stderr que
        // son chemin.
        // Nom IMPRÉVISIBLE (aléa 64 bits, pas seulement le PID) et ouverture en
        // create_new + O_NOFOLLOW : /tmp est mondialement inscriptible, et un nom
        // devinable ouvert en simple `create` suivrait un lien symbolique planté
        // d'avance par un autre compte — les traces, qui portent le mot de passe
        // en clair, atterriraient dans le fichier de son choix (CWE-59). create_new
        // échoue si la cible existe déjà ; O_NOFOLLOW refuse un lien.
        let chemin = std::env::temp_dir().join(format!(
            "avash-rdp-trace-{}-{:016x}.log",
            std::process::id(),
            rand::random::<u64>()
        ));
        let mut ouverture = std::fs::OpenOptions::new();
        ouverture.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            ouverture.mode(0o600);
            ouverture.custom_flags(libc::O_NOFOLLOW);
        }
        match ouverture.open(&chemin) {
            Ok(fichier) => {
                eprintln!(
                    "avash-rdp : traces actives, écrites dans {} (0600). ATTENTION, \
                     elles contiennent le mot de passe en clair — ne les collez nulle \
                     part sans les avoir relues.",
                    chemin.display()
                );
                tracing_subscriber::fmt()
                    .with_env_filter(tracing_subscriber::EnvFilter::new(filtre))
                    .with_ansi(false)
                    .with_writer(move || {
                        fichier
                            .try_clone()
                            .expect("clonage du descripteur de trace")
                    })
                    .init();
            }
            Err(e) => {
                // On refuse de retomber sur stderr : ce serait rouvrir la fuite que
                // ce fichier ferme. Sans trace, mais sans mot de passe exposé.
                eprintln!(
                    "avash-rdp : impossible d'ouvrir le fichier de trace {} ({e}) — \
                     traces désactivées.",
                    chemin.display()
                );
            }
        }
    }
    if let Some(chemin) = std::env::args()
        .nth(1)
        .filter(|a| a == "--rejouer")
        .and(std::env::args().nth(2))
    {
        let e = magnetoscope::lire(&chemin)?;
        let r = magnetoscope::rejouer(&e, false)?;
        println!(
            "rejeu : {} acceptés, {} graphiques refusés, {} hors périmètre, {} rectangles, empreinte {:016x}",
            r.acceptes, r.refuses, r.hors_perimetre, r.rectangles, r.empreinte
        );
        return Ok(());
    }
    let args = parse_args()?;
    // Une redirection oblige à tout refaire : nouvelle connexion TCP, nouvelle
    // négociation, en présentant cette fois le jeton de routage. GNOME Remote
    // Desktop s'en sert pour remettre le client du démon système au démon de la
    // session ; sans cette boucle, on décode la demande sans pouvoir y répondre.
    //
    // Bornée à trois tours : une chaîne de redirections sans fin serait un
    // serveur mal configuré, ou hostile.
    let mut redirection: Option<Box<ironrdp::session::redirection::Redirection>> = None;
    let mut poste: Option<Poste> = None;
    let memoire = chemin_canal_graphique();
    let cle = format!("{}:{}", args.host, args.port);
    let mut graphique = egfx::Politique::pour(&cle, memoire.as_deref());
    for _ in 0..TOURS_MAX {
        let dessine = std::sync::atomic::AtomicBool::new(false);
        let issue = executer(&args, redirection.take(), &mut poste, graphique, &dessine).await;
        // Une session qui se termine sans avoir affiché la moindre image, alors
        // qu'on lui refusait le canal graphique, désigne un serveur qui n'a que
        // celui-là. GNOME Remote Desktop ne patiente même pas : son pipeline ne
        // pouvant s'ouvrir, il raccroche aussitôt. Reprendre est la seule
        // réponse juste — et la seule qui n'exige pas de deviner à l'avance à
        // quelle famille de serveur on parle.
        let issue = match issue {
            Ok(Suite::Fini) | Err(_)
                if graphique == egfx::Politique::Observer
                    && !dessine.load(std::sync::atomic::Ordering::Relaxed) =>
            {
                Ok(Suite::ReprendreAvecGraphique)
            }
            autre => autre,
        };
        match issue? {
            Suite::ReprendreAvecGraphique => {
                eprintln!(
                    "egfx : ce serveur ne dessine pas par le chemin classique, \
                     reprise avec le canal graphique"
                );
                if let Some(m) = memoire.as_deref() {
                    egfx::memoriser(&cle, m);
                }
                graphique = egfx::Politique::Accepter;
            }
            Suite::Fini => return Ok(()),
            Suite::Rediriger(r) => {
                eprintln!(
                    "redirection : jeton de {} octets, identifiants {}",
                    r.jeton.as_ref().map_or(0, Vec::len),
                    if r.utilisateur.is_some() {
                        "fournis"
                    } else {
                        "absents"
                    }
                );
                redirection = Some(r);
            }
        }
    }
    anyhow::bail!(
        "Le serveur nous redirige sans fin : {TOURS_MAX} tours n'ont pas suffi à ouvrir une session."
    )
}

/// Nombre de connexions successives tolérées pour une seule ouverture de
/// session. Quatre suffisent au pire cas connu : connexion, redirection, reprise
/// avec le canal graphique, redirection de nouveau. La marge est là pour ne pas
/// transformer un serveur inhabituel en échec ; la borne, pour qu'un serveur qui
/// redirige en rond ne nous y entraîne pas.
const TOURS_MAX: usize = 6;

/// Ce qu'une session a donné.
/// Le poste de travail côté interface : l'écoute locale et le client accepté.
///
/// Il survit aux reconnexions RDP. Une redirection de serveur rétablit la
/// session distante par en dessous ; l'interface, elle, garde le même port, le
/// même jeton et la même WebSocket, et n'a rien à réapprendre.
struct Poste {
    _listener: TcpListener,
    sink: futures_util::stream::SplitSink<tokio_tungstenite::WebSocketStream<TcpStream>, Message>,
    stream: futures_util::stream::SplitStream<tokio_tungstenite::WebSocketStream<TcpStream>>,
}

/// Le couple (émetteur, récepteur) d'un WebSocket accepté, transmis d'une tâche
/// de validation vers la boucle d'acceptation.
type PosteSplit = (
    futures_util::stream::SplitSink<tokio_tungstenite::WebSocketStream<TcpStream>, Message>,
    futures_util::stream::SplitStream<tokio_tungstenite::WebSocketStream<TcpStream>>,
);

/// Compare deux jetons en temps constant : la durée ne dépend pas de la position
/// du premier octet qui diffère. Le `==` de tranches s'arrête au premier écart,
/// ce qui, en théorie, laisse deviner le jeton octet par octet. Non exploitable
/// ici (jeton de 16 octets, comparaison noyée dans la gigue d'une boucle TCP en
/// loopback), mais gratuit à faire correctement.
fn jetons_egaux(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Rappel de validation d'origine pour `accept_hdr_async`. Fonction nommée (et
/// non closure) pour porter l'`allow` : le type d'erreur imposé par tungstenite
/// est volumineux, mais on ne le construit qu'au rejet d'un client — jamais sur
/// le chemin normal.
#[allow(clippy::result_large_err)]
fn verifier_origine(
    req: &tokio_tungstenite::tungstenite::handshake::server::Request,
    resp: tokio_tungstenite::tungstenite::handshake::server::Response,
) -> Result<
    tokio_tungstenite::tungstenite::handshake::server::Response,
    tokio_tungstenite::tungstenite::handshake::server::ErrorResponse,
> {
    let origine = req.headers().get("origin").and_then(|v| v.to_str().ok());
    if origine_admise(origine) {
        Ok(resp)
    } else {
        let mut refus = tokio_tungstenite::tungstenite::handshake::server::ErrorResponse::new(
            Some("origine non autorisée".to_owned()),
        );
        *refus.status_mut() = tokio_tungstenite::tungstenite::http::StatusCode::FORBIDDEN;
        Err(refus)
    }
}

/// Décide si une origine WebSocket est admise. Une page web réelle porte
/// `http(s)://<domaine>` : on la refuse. La webview native porte `tauri://…`
/// (Linux/macOS) ou `http(s)://tauri.localhost` (Windows) ; le serveur de
/// développement, `http://localhost:<port>`. Une absence d'origine est admise —
/// certains clients n'en posent pas, et le jeton reste l'authentification réelle.
///
/// Le tri se fait sur une copie en minuscules (un navigateur normalise le schéma,
/// mais on ne s'y fie pas) et refuse par défaut : seuls les schémas explicitement
/// attendus (tauri://) passent, tout autre (`file://`, `null`, `data:`…) est
/// rejeté. Fail-closed — le laxisme précédent n'était que de la défense en
/// profondeur, autant qu'elle ferme réellement.
fn origine_admise(origine: Option<&str>) -> bool {
    let Some(o) = origine else {
        return true;
    };
    let o = o.to_ascii_lowercase();
    if let Some(reste) = o
        .strip_prefix("http://")
        .or_else(|| o.strip_prefix("https://"))
    {
        let hote = reste.split(['/', ':']).next().unwrap_or(reste);
        hote == "tauri.localhost" || hote == "localhost" || hote == "127.0.0.1"
    } else {
        // Seule la webview native (schéma tauri://) est admise hors http(s) ; tout
        // autre schéma est refusé plutôt qu'admis par défaut.
        o.starts_with("tauri://")
    }
}

enum Suite {
    /// Rien n'a été dessiné : ce serveur n'a probablement que le canal
    /// graphique, qu'il faut lui offrir — donc se reconnecter.
    ReprendreAvecGraphique,
    /// La session s'est terminée normalement.
    Fini,
    /// Le serveur nous renvoie ailleurs ; il faut tout refaire avec ce qu'il donne.
    Rediriger(Box<ironrdp::session::redirection::Redirection>),
}

async fn executer(
    args: &Args,
    redirection: Option<Box<ironrdp::session::redirection::Redirection>>,
    poste: &mut Option<Poste>,
    graphique: egfx::Politique,
    dessine: &std::sync::atomic::AtomicBool,
) -> Result<Suite> {
    let local_text: LocalClip = std::sync::Arc::new(std::sync::Mutex::new(None));
    let (clip_tx, mut clip_rx) = tokio::sync::mpsc::unbounded_channel::<ClipReq>();
    // Actif par défaut — parité avec les autres clients RDP ; l'interface
    // annonce aussitôt le réglage retenu par l'utilisateur (message [12]).
    let partage_clip = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let clip_backend = ClipBackend {
        partage: partage_clip.clone(),
        local_text: local_text.clone(),
        tx: clip_tx.clone(),
    };
    // Une tentative de connexion doit être BORNÉE. Sans cela le processus reste
    // pendu indéfiniment, sans un mot : constaté contre un xrdp qui annonce NLA
    // mais ne mène jamais l'échange CredSSP à son terme — TLS est monté, les
    // données circulent, et rien n'aboutit. L'utilisateur voit un onglet figé.
    const DELAI_CONNEXION: Duration = Duration::from_secs(25);
    let (result, mut framed, canal_egfx, file_egfx) = match tokio::time::timeout(
        DELAI_CONNEXION,
        connect(args, clip_backend, redirection.as_deref(), graphique),
    )
    .await
    {
        Ok(r) => r?,
        Err(_) if !args.sans_nla => anyhow::bail!(
            "{NLA_INDISPONIBLE} L'authentification réseau (NLA) n'a pas abouti \
             en {} s. Certains serveurs annoncent NLA sans savoir le mener à \
             bien — c'est le cas de plusieurs versions de xrdp.",
            DELAI_CONNEXION.as_secs()
        ),
        Err(_) => anyhow::bail!(
            "Le serveur n'a pas répondu en {} s.",
            DELAI_CONNEXION.as_secs()
        ),
    };
    let (w, h) = taille_sure(result.desktop_size.width, result.desktop_size.height)?;
    let activation_factory = result.activation_factory;
    eprintln!("connecté : {w}x{h}");
    let mut image = DecodedImage::new(PixelFormat::RgbA32, w, h);
    let (io_channel_id, user_channel_id) = (result.io_channel_id, result.user_channel_id);
    let (message_channel_id, share_id) = (result.message_channel_id, result.share_id);
    let canal_dvc = result
        .static_channels
        .get_channel_id_by_type::<DrdynvcClient>();
    let canal_clip = result
        .static_channels
        .get_channel_id_by_type::<CliprdrClient>();
    if std::env::var_os("AVASH_EGFX_TRACE").is_some() {
        eprintln!("canaux statiques : dvc={canal_dvc:?} clip={canal_clip:?} io={io_channel_id} user={user_channel_id}");
    }
    let mut active = ActiveStageBuilder {
        static_channels: result.static_channels,
        user_channel_id: result.user_channel_id,
        io_channel_id: result.io_channel_id,
        message_channel_id: result.message_channel_id,
        share_id: result.share_id,
        compression_type: result.compression_type,
        enable_server_pointer: result.enable_server_pointer,
        pointer_software_rendering: result.pointer_software_rendering,
    }
    .build();

    // Magnétoscope : capture le dialogue du serveur pour le rejouer plus tard,
    // sans réseau. Voir magnetoscope.rs — c'est ce qui transforme une machine du
    // parc en fixture permanente.
    let mut magneto = match args.enregistrer.as_deref() {
        Some(chemin) => {
            let entete = magnetoscope::Entete {
                largeur: w,
                hauteur: h,
                io: io_channel_id,
                utilisateur: user_channel_id,
                message: message_channel_id,
                partage: share_id,
                compression: 0,
                canal_dvc: canal_dvc.unwrap_or(0),
                canal_clip: canal_clip.unwrap_or(0),
            };
            let e =
                magnetoscope::Enregistreur::nouveau(chemin, &entete, magnetoscope::PLAFOND_DEFAUT)?;
            eprintln!("enregistrement : {chemin}");
            Some(e)
        }
        None => None,
    };

    if let Some(path) = args.shot.clone() {
        return run_shot(
            &mut active,
            &mut image,
            &mut framed,
            &path,
            magneto.as_mut(),
            &Graphique {
                canal: &canal_egfx,
                file: &file_egfx,
                dessine,
            },
        )
        .await;
    }

    // Serveur WebSocket local : un seul client (Avash), jeton obligatoire.
    // Établi au premier passage seulement : une redirection de serveur rappelle
    // cette fonction, et rouvrir un port neuf laisserait l'interface parler dans
    // le vide, attachée à l'ancien.
    if poste.is_none() {
        // Serveur WebSocket local : un seul client (Avash), jeton obligatoire.
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .context("écoute WebSocket")?;
        let port = listener.local_addr()?.port();
        let token = format!("{:016x}", rand::random::<u64>());
        // Annonce le point de connexion à Avash.
        let mut out = tokio::io::stdout();
        out.write_all(format!("{port} {token}\n").as_bytes())
            .await?;
        out.flush().await?;

        // On boucle sur les connexions au lieu d'en accepter une seule. Le port est
        // ouvert avant même que l'interface n'en soit avertie : n'importe quel
        // processus local — ou une page web, les WebSocket n'étant pas soumises à
        // la politique d'origine pour *établir* la connexion — pouvait s'y
        // présenter le premier. Un message quelconque faisait quitter le sidecar,
        // détruisant une session RDP déjà authentifiée (TLS + NLA refaits) ; une
        // connexion TCP laissée sans poignée de main WebSocket consommait la seule
        // place d'`accept` et l'interface n'arrivait jamais à se connecter.
        // Le jeton (64 bits) reste hors de portée : c'était un déni de service, pas
        // un détournement. On rejette maintenant l'intrus et on attend le suivant,
        // avec un délai de garde par tentative pour qu'un client muet ne bloque pas
        // la file.
        const DELAI_POIGNEE: Duration = Duration::from_secs(10);
        // Chaque validation (poignée WebSocket + premier message) dans SA tâche,
        // et l'acceptation continue en parallèle : un client muet n'immobilise
        // plus la file, ce qui fermait la porte à un déni de service par une page
        // web ou un processus local qui ouvrait des connexions sans rien envoyer.
        // On retient le premier client qui présente le bon jeton, puis on cesse
        // d'accepter (les tâches encore en vol tombent avec le canal).
        let (tx, mut rx) = tokio::sync::mpsc::channel::<PosteSplit>(1);
        let (sink, stream) = loop {
            tokio::select! {
                Some(pair) = rx.recv() => break pair,
                accepte = listener.accept() => {
                    let Ok((tcp, _)) = accepte else { continue };
                    tcp.set_nodelay(true).ok();
                    let tx = tx.clone();
                    let token = token.clone();
                    tokio::spawn(async move {
                        // Contrôle d'origine (verifier_origine) : une page web réelle
                        // porte http(s)://<domaine> et se voit refusée ; la webview
                        // (tauri://… ou tauri.localhost, localhost en dev) passe. Le
                        // jeton reste requis.
                        let Ok(Ok(ws)) = tokio::time::timeout(
                            DELAI_POIGNEE,
                            tokio_tungstenite::accept_hdr_async(tcp, verifier_origine),
                        )
                        .await
                        else {
                            return; // poignée absente, trop lente, ou origine refusée
                        };
                        let (sink, mut stream) = ws.split();
                        // Premier message = le jeton, comparé à temps constant.
                        if let Ok(Some(Ok(Message::Binary(t)))) =
                            tokio::time::timeout(DELAI_POIGNEE, stream.next()).await
                        {
                            if jetons_egaux(&t, token.as_bytes()) {
                                let _ = tx.send((sink, stream)).await;
                            }
                        }
                    });
                }
            }
        };
        *poste = Some(Poste {
            _listener: listener,
            sink,
            stream,
        });
    }
    let Poste { sink, stream, .. } = poste.as_mut().expect("poste établi juste au-dessus");

    // CONNECTED [1][w][h]
    let mut hello = vec![1u8];
    hello.extend_from_slice(&w.to_le_bytes());
    hello.extend_from_slice(&h.to_le_bytes());
    sink.send(Message::Binary(hello)).await?;

    let mut db = Database::new();

    // --- Cadencement adaptatif sur ACK (anti-lag) ---------------------------
    // Au plus une trame « en vol ». Les mises à jour qui arrivent pendant le
    // rendu de la webview sont fusionnées (union des rectangles) ; à l'ACK, on
    // envoie l'état le plus récent. Rapide → chaque trame part aussitôt (aucune
    // latence ajoutée) ; lent → on fusionne, jamais de file qui s'accumule.
    let mut dirty: Vec<InclusiveRectangle> = Vec::new();
    let mut awaiting_ack = false;
    // Onglet masqué : le canvas n'est pas à l'écran, mais l'accusé de rendu
    // partait quand même — le serveur voyait la voie libre en permanence et le
    // sidecar continuait à décoder et à pousser des trames pleines (8 Mo en
    // 1080p) que personne ne regardait. En pause, on accumule le rectangle sale
    // sans rien émettre ; le retour au premier plan demande un REFRESH.
    let mut en_pause = false;
    // Le serveur dessine-t-il par le chemin classique ? Voir ATTENTE_EGFX.
    let mut dessin_classique = false;
    let mut silence: Option<Instant> = None;
    let mut last_send = Instant::now();
    // Métriques (fenêtre ~1 s) : fps, débit, latence de bout en bout.
    let (mut stat_frames, mut stat_bytes): (u32, u64) = (0, 0);
    let mut lat_ms: f32 = 0.0;
    let mut stat_window = Instant::now();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    // Envoie la zone accumulée si la voie est libre. `sink`/`image`/compteurs
    // viennent du scope englobant (macro non hygiénique volontairement).
    #[allow(clippy::items_after_statements)]
    macro_rules! flush_dirty {
        () => {
            if !awaiting_ack && !en_pause {
                if !dirty.is_empty() {
                    let msg = frames_msg(&image, &dirty);
                    dirty.clear();
                    stat_bytes += msg.len() as u64;
                    stat_frames += 1;
                    sink.send(Message::Binary(msg))
                        .await
                        .context("envoi frame")?;
                    awaiting_ack = true;
                    last_send = Instant::now();
                }
            }
        };
    }

    // Récolte les trames décodées par le canal graphique et les peint dans
    // l'image. EGFX décode dans ses propres surfaces : sans ce report, la
    // session est parfaitement fonctionnelle et l'écran reste noir.
    #[allow(clippy::items_after_statements)]
    macro_rules! peindre_egfx {
        () => {{
            let sortie = std::mem::take(&mut *file_egfx.lock().unwrap());
            if let Some((nl, nh)) = sortie.taille {
                if (nl, nh) != (image.width(), image.height()) {
                    image = DecodedImage::new(PixelFormat::RgbA32, nl, nh);
                    dirty.clear();
                    awaiting_ack = false;
                    let mut hello = vec![1u8];
                    hello.extend_from_slice(&nl.to_le_bytes());
                    hello.extend_from_slice(&nh.to_le_bytes());
                    sink.send(Message::Binary(hello))
                        .await
                        .context("nouvelle taille")?;
                }
            }
            if !sortie.trames.is_empty() {
                dessine.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            for t in sortie.trames {
                image.peindre_rgba(t.x, t.y, t.largeur, t.hauteur, &t.pixels);
                ajouter_rect(
                    &mut dirty,
                    &InclusiveRectangle {
                        left: t.x,
                        top: t.y,
                        right: t.x.saturating_add(t.largeur).saturating_sub(1),
                        bottom: t.y.saturating_add(t.hauteur).saturating_sub(1),
                    },
                );
            }
            flush_dirty!();
        }};
    }

    // Envoie au serveur les messages d'un canal statique (ici CLIPRDR).
    #[allow(clippy::items_after_statements)]
    macro_rules! send_svc {
        ($msgs:expr) => {{
            let bytes = active
                .process_svc_processor_messages($msgs)
                .context("encodage SVC")?;
            framed.write_all(&bytes).await.context("écriture SVC")?;
        }};
    }

    loop {
        tokio::select! {
            biased;
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Binary(b))) if b.first() == Some(&5) && b.len() >= 5 => {
                        // RESIZE : demande au serveur de re-rendre à la nouvelle
                        // taille via Display Control. Sans effet tant que le canal
                        // n'a pas reçu ses capacités (encode_resize renvoie None).
                        let rw = u16::from_le_bytes([b[1], b[2]]);
                        let rh = u16::from_le_bytes([b[3], b[4]]);
                        let (aw, ah) = MonitorLayoutEntry::adjust_display_size(u32::from(rw), u32::from(rh));
                        if let Some(res) = active.encode_resize(aw, ah, None, None) {
                            let bytes = res.context("encodage resize")?;
                            framed.write_all(&bytes).await.context("écriture resize")?;
                        }
                    }
                    Some(Ok(Message::Binary(b))) if b.first() == Some(&6) => {
                        // ACK de rendu : RTT de bout en bout, lissé.
                        let rtt = last_send.elapsed().as_secs_f32() * 1000.0;
                        lat_ms = if lat_ms == 0.0 { rtt } else { lat_ms.mul_add(0.8, rtt * 0.2) };
                        awaiting_ack = false;
                        flush_dirty!();
                    }
                    Some(Ok(Message::Binary(b))) if b.first() == Some(&8) => {
                        // Presse-papiers du poste : on mémorise le texte et on
                        // l'annonce au serveur (collage possible dans le distant).
                        if partage_clip.load(std::sync::atomic::Ordering::Relaxed) {
                            if let Ok(text) = std::str::from_utf8(&b[1..]) {
                                if let Ok(mut g) = local_text.lock() {
                                    *g = Some(text.to_owned());
                                }
                                let _ = clip_tx.send(ClipReq::Advertise);
                            }
                        }
                    }
                    Some(Ok(Message::Binary(b))) if b.first() == Some(&12) && b.len() >= 2 => {
                        // CLIPBOARD : 0 = l'utilisateur a coupé le partage.
                        partage_clip.store(b[1] != 0, std::sync::atomic::Ordering::Relaxed);
                    }
                    Some(Ok(Message::Binary(b))) if b.first() == Some(&11) && b.len() >= 2 => {
                        // PAUSE : 1 = l'onglet est passé à l'arrière-plan.
                        en_pause = b[1] != 0;
                        if !en_pause {
                            flush_dirty!();
                        }
                    }
                    Some(Ok(Message::Binary(b))) if b.first() == Some(&9) => {
                        // REFRESH : l'onglet a été réaffiché et son canvas peut être
                        // vide ; on renvoie l'image entière, hors cadencement.
                        let full = InclusiveRectangle {
                            left: 0,
                            top: 0,
                            right: image.width().saturating_sub(1),
                            bottom: image.height().saturating_sub(1),
                        };
                        let msg = frame_msg(&image, &full);
                        stat_bytes += msg.len() as u64;
                        stat_frames += 1;
                        sink.send(Message::Binary(msg)).await.context("envoi refresh")?;
                        awaiting_ack = true;
                        last_send = Instant::now();
                        dirty.clear();
                        // Un REFRESH ne s'obtient qu'en revenant au premier plan :
                        // il lève la pause. Le flux ne peut donc pas rester gelé
                        // si l'interface oubliait de la lever explicitement.
                        en_pause = false;
                    }
                    Some(Ok(Message::Binary(b))) => {
                        // [10] ne décrit pas une frappe mais l'état des verrous :
                        // il produit son événement directement, sans passer par
                        // la base d'état des touches.
                        let events: Vec<_> = if b.first() == Some(&10) && b.len() >= 2 {
                            vec![lock_sync_event(b[1])]
                        } else {
                            db.apply(input_ops(&b)).into_vec()
                        };
                        for o in active.process_fastpath_input(&mut image, &events)? {
                            if let ActiveStageOutput::ResponseFrame(f) = o {
                                framed.write_all(&f).await.context("écriture entrée")?;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_)) | Err(_)) | None => break, // fin/erreur
                    Some(Ok(_)) => {}
                }
            }
            _ = tick.tick() => {
                // Annonce des capacités graphiques, une fois le canal ouvert et
                // dans une écriture qui lui est propre (voir `egfx::start`).
                // Deux issues selon ce que le serveur nous a laissé faire. Si
                // le canal graphique est ouvert, il ne reste qu'à annoncer nos
                // capacités — dans une écriture qui lui soit propre. S'il ne
                // l'est pas et que rien n'a été dessiné, c'est que ce serveur
                // n'a que celui-là : il faut reprendre en le lui accordant.
                const ATTENTE_EGFX: Duration = Duration::from_secs(4);
                if !dessin_classique {
                    let depuis = *silence.get_or_insert_with(Instant::now);
                    if egfx::canal_ouvert(&canal_egfx).is_some() {
                        if let Some((id, pdu)) = egfx::annonce_a_emettre(&canal_egfx) {
                            eprintln!("egfx : annonce des capacités sur le canal {id}");
                            let bytes = active
                                .process_svc_processor_messages(egfx::lot_dvc(id, pdu)?)
                                .context("encodage egfx")?;
                            framed.write_all(&bytes).await.context("écriture egfx")?;
                        }
                    } else if graphique == egfx::Politique::Observer
                        && depuis.elapsed() >= ATTENTE_EGFX
                    {
                        return Ok(Suite::ReprendreAvecGraphique);
                    }
                }
                // Un ACK perdu ne doit pas geler l'écran : au-delà du délai, on
                // considère la webview libre et on renvoie l'état courant.
                if awaiting_ack && last_send.elapsed() > ACK_TIMEOUT {
                    awaiting_ack = false;
                }
                flush_dirty!();
                if stat_window.elapsed() >= Duration::from_secs(1) {
                    let secs = stat_window.elapsed().as_secs_f32();
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
                    let fps = (stat_frames as f32 / secs).round() as u16;
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
                    let kbps = ((stat_bytes as f32 / 1024.0) / secs).round() as u32;
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let lat = lat_ms.round() as u16;
                    // STATS [7] fps:u16 kbps:u32 lat_ms:u16
                    let mut m = vec![7u8];
                    m.extend_from_slice(&fps.to_le_bytes());
                    m.extend_from_slice(&kbps.to_le_bytes());
                    m.extend_from_slice(&lat.to_le_bytes());
                    sink.send(Message::Binary(m)).await.ok();
                    stat_frames = 0;
                    stat_bytes = 0;
                    stat_window = Instant::now();
                }
            }
            Some(req) = clip_rx.recv() => {
                match req {
                    ClipReq::Advertise => {
                        let msgs = active.get_svc_processor_mut::<CliprdrClient>().and_then(|c| {
                            c.initiate_copy(&[ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT)]).ok()
                        });
                        if let Some(msgs) = msgs {
                            send_svc!(msgs);
                        }
                    }
                    ClipReq::RequestPaste(fmt) => {
                        let msgs = active
                            .get_svc_processor_mut::<CliprdrClient>()
                            .and_then(|c| c.initiate_paste(fmt).ok());
                        if let Some(msgs) = msgs {
                            send_svc!(msgs);
                        }
                    }
                    ClipReq::ServeData(resp) => {
                        let msgs = active
                            .get_svc_processor_mut::<CliprdrClient>()
                            .and_then(|c| c.submit_format_data(resp).ok());
                        if let Some(msgs) = msgs {
                            send_svc!(msgs);
                        }
                    }
                    ClipReq::RemoteText(text) => {
                        // CLIPBOARD [8] vers le front : le distant a copié du texte.
                        let mut m = vec![8u8];
                        m.extend_from_slice(text.as_bytes());
                        sink.send(Message::Binary(m)).await.context("envoi presse-papiers")?;
                    }
                }
            }
            read = framed.read_pdu() => {
                let (action, payload) = read.map_err(|e| {
                    // Même événement, même message : une coupure en pleine session
                    // affichait le code brut du système. Voir
                    // session_close_par_le_serveur.
                    let t = format!("{e} {e:?}");
                    if session_close_par_le_serveur(&t) {
                        anyhow::anyhow!(
                            "Le serveur a fermé la connexion. Si cela se produit \
                             juste après l'ouverture, c'est que la session ne \
                             démarre pas de son côté ; son journal le dira."
                        )
                    } else {
                        anyhow::Error::new(e).context("lecture PDU")
                    }
                })?;
                if let Some(m) = magneto.as_mut() {
                    m.ajouter(action, &payload);
                }
                let sorties = active.process(&mut image, action, &payload)?;
                peindre_egfx!();
                for o in sorties {
                    match o {
                        ActiveStageOutput::ResponseFrame(f) => framed.write_all(&f).await.context("écriture réponse")?,
                        ActiveStageOutput::GraphicsUpdate(rect) => {
                            dessin_classique = true;
                            dessine.store(true, std::sync::atomic::Ordering::Relaxed);
                            ajouter_rect(&mut dirty, &rect);
                            flush_dirty!();
                        }
                        ActiveStageOutput::Terminate(_) => return Ok(Suite::Fini),
                        ActiveStageOutput::Redirection(r) => return Ok(Suite::Rediriger(r)),
                        ActiveStageOutput::DeactivateAll => {
                            // Le serveur a accepté le changement de résolution : dérouler
                            // la séquence désactivation/réactivation pour renégocier.
                            let mut buf = WriteBuf::new();
                            let mut seq = activation_factory.create();
                            let size = loop {
                                single_sequence_step(&mut framed, &mut seq, &mut buf)
                                    .await
                                    .context("réactivation")?;
                                if let ConnectionActivationState::Finalized { desktop_size, share_id, .. } =
                                    seq.connection_activation_state()
                                {
                                    active.set_share_id(share_id);
                                    break desktop_size;
                                }
                            };
                            let (nw, nh) = taille_sure(size.width, size.height)?;
                            image = DecodedImage::new(PixelFormat::RgbA32, nw, nh);
                            // Annonce la nouvelle taille à Avash (réutilise CONNECTED [1][w][h]).
                            let mut msg = vec![1u8];
                            msg.extend_from_slice(&nw.to_le_bytes());
                            msg.extend_from_slice(&nh.to_le_bytes());
                            sink.send(Message::Binary(msg)).await.context("annonce resize")?;
                            // Réinitialise le cadencement : une image en attente à
                            // l'ancienne taille n'a plus de sens, et un ACK laissé en
                            // suspens gèlerait la reprise. Le serveur va renvoyer un
                            // rafraîchissement complet.
                            dirty.clear();
                            awaiting_ack = false;
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    Ok(Suite::Fini)
}

/// L'annonce de capacités graphiques à écrire, s'il y en a une.
///
/// Rendue prête à l'emploi : le PDU est déjà encadré pour le canal statique.
fn annonce_egfx(active: &mut ActiveStage, g: &Graphique<'_>) -> Result<Option<(u32, Vec<u8>)>> {
    let Some((id, pdu)) = egfx::annonce_a_emettre(g.canal) else {
        return Ok(None);
    };
    let bytes = active
        .process_svc_processor_messages(egfx::lot_dvc(id, pdu)?)
        .context("encodage egfx")?;
    Ok(Some((id, bytes)))
}

/// Ce qu'il faut savoir du canal graphique pendant une session.
struct Graphique<'a> {
    canal: &'a egfx::CanalPartage,
    file: &'a egfx::FilePartagee,
    /// Une image, une seule, a-t-elle été affichée ? C'est ce qui décide s'il
    /// faut reprendre la connexion en accordant le canal.
    dessine: &'a std::sync::atomic::AtomicBool,
}

async fn run_shot(
    active: &mut ActiveStage,
    image: &mut DecodedImage,
    framed: &mut ironrdp_tokio::TokioFramed<ironrdp_tls::TlsStream<TcpStream>>,
    path: &str,
    mut magneto: Option<&mut magnetoscope::Enregistreur>,
    g: &Graphique<'_>,
) -> Result<Suite> {
    // Deux fenêtres, selon ce que le serveur donne. Un serveur qui a commencé à
    // dessiner a tout dit en cinq secondes ; un serveur muet est peut-être un
    // pipeline graphique, qu'il faut laisser venir.
    let debut = tokio::time::Instant::now();
    // Lecture par courtes attentes plutôt qu'en un seul long blocage : un
    // serveur Windows qui attend le canal graphique n'envoie plus rien du tout,
    // et l'annonce de capacités — émise dans le corps de cette boucle — ne
    // partait jamais. Le silence est précisément le moment où il faut parler.
    loop {
        let limite = debut
            + if g.dessine.load(std::sync::atomic::Ordering::Relaxed) {
                Duration::from_secs(5)
            } else {
                Duration::from_secs(12)
            };
        if tokio::time::Instant::now() >= limite {
            break;
        }
        let lu = tokio::time::timeout(Duration::from_millis(200), framed.read_pdu()).await;
        let (action, payload) = match lu {
            Ok(Ok(v)) => v,
            Ok(Err(_)) => break,
            Err(_) => {
                // Rien n'est venu : on repasse quand même par l'entretien du
                // canal graphique ci-dessous, puis on attend de nouveau.
                if let Some((id, pdu)) = annonce_egfx(active, g)? {
                    framed.write_all(&pdu).await.context("annonce egfx")?;
                    let _ = id;
                }
                continue;
            }
        };
        if let Some(m) = magneto.as_mut() {
            m.ajouter(action, &payload);
        }
        let mut done = false;
        if let Some((_, pdu)) = annonce_egfx(active, g)? {
            framed.write_all(&pdu).await.context("annonce egfx")?;
        }
        for t in std::mem::take(&mut *g.file.lock().unwrap()).trames {
            g.dessine.store(true, std::sync::atomic::Ordering::Relaxed);
            image.peindre_rgba(t.x, t.y, t.largeur, t.hauteur, &t.pixels);
        }
        for o in active.process(image, action, &payload)? {
            match o {
                ActiveStageOutput::ResponseFrame(f) => framed.write_all(&f).await?,
                ActiveStageOutput::GraphicsUpdate(_) => {
                    g.dessine.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                ActiveStageOutput::Terminate(_) => done = true,
                // Même chemin que la session interactive : suivre la redirection
                // plutôt que de s'arrêter dessus.
                ActiveStageOutput::Redirection(r) => return Ok(Suite::Rediriger(r)),
                _ => {}
            }
        }
        if done {
            break;
        }
    }
    let buf: image::ImageBuffer<image::Rgba<u8>, _> = image::ImageBuffer::from_raw(
        u32::from(image.width()),
        u32::from(image.height()),
        image.data().to_vec(),
    )
    .context("image invalide")?;
    buf.save(path)?;
    eprintln!("capture : {path}");
    Ok(Suite::Fini)
}

#[cfg(test)]
mod tests_certificat {
    use super::{juger_certificat, VerdictCert};

    #[test]
    fn rien_de_memorise_donne_un_premier_contact() {
        assert_eq!(juger_certificat(None, "aa"), VerdictCert::PremierContact);
    }

    #[test]
    fn la_meme_empreinte_est_reconnue() {
        assert_eq!(juger_certificat(Some("aa"), "aa"), VerdictCert::Connu);
    }

    /// Le cœur du correctif : sans lui, `ironrdp_tls::upgrade` acceptait
    /// n'importe quel certificat, puis CredSSP livrait les identifiants.
    #[test]
    fn une_empreinte_differente_est_un_changement() {
        assert_eq!(
            juger_certificat(Some("aa"), "bb"),
            VerdictCert::Change {
                attendue: "aa".into()
            }
        );
    }
}

#[cfg(test)]
mod tests_acces_local {
    use super::{jetons_egaux, origine_admise};

    #[test]
    fn jetons_egaux_ne_depend_pas_de_la_position_du_premier_ecart() {
        assert!(jetons_egaux(b"0123456789abcdef", b"0123456789abcdef"));
        assert!(!jetons_egaux(b"0123456789abcdef", b"0123456789abcdeg"));
        assert!(!jetons_egaux(b"x123456789abcdef", b"0123456789abcdef"));
        // Longueurs différentes : refus sans lire plus loin.
        assert!(!jetons_egaux(b"court", b"beaucoup plus long"));
        assert!(!jetons_egaux(b"", b"x"));
        assert!(jetons_egaux(b"", b""));
    }

    #[test]
    fn une_page_web_reelle_est_refusee() {
        assert!(!origine_admise(Some("http://evil.example")));
        assert!(!origine_admise(Some("https://evil.example:8443")));
        assert!(!origine_admise(Some("https://cdn.attaquant.net/x")));
    }

    #[test]
    fn la_webview_native_et_le_developpement_passent() {
        assert!(origine_admise(None)); // pas d'en-tête : le jeton fait foi
        assert!(origine_admise(Some("tauri://localhost")));
        assert!(origine_admise(Some("http://tauri.localhost")));
        assert!(origine_admise(Some("https://tauri.localhost")));
        assert!(origine_admise(Some("http://localhost:1420"))); // vite dev
        assert!(origine_admise(Some("http://127.0.0.1:5173")));
    }

    #[test]
    fn un_sous_domaine_de_tauri_localhost_ne_passe_pas() {
        // « tauri.localhost.evil.com » ne doit pas être pris pour tauri.localhost.
        assert!(!origine_admise(Some("http://tauri.localhost.evil.com")));
    }

    #[test]
    fn la_casse_du_schema_ne_contourne_pas_le_controle() {
        // Un schéma en majuscules ne doit pas basculer dans la branche « admis ».
        assert!(!origine_admise(Some("HTTP://evil.example")));
        assert!(!origine_admise(Some("HtTpS://evil.example")));
        assert!(origine_admise(Some("HTTP://localhost:1420")));
    }

    #[test]
    fn un_schema_inattendu_est_refuse_par_defaut() {
        // Fail-closed : file://, data:, null… ne sont pas admis.
        assert!(!origine_admise(Some("file:///etc/passwd")));
        assert!(!origine_admise(Some("null")));
        assert!(!origine_admise(Some("data:text/html,x")));
        // La webview native reste admise.
        assert!(origine_admise(Some("TAURI://localhost")));
    }
}

#[cfg(test)]
mod tests_negociation {
    use super::{build_config, parse_args_de};

    /// Par défaut, seul NLA est annoncé : un serveur qui ne sait pas le faire
    /// doit échouer la négociation, pas obtenir le mot de passe dans un canal
    /// TLS sans s'être authentifié.
    #[test]
    fn par_defaut_seul_nla_est_annonce() {
        let a = parse_args_de(&["--host", "x", "-u", "u"], "p").unwrap();
        let c = build_config(&a, None);
        assert!(
            !c.enable_tls,
            "SSL annoncé : le repli de NLA vers TLS redevient possible"
        );
        assert!(c.enable_credssp);
    }

    /// `--sans-nla` rétablit l'annonce de SSL — sur décision explicite de
    /// l'utilisateur, pour un serveur qui ne propose pas NLA (un xrdp dont le
    /// module PAM n'est pas configuré, par exemple). NLA reste préféré si le
    /// serveur sait le faire : on annonce les deux, il choisit.
    #[test]
    fn sans_nla_annonce_les_deux_sans_renoncer_a_nla() {
        let a = parse_args_de(&["--host", "x", "-u", "u", "--sans-nla"], "p").unwrap();
        let c = build_config(&a, None);
        assert!(c.enable_tls);
        assert!(
            c.enable_credssp,
            "NLA doit rester préféré quand le serveur sait le faire"
        );
    }
}

#[cfg(test)]
mod tests_taille {
    use super::taille_sure;

    /// C'est le serveur qui confirme la résolution, et rien ne l'oblige à
    /// reprendre celle demandée. `DecodedImage::new` alloue largeur × hauteur × 4
    /// d'un bloc : 17 Gio pour un 65535×65535 annoncé, rejouable à volonté par
    /// renégociation. Ce plafond n'avait aucun test — et les tests du sidecar ne
    /// tournaient nulle part, ce qui n'aurait rien changé.
    #[test]
    fn une_resolution_deraisonnable_est_refusee() {
        for (w, h) in [(0, 1), (1, 0), (0, 0), (8193, 1), (1, 8193), (65535, 65535)] {
            assert!(
                taille_sure(w, h).is_err(),
                "résolution acceptée alors qu'elle ne devrait pas : {w}x{h}"
            );
        }
    }

    #[test]
    fn les_resolutions_courantes_passent() {
        for (w, h) in [(1, 1), (1920, 1080), (3440, 1440), (8192, 8192)] {
            assert_eq!(taille_sure(w, h).unwrap(), (w, h), "{w}x{h} refusée à tort");
        }
    }
}

#[cfg(test)]
mod tests_fichier_empreintes {
    use super::chercher_empreinte;

    /// Ne rien trouver vaut « premier contact », donc acceptation et
    /// mémorisation : toute entrée que la recherche rate revient à désarmer le
    /// TOFU pour cet hôte, en silence. Ces cas-là méritaient un test.
    #[test]
    fn une_entree_presente_est_retrouvee() {
        let contenu = "a:3389 aaaa\nsrv.exemple:3389 bbbb\nz:3389 cccc\n";
        assert_eq!(
            chercher_empreinte(contenu, "srv.exemple:3389").as_deref(),
            Some("bbbb")
        );
        // Dernière ligne sans saut de ligne final.
        assert_eq!(chercher_empreinte("x:1 dd", "x:1").as_deref(), Some("dd"));
    }

    #[test]
    fn un_fichier_vide_ou_abime_ne_fait_pas_trouver_n_importe_quoi() {
        for contenu in ["", "\n\n", "ligne-sans-espace\n", "  \n"] {
            assert_eq!(chercher_empreinte(contenu, "srv:3389"), None, "{contenu:?}");
        }
    }

    #[test]
    fn une_cle_voisine_ne_correspond_pas() {
        let contenu = "srv.exemple:3389 bbbb\n";
        for cle in [
            "srv.exemple:3390",
            "srv.exemple",
            "srv.exemple:33890",
            "rv.exemple:3389",
        ] {
            assert_eq!(chercher_empreinte(contenu, cle), None, "{cle}");
        }
    }

    /// Deux entrées pour le même hôte : c'est la première qui fait foi, et elle
    /// doit être trouvée — sans quoi une ligne ajoutée en fin de fichier
    /// masquerait l'empreinte d'origine.
    #[test]
    fn avash_home_detourne_le_fichier_de_confiance() {
        // Sans cela, la suite bout en bout sous Windows écrirait dans le
        // fichier réel de l'utilisateur : `config_dir()` y ignore HOME.
        let bac = std::env::temp_dir().join(format!("avash-rdp-{}", std::process::id()));
        let precedent = std::env::var_os("AVASH_HOME");
        unsafe { std::env::set_var("AVASH_HOME", &bac) };
        let sous_bac = crate::chemin_empreintes().expect("un chemin");
        unsafe {
            match precedent {
                Some(v) => std::env::set_var("AVASH_HOME", v),
                None => std::env::remove_var("AVASH_HOME"),
            }
        }
        assert!(
            sous_bac.starts_with(&bac),
            "le fichier de confiance doit suivre AVASH_HOME, or il pointe sur {sous_bac:?}"
        );
        assert!(sous_bac.ends_with("rdp_known_hosts"));
    }

    #[test]
    fn la_premiere_entree_fait_foi() {
        let contenu = "srv:3389 originale\nsrv:3389 ajoutee\n";
        assert_eq!(
            chercher_empreinte(contenu, "srv:3389").as_deref(),
            Some("originale")
        );
    }
}

#[cfg(test)]
mod tests_disposition {
    use super::{analyser_disposition, disposition_pour_code};

    #[test]
    fn les_dispositions_courantes_sont_reconnues() {
        assert_eq!(disposition_pour_code("fr"), Some(0x0000_040C));
        assert_eq!(disposition_pour_code("de"), Some(0x0000_0407));
        assert_eq!(disposition_pour_code("us"), Some(0x0000_0409));
        assert_eq!(disposition_pour_code("be"), Some(0x0000_080C));
    }

    #[test]
    fn une_liste_xkb_ne_retient_que_la_premiere() {
        // KDE écrit « LayoutList=fr,us » quand deux dispositions coexistent.
        assert_eq!(disposition_pour_code("fr,us"), Some(0x0000_040C));
        // Et setxkbmap rend parfois « fr(azerty) ».
        assert_eq!(disposition_pour_code("fr(azerty)"), Some(0x0000_040C));
    }

    #[test]
    fn une_disposition_inconnue_ne_donne_rien() {
        // Mieux vaut le défaut du serveur qu'une disposition inventée.
        assert_eq!(disposition_pour_code("klingon"), None);
        assert_eq!(disposition_pour_code(""), None);
    }

    #[test]
    fn l_argument_accepte_hexa_decimal_et_code() {
        assert_eq!(analyser_disposition("0x40c"), Some(0x40C));
        assert_eq!(analyser_disposition("1036"), Some(1036));
        assert_eq!(analyser_disposition(" fr "), Some(0x0000_040C));
        assert_eq!(analyser_disposition("n'importe quoi"), None);
    }
}

#[cfg(test)]
mod tests_zone_sale {
    use super::{ajouter_rect, RECTS_MAX};
    use ironrdp::pdu::geometry::InclusiveRectangle;

    fn r(l: u16, t: u16, ri: u16, b: u16) -> InclusiveRectangle {
        InclusiveRectangle {
            left: l,
            top: t,
            right: ri,
            bottom: b,
        }
    }

    #[test]
    fn deux_zones_voisines_fusionnent() {
        // Côte à côte : l'union ne coûte pas plus que les deux séparés.
        let mut z = Vec::new();
        ajouter_rect(&mut z, &r(0, 0, 9, 9));
        ajouter_rect(&mut z, &r(10, 0, 19, 9));
        assert_eq!(z.len(), 1, "deux zones contiguës doivent n'en faire qu'une");
        assert_eq!((z[0].left, z[0].right), (0, 19));
    }

    #[test]
    fn deux_coins_opposes_ne_fusionnent_pas() {
        // C'est LE cas qui envoyait un plein écran pour deux poussières.
        let mut z = Vec::new();
        ajouter_rect(&mut z, &r(0, 0, 9, 9));
        ajouter_rect(&mut z, &r(1200, 700, 1209, 709));
        assert_eq!(z.len(), 2, "deux coins opposés doivent rester séparés");
    }

    #[test]
    fn un_rectangle_inclus_disparait_dans_le_sien() {
        let mut z = Vec::new();
        ajouter_rect(&mut z, &r(0, 0, 99, 99));
        ajouter_rect(&mut z, &r(10, 10, 19, 19));
        assert_eq!(z.len(), 1);
        assert_eq!((z[0].right, z[0].bottom), (99, 99));
    }

    #[test]
    fn le_nombre_de_rectangles_reste_borne() {
        // Une trame ne peut pas porter un nombre illimité de zones : au-delà du
        // plafond, la paire la moins coûteuse fusionne.
        let mut z = Vec::new();
        for i in 0..40u16 {
            let x = i * 30;
            ajouter_rect(&mut z, &r(x, x, x + 5, x + 5));
        }
        assert!(
            z.len() <= RECTS_MAX,
            "zone non bornée : {} rectangles",
            z.len()
        );
    }

    #[test]
    fn des_rectangles_qui_se_chevauchent_au_dela_du_plafond_ne_paniquent_pas() {
        // Bug corrigé : le choix de la paire à fusionner calculait
        // aire(union) - aire(a) - aire(b) ; pour deux rectangles qui se recouvrent,
        // l'union est plus petite que la somme → soustraction u64 négative, panique
        // en debug/test. Il faut PLUS de RECTS_MAX rectangles, dont certains se
        // chevauchent, pour entrer dans la boucle de fusion fautive.
        let mut z = Vec::new();
        for i in 0..(RECTS_MAX as u16 + 4) {
            // Des rectangles largement recouvrants (pas seulement inclus l'un dans
            // l'autre, sinon ils fusionneraient avant d'atteindre la boucle).
            ajouter_rect(&mut z, &r(i * 3, i * 3, i * 3 + 40, i * 3 + 40));
        }
        assert!(z.len() <= RECTS_MAX, "zone non bornée : {}", z.len());
    }

    #[test]
    fn la_zone_couvre_toujours_tout_ce_qui_a_ete_signale() {
        // Propriété essentielle : on peut fusionner, jamais PERDRE un pixel sale.
        let mut z = Vec::new();
        let entrees = [
            r(5, 5, 9, 9),
            r(700, 400, 720, 420),
            r(1200, 10, 1210, 20),
            r(300, 300, 305, 305),
        ];
        for e in &entrees {
            ajouter_rect(&mut z, e);
        }
        for e in &entrees {
            assert!(
                z.iter().any(|c| c.left <= e.left
                    && c.top <= e.top
                    && c.right >= e.right
                    && c.bottom >= e.bottom),
                "le rectangle {e:?} n'est couvert par aucune zone"
            );
        }
    }
}

#[cfg(test)]
mod tests_entrees_hostiles {
    use super::input_ops;

    /// Générateur déterministe : un échec doit pouvoir être rejoué à
    /// l'identique. Un test aléatoire non reproductible n'aide personne.
    fn suite(graine: u64) -> impl FnMut() -> u64 {
        let mut e = graine;
        move || {
            e ^= e << 13;
            e ^= e >> 7;
            e ^= e << 17;
            e
        }
    }

    #[test]
    fn aucun_message_malforme_ne_fait_paniquer() {
        // Ces octets viennent du canal local. Il est authentifié par jeton, mais
        // un client authentifié reste un client : rien ne garantit qu'il envoie
        // des messages bien formés — un bogue d'interface suffit. Une analyse
        // qui panique ferait tomber une session RDP déjà établie.
        let mut alea = suite(0x5eed_1234_abcd_ef01);
        for _ in 0..20_000 {
            let n = (alea() % 24) as usize;
            let mut b = Vec::with_capacity(n);
            for _ in 0..n {
                b.push((alea() & 0xff) as u8);
            }
            let _ = input_ops(&b);
        }
    }

    #[test]
    fn chaque_type_connu_tronque_a_toutes_les_longueurs() {
        // Le vrai piège n'est pas l'octet aléatoire mais le message VALIDE
        // coupé trop tôt : le type est reconnu, la charge manque.
        for type_msg in 0u8..=13 {
            for longueur in 0..20usize {
                let mut b = vec![type_msg];
                b.extend(std::iter::repeat_n(0xa5u8, longueur));
                let _ = input_ops(&b);
            }
        }
    }

    #[test]
    fn un_message_vide_ne_produit_rien() {
        assert!(input_ops(&[]).is_empty());
    }
}

#[cfg(test)]
mod tests_fin_de_session {
    use super::session_close_par_le_serveur;

    #[test]
    fn l_ultimatum_est_reconnu() {
        assert!(session_close_par_le_serveur(
            "decode error other (received disconnect provider ultimatum)"
        ));
    }

    #[test]
    fn la_coupure_tcp_windows_est_reconnue() {
        // 10054 = WSAECONNRESET. Le même événement que l'ultimatum, mais vu par
        // le système : c'est le code brut qu'Adrien a reçu sous Windows.
        assert!(session_close_par_le_serveur(
            "lecture PDU: Une connexion existante a dû être fermée (os error 10054)"
        ));
    }

    #[test]
    fn la_coupure_tcp_unix_est_reconnue() {
        assert!(session_close_par_le_serveur(
            "lecture PDU: Connection reset by peer (os error 104)"
        ));
    }

    #[test]
    fn une_erreur_sans_rapport_ne_l_est_pas() {
        // Sans quoi tout échec porterait un message rassurant et faux.
        assert!(!session_close_par_le_serveur(
            "InvalidToken: CredSSP server returned an error status; status is STATUS_LOGON_FAILURE"
        ));
        assert!(!session_close_par_le_serveur(
            "connexion TCP à 10.0.0.1:3389: timed out"
        ));
    }
}

#[cfg(test)]
mod tests_coupure {
    use super::est_coupure;

    #[test]
    fn le_code_windows_est_reconnu() {
        // 10054 = WSAECONNRESET : le code brut qu'un utilisateur reçoit sans
        // pouvoir en rien conclure.
        assert!(est_coupure(
            "début de connexion: Une connexion existante a dû être fermée (os error 10054)"
        ));
    }

    #[test]
    fn le_code_unix_et_la_fin_de_flux_sont_reconnus() {
        assert!(est_coupure("Connection reset by peer (os error 104)"));
        assert!(est_coupure("unexpected end of file"));
    }

    #[test]
    fn un_echec_ordinaire_ne_l_est_pas() {
        // Sans quoi un mauvais mot de passe proposerait de renoncer à NLA.
        assert!(!est_coupure("STATUS_LOGON_FAILURE"));
        assert!(!est_coupure("connexion TCP à 10.0.0.1:3389: timed out"));
        assert!(!est_coupure("Le certificat de 10.0.0.1:3389 a changé."));
    }
}

#[cfg(test)]
mod tests_jeton {
    use super::valeur_du_jeton;

    #[test]
    fn le_prefixe_et_le_terminateur_sont_retires() {
        // Ce que GNOME Remote Desktop envoie réellement.
        assert_eq!(
            valeur_du_jeton(b"Cookie: msts=2464288595\r\n"),
            "2464288595"
        );
    }

    #[test]
    fn une_valeur_deja_nue_passe_telle_quelle() {
        assert_eq!(valeur_du_jeton(b"2464288595"), "2464288595");
    }

    #[test]
    fn un_jeton_vide_ne_panique_pas() {
        assert_eq!(valeur_du_jeton(b""), "");
        assert_eq!(valeur_du_jeton(b"Cookie: msts=\r\n"), "");
    }
}

#[cfg(test)]
mod tests_rdstls {
    use super::verdict_rdstls;

    #[test]
    fn les_codes_connus_sont_traduits() {
        assert!(verdict_rdstls(0x0000_052e).contains("ne reconnaît pas les identifiants"));
        assert!(verdict_rdstls(0x0000_0775).contains("verrouillé"));
    }

    #[test]
    fn un_code_inconnu_reste_lisible() {
        let m = verdict_rdstls(0x0000_dead);
        assert!(m.contains("raison inconnue"));
        assert!(
            m.contains("0x0000dead"),
            "le code brut doit rester consultable : {m}"
        );
    }

    #[test]
    fn le_message_decharge_l_utilisateur() {
        // Ces identifiants sont engendrés par le serveur : accuser une faute de
        // frappe enverrait chercher au mauvais endroit.
        assert!(
            verdict_rdstls(0x0000_052e).contains("pas une \u{fffd}rreur de saisie")
                || verdict_rdstls(0x0000_052e).contains("erreur de saisie")
        );
    }
}

#[cfg(test)]
mod tests_identifiants {
    use super::split_credentials;

    /// NLA/CredSSP attend le domaine à part ; l'utilisateur, lui, le tape
    /// comme il l'a toujours fait. Les deux formes courantes sont découpées, et
    /// `--domain` explicite laisse le nom intact.
    #[test]
    fn les_deux_formes_de_domaine_sont_decoupees() {
        assert_eq!(
            split_credentials("TEST\\adrien", None),
            ("adrien".to_owned(), Some("TEST".to_owned()))
        );
        assert_eq!(
            split_credentials("adrien@exemple.local", None),
            ("adrien".to_owned(), Some("exemple.local".to_owned()))
        );
    }

    #[test]
    fn sans_domaine_le_nom_reste_entier() {
        assert_eq!(
            split_credentials("adrien", None),
            ("adrien".to_owned(), None)
        );
    }

    #[test]
    fn un_domaine_explicite_prime_et_laisse_le_nom_tel_quel() {
        // Un compte contenant un « @ » légitime ne doit pas être redécoupé
        // quand l'appelant a déjà dit le domaine.
        assert_eq!(
            split_credentials("adrien@exemple.local", Some("AUTRE")),
            ("adrien@exemple.local".to_owned(), Some("AUTRE".to_owned()))
        );
    }
}

#[cfg(test)]
mod tests_trames {
    use super::{frame_msg, frames_msg, mouse_button};
    use ironrdp::graphics::image_processing::PixelFormat;
    use ironrdp::pdu::geometry::InclusiveRectangle;
    use ironrdp::session::image::DecodedImage;

    fn image_numerotee(l: u16, h: u16) -> DecodedImage {
        // Chaque pixel porte sa position : un rectangle mal découpé se voit.
        let mut image = DecodedImage::new(PixelFormat::RgbA32, l, h);
        let pixels: Vec<u8> = (0..usize::from(l) * usize::from(h))
            .flat_map(|i| [(i % 256) as u8, (i / 256) as u8, 0xAA, 0xFF])
            .collect();
        image.peindre_rgba(0, 0, l, h, &pixels);
        image
    }

    fn r(l: u16, t: u16, ri: u16, b: u16) -> InclusiveRectangle {
        InclusiveRectangle {
            left: l,
            top: t,
            right: ri,
            bottom: b,
        }
    }

    /// Le format binaire est le contrat avec l'interface (`ws.onmessage`) : un
    /// octet de type, quatre u16 petit-boutiens, puis les pixels ligne par
    /// ligne. Aucun test ne le fixait.
    #[test]
    fn une_trame_simple_porte_sa_geometrie_et_ses_pixels() {
        let image = image_numerotee(8, 4);
        let m = frame_msg(&image, &r(2, 1, 4, 2)); // 3 × 2 pixels
        assert_eq!(m[0], 2);
        assert_eq!(&m[1..9], &[2, 0, 1, 0, 3, 0, 2, 0]);
        assert_eq!(m.len(), 9 + 3 * 2 * 4);
        // Premier pixel copié = position (2,1) = indice 1*8+2 = 10.
        assert_eq!(&m[9..13], &[10, 0, 0xAA, 0xFF]);
        // Première ligne complète : indices 10, 11, 12 ; puis 18, 19, 20.
        assert_eq!(m[9 + 3 * 4], 18, "la seconde ligne suit le pas de l'image");
    }

    #[test]
    fn un_seul_rectangle_garde_la_forme_historique() {
        let image = image_numerotee(8, 4);
        let zone = [r(0, 0, 1, 1)];
        assert_eq!(frames_msg(&image, &zone), frame_msg(&image, &zone[0]));
    }

    #[test]
    fn plusieurs_rectangles_sont_concatenes_avec_leur_compte() {
        let image = image_numerotee(8, 4);
        let zone = [r(0, 0, 1, 0), r(6, 3, 7, 3)]; // 2×1 chacun
        let m = frames_msg(&image, &zone);
        assert_eq!(m[0], 13);
        assert_eq!(m[1], 2, "nombre de rectangles");
        assert_eq!(m.len(), 2 + 2 * (8 + 2 * 4));
        assert_eq!(&m[2..10], &[0, 0, 0, 0, 2, 0, 1, 0]);
        assert_eq!(&m[10..14], &[0, 0, 0xAA, 0xFF]);
        let second = 2 + 8 + 8;
        assert_eq!(&m[second..second + 8], &[6, 0, 3, 0, 2, 0, 1, 0]);
        // (6,3) = indice 3*8+6 = 30.
        assert_eq!(&m[second + 8..second + 12], &[30, 0, 0xAA, 0xFF]);
    }

    #[test]
    fn les_boutons_de_souris_suivent_la_convention_du_front() {
        use ironrdp::input::MouseButton;
        // 0 = gauche (et tout inconnu), 1 = milieu, 2 = droit — l'ordre de
        // `MouseEvent.button` du navigateur, transmis tel quel.
        assert_eq!(mouse_button(0), MouseButton::Left);
        assert_eq!(mouse_button(1), MouseButton::Middle);
        assert_eq!(mouse_button(2), MouseButton::Right);
        assert_eq!(mouse_button(3), MouseButton::X1);
        assert_eq!(mouse_button(4), MouseButton::X2);
        assert_eq!(mouse_button(200), MouseButton::Left);
    }
}

#[cfg(test)]
mod tests_configuration {
    use super::{build_config, parse_args_de};
    use ironrdp::connector::Credentials;
    use ironrdp::session::redirection::Redirection;

    fn redirection() -> Redirection {
        Redirection {
            session_id: 7,
            drapeaux: 0,
            adresse: None,
            jeton: Some(b"Cookie: msts=2464288595\r\n".to_vec()),
            utilisateur: Some("69<;349v".to_owned()),
            domaine: None,
            mot_de_passe: Some(b"secret".to_vec()),
            fqdn: None,
            guid: None,
            utilisateur_brut: None,
            domaine_brut: None,
        }
    }

    /// Après une redirection, ce sont les identifiants du serveur — engendrés
    /// pour l'occasion — qui partent, et le jeton de routage est replacé dans
    /// la requête X.224. Sans quoi GNOME Remote Desktop renvoie à l'accueil,
    /// indéfiniment.
    #[test]
    fn une_redirection_impose_ses_identifiants_et_son_jeton() {
        let a = parse_args_de(&["--host", "x", "-u", "adrien"], "mdp").unwrap();
        let c = build_config(&a, Some(&redirection()));
        match c.credentials {
            Credentials::UsernamePassword { username, password } => {
                assert_eq!(username, "69<;349v");
                assert_eq!(password, "secret");
            }
            autre => panic!("identifiants inattendus : {autre:?}"),
        }
        assert!(
            c.request_data.is_some(),
            "le jeton de routage doit être posé"
        );
    }

    #[test]
    fn sans_redirection_ce_sont_ceux_de_l_utilisateur() {
        let a = parse_args_de(&["--host", "x", "-u", "TEST\\adrien"], "mdp").unwrap();
        let c = build_config(&a, None);
        match c.credentials {
            Credentials::UsernamePassword { username, password } => {
                assert_eq!(username, "adrien");
                assert_eq!(password, "mdp");
            }
            autre => panic!("identifiants inattendus : {autre:?}"),
        }
        assert_eq!(c.domain.as_deref(), Some("TEST"));
        assert!(c.request_data.is_none());
    }
}
