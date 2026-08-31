//! Avash — sidecar client RDP (IronRDP), isolé de russh.
//!
//! Sert le bureau distant à Avash via un **WebSocket local binaire** (vrai
//! ArrayBuffer côté webview : pas de base64, pas de JSON — débit maximal, même
//! en 3440×1440). Écoute sur 127.0.0.1:<port aléatoire> et n'accepte qu'un
//! client présentant le bon jeton. Imprime « PORT TOKEN » sur stdout au départ.
//!
//! Messages WebSocket (binaires, auto-délimités) :
//!   sidecar -> app : [1]=CONNECTED w:u16 h:u16 · [2]=FRAME x,y,w,h:u16 + RGBA · [3]=ERROR utf8
//!   app -> sidecar : [1]MOUSE_MOVE x,y · [2]BUTTON b,down,x,y · [3]WHEEL delta:i16 · [4]KEY sc:u16,down · [5]RESIZE w,h · [6]ACK · [8]CLIPBOARD utf8 · [9]REFRESH · [10]LOCKS bits:u8
//!
//! Usage : avash-rdp --host H [--port 3389] -u USER -p PASS [--width W --height H] [--domain D] [--shot out.png]

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
    Ok(Args {
        host: a.opt("--host").context("argument requis : --host")?,
        port: a.opt("--port").and_then(|s| s.parse().ok()).unwrap_or(3389),
        user: a.req2("-u", "--username")?,
        pass: read_password(&a)?,
        domain: a.opt("--domain"),
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

fn build_config(a: &Args) -> connector::Config {
    let (username, domain) = split_credentials(&a.user, a.domain.as_deref());
    connector::Config {
        credentials: Credentials::UsernamePassword {
            username,
            password: a.pass.clone(),
        },
        domain,
        enable_tls: true,
        enable_credssp: true,
        keyboard_type: KeyboardType::IbmEnhanced,
        keyboard_subtype: 0,
        keyboard_layout: 0,
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
fn chemin_empreintes() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("avash")
        .join("rdp_known_hosts")
}

/// Empreinte mémorisée pour `hote:port`, s'il y en a une.
fn empreinte_memorisee(cle: &str) -> Option<String> {
    let contenu = std::fs::read_to_string(chemin_empreintes()).ok()?;
    contenu.lines().find_map(|l| {
        let (h, e) = l.split_once(' ')?;
        (h == cle).then(|| e.trim().to_owned())
    })
}

/// Mémorise l'empreinte d'un hôte au premier contact.
fn memoriser_empreinte(cle: &str, emp: &str) -> Result<()> {
    let chemin = chemin_empreintes();
    if let Some(parent) = chemin.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut contenu = std::fs::read_to_string(&chemin).unwrap_or_default();
    if !contenu.is_empty() && !contenu.ends_with('\n') {
        contenu.push('\n');
    }
    contenu.push_str(&format!("{cle} {emp}\n"));
    std::fs::write(&chemin, contenu)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
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
    let should_upgrade = ironrdp_tokio::connect_begin(&mut framed, &mut connector)
        .await
        .context("début de connexion")?;
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
    .context("finalisation (CredSSP/NLA)")?;
    Ok((result, framed))
}

#[tokio::main]
async fn main() -> Result<()> {
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
    let (result, mut framed) = connect(&args, clip_backend).await?;
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
