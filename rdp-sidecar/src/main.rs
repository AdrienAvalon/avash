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

fn build_config(a: &Args) -> connector::Config {
    let (username, domain) = split_credentials(&a.user, a.domain.as_deref());
    connector::Config {
        credentials: Credentials::UsernamePassword {
            username,
            password: a.pass.clone(),
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
        request_data: None,
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

    // Écriture atomique, comme le fait le cœur pour ses propres fichiers. Le
    // sidecar ne dépend pas du crate `avash`, la fonction n'y était donc pas —
    // alors que c'est ce fichier-ci qui compte le plus : le perdre ramène TOUS
    // les serveurs à « premier contact », et le TOFU cesse de protéger sans que
    // rien ne le signale. Une lecture-modification-écriture non atomique perdait
    // aussi l'empreinte d'un premier contact concurrent.
    let tmp = chemin.with_extension(format!("tmp{}", std::process::id()));
    {
        use std::io::Write as _;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut f = options.open(&tmp)?;
        f.write_all(contenu.as_bytes())?;
        f.sync_all()?;
    }
    if let Err(e) = std::fs::rename(&tmp, &chemin) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&chemin, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
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

async fn connect(
    a: &Args,
    clip_backend: ClipBackend,
) -> Result<(
    connector::ConnectionResult,
    ironrdp_tokio::TokioFramed<ironrdp_tls::TlsStream<TcpStream>>,
)> {
    let tcp = TcpStream::connect((a.host.as_str(), a.port))
        .await
        .with_context(|| format!("connexion TCP à {}:{}", a.host, a.port))?;
    // Nagle OFF : les entrées et les petits rectangles d'écran partent sans délai.
    tcp.set_nodelay(true).ok();
    let client_addr = tcp.local_addr()?;
    let mut framed = ironrdp_tokio::TokioFramed::new(tcp);
    // Canal Display Control (DVC) : permet le redimensionnement natif du
    // bureau distant (le serveur re-rend à la nouvelle résolution).
    let mut connector = connector::ClientConnector::new(build_config(a), client_addr)
        .with_static_channel(
            DrdynvcClient::new()
                .with_dynamic_channel(DisplayControlClient::new(|_caps| Ok(Vec::new()))),
        )
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
        Err(e) => return Err(e).context("début de connexion"),
    };
    let initial = framed.into_inner_no_leftover();
    let (upgraded_stream, cert) = ironrdp_tls::upgrade(initial, &a.host)
        .await
        .context("passage TLS")?;
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
        if texte.contains("disconnect provider ultimatum") {
            anyhow::anyhow!(
                "Le serveur a accepté vos identifiants puis a mis fin à la session \
                 avant de l'ouvrir. L'authentification n'est pas en cause : c'est \
                 côté serveur que la session ne démarre pas (compte sans session \
                 autorisée, service de session en échec, ou poste déjà occupé)."
            )
        } else {
            anyhow::Error::new(e).context("fin de la séquence de connexion")
        }
    })?;
    Ok((result, framed))
}

#[tokio::main]
async fn main() -> Result<()> {
    if std::env::var_os("RUST_LOG").is_some() {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_writer(std::io::stderr)
            .init();
    }
    let args = parse_args()?;
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
    let (result, mut framed) =
        match tokio::time::timeout(DELAI_CONNEXION, connect(&args, clip_backend)).await {
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

    if let Some(path) = args.shot.clone() {
        return run_shot(&mut active, &mut image, &mut framed, &path).await;
    }

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
    let (mut sink, mut stream) = loop {
        let (tcp, _) = listener.accept().await.context("acceptation WebSocket")?;
        tcp.set_nodelay(true).ok();
        let Ok(Ok(ws)) =
            tokio::time::timeout(DELAI_POIGNEE, tokio_tungstenite::accept_async(tcp)).await
        else {
            continue; // poignée de main absente ou trop lente : au suivant
        };
        let (sink, mut stream) = ws.split();
        // Premier message du client = le jeton (sinon on refuse ce client-là).
        match tokio::time::timeout(DELAI_POIGNEE, stream.next()).await {
            Ok(Some(Ok(Message::Binary(t)))) if t == token.as_bytes() => break (sink, stream),
            _ => continue,
        }
    };

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
    let mut dirty: Option<InclusiveRectangle> = None;
    let mut awaiting_ack = false;
    // Onglet masqué : le canvas n'est pas à l'écran, mais l'accusé de rendu
    // partait quand même — le serveur voyait la voie libre en permanence et le
    // sidecar continuait à décoder et à pousser des trames pleines (8 Mo en
    // 1080p) que personne ne regardait. En pause, on accumule le rectangle sale
    // sans rien émettre ; le retour au premier plan demande un REFRESH.
    let mut en_pause = false;
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
                if let Some(r) = dirty.take() {
                    let msg = frame_msg(&image, &r);
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
                        dirty = None;
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
                let (action, payload) = read.context("lecture PDU")?;
                for o in active.process(&mut image, action, &payload)? {
                    match o {
                        ActiveStageOutput::ResponseFrame(f) => framed.write_all(&f).await.context("écriture réponse")?,
                        ActiveStageOutput::GraphicsUpdate(rect) => {
                            dirty = Some(dirty.map_or(rect.clone(), |d| InclusiveRectangle {
                                left: d.left.min(rect.left),
                                top: d.top.min(rect.top),
                                right: d.right.max(rect.right),
                                bottom: d.bottom.max(rect.bottom),
                            }));
                            flush_dirty!();
                        }
                        ActiveStageOutput::Terminate(_) => return Ok(()),
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
                            dirty = None;
                            awaiting_ack = false;
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    Ok(())
}

async fn run_shot(
    active: &mut ActiveStage,
    image: &mut DecodedImage,
    framed: &mut ironrdp_tokio::TokioFramed<ironrdp_tls::TlsStream<TcpStream>>,
    path: &str,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while let Ok(Ok((action, payload))) = tokio::time::timeout_at(deadline, framed.read_pdu()).await
    {
        let mut done = false;
        for o in active.process(image, action, &payload)? {
            match o {
                ActiveStageOutput::ResponseFrame(f) => framed.write_all(&f).await?,
                ActiveStageOutput::Terminate(_) => done = true,
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
    Ok(())
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
mod tests_negociation {
    use super::{build_config, parse_args_de};

    /// Par défaut, seul NLA est annoncé : un serveur qui ne sait pas le faire
    /// doit échouer la négociation, pas obtenir le mot de passe dans un canal
    /// TLS sans s'être authentifié.
    #[test]
    fn par_defaut_seul_nla_est_annonce() {
        let a = parse_args_de(&["--host", "x", "-u", "u"], "p").unwrap();
        let c = build_config(&a);
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
        let c = build_config(&a);
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
