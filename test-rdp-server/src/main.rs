//! Serveur RDP de TEST pour Avash — adapté de l'exemple `server.rs` d'`IronRDP`
//! (MIT/Apache-2.0). Sert un faux bureau (rectangles aléatoires) en TLS/NLA
//! pour valider le sidecar `avash-rdp` sans machine Windows, avec un son, un
//! presse-papiers et, dès qu'un client annonce un lecteur redirigé, le
//! scénario rdpdr de `src/rdpdr/` dont chaque étape s'écrit sur la sortie
//! standard.
//!
//! Example of utilizing `ironrdp-server` crate.

#![allow(unused_crate_dependencies)] // False positives because there are both a library and a binary.
#![allow(clippy::print_stdout)]

mod rdpdr;

use core::net::SocketAddr;
use core::num::{NonZeroU16, NonZeroUsize};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Context as _;
use ironrdp::cliprdr::backend::{ClipboardMessage, CliprdrBackend, CliprdrBackendFactory};
use ironrdp::cliprdr::pdu::{
    ClipboardFileAttributes, ClipboardFormat, ClipboardFormatId, ClipboardFormatName,
    ClipboardGeneralCapabilityFlags, FileContentsFlags, FileContentsRequest, FileContentsResponse,
    FileDescriptor, FormatDataRequest, FormatDataResponse, LockDataId,
};
use ironrdp::connector::DesktopSize;
use ironrdp::core::IntoOwned as _;
use ironrdp::rdpsnd::pdu::{AudioFormat, WaveFormat};
use ironrdp::rdpsnd::server::{
    NegotiatedFormat, RdpsndError, RdpsndServerHandler, RdpsndServerMessage,
};
use ironrdp::server::tokio::sync::mpsc::UnboundedSender;
use ironrdp::server::tokio::time::{self, sleep, Duration};
use ironrdp::server::{
    tokio, BitmapUpdate, CliprdrServerFactory, Credentials, DisplayUpdate, KeyboardEvent,
    MouseEvent, PixelFormat, RdpServer, RdpServerDisplay, RdpServerDisplayUpdates,
    RdpServerInputHandler, ServerEvent, ServerEventSender, SoundServerFactory, TlsIdentityCtx,
};
use rand::prelude::*;
use tracing::{debug, info, warn};

const HELP: &str = "\
USAGE:
  cargo run --example=server -- [--bind-addr <SOCKET ADDRESS>] [--cert <CERTIFICATE>] [--key <CERTIFICATE KEY>] [--user USERNAME] [--pass PASSWORD] [--sec tls|hybrid]
                                [--offrir <FICHIER>] [--recevoir-dans <DOSSIER>] [--mire <HZ>]

  --mire <HZ>  pousse une image plein écran neuve HZ fois par seconde, au lieu
               d'un rectangle de bruit toutes les 100 ms : la charge du banc
               d'affichage (e2e/mesures/rdp.spec.js).
";

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), anyhow::Error> {
    let action = match parse_args() {
        Ok(action) => action,
        Err(e) => {
            println!("{HELP}");
            return Err(e.context("invalid argument(s)"));
        }
    };

    setup_logging()?;

    match action {
        Action::ShowHelp => {
            println!("{HELP}");
            Ok(())
        }
        Action::Run {
            bind_addr,
            hybrid,
            user,
            pass,
            cert,
            key,
            offrir,
            recevoir_dans,
            mire,
        } => {
            run(
                bind_addr,
                hybrid,
                user,
                pass,
                cert,
                key,
                offrir,
                recevoir_dans,
                mire,
            )
            .await
        }
    }
}

#[derive(Debug)]
enum Action {
    ShowHelp,
    Run {
        bind_addr: SocketAddr,
        hybrid: bool,
        user: String,
        pass: String,
        cert: Option<PathBuf>,
        key: Option<PathBuf>,
        /// Fichier offert par le presse-papiers quand le client copie le
        /// texte déclencheur.
        offrir: Option<PathBuf>,
        /// Dossier où reçoivent les fichiers que le client offre.
        recevoir_dans: Option<PathBuf>,
        /// Cadence de la mire plein écran, en images par seconde.
        mire: Option<u32>,
    },
}

fn parse_args() -> anyhow::Result<Action> {
    let mut args = pico_args::Arguments::from_env();

    let action = if args.contains(["-h", "--help"]) {
        Action::ShowHelp
    } else {
        let bind_addr = args.opt_value_from_str("--bind-addr")?.unwrap_or_else(|| {
            "127.0.0.1:3389"
                .parse()
                .expect("valid hardcoded SocketAddr string")
        });

        let sec = args
            .opt_value_from_str("--sec")?
            .unwrap_or_else(|| "hybrid".to_owned());
        let hybrid = match sec.as_ref() {
            "tls" => false,
            "hybrid" => true,
            _ => anyhow::bail!("Unhandled security: '{sec}'"),
        };

        let cert = args.opt_value_from_str("--cert")?;
        let key = args.opt_value_from_str("--key")?;

        let user = args
            .opt_value_from_str("--user")?
            .unwrap_or_else(|| "user".to_owned());
        let pass = args
            .opt_value_from_str("--pass")?
            .unwrap_or_else(|| "pass".to_owned());
        let offrir = args.opt_value_from_str("--offrir")?;
        let recevoir_dans = args.opt_value_from_str("--recevoir-dans")?;
        let mire: Option<u32> = args.opt_value_from_str("--mire")?;
        if mire.is_some_and(|hz| hz == 0 || hz > 240) {
            anyhow::bail!("--mire attend une cadence entre 1 et 240 images par seconde");
        }

        Action::Run {
            bind_addr,
            hybrid,
            user,
            pass,
            cert,
            key,
            offrir,
            recevoir_dans,
            mire,
        }
    };

    Ok(action)
}

fn setup_logging() -> anyhow::Result<()> {
    use tracing::metadata::LevelFilter;
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::EnvFilter;

    let fmt_layer = tracing_subscriber::fmt::layer().compact();

    let env_filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::WARN.into())
        .with_env_var("IRONRDP_LOG")
        .from_env_lossy();

    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(env_filter)
        .try_init()
        .context("failed to set tracing global subscriber")?;

    Ok(())
}

#[derive(Clone, Debug)]
struct Handler {
    mire: Option<u32>,
    /// Le drapeau que la bibliothèque pose quand le client envoie un Suppress
    /// Output (bureau masqué ou réduit) et lève au Suppress Output contraire
    /// ou au Refresh Rect. Partagé avec elle par
    /// `with_display_suppressed_handle`.
    suppression: Arc<AtomicBool>,
}

impl Handler {
    fn new(mire: Option<u32>) -> Self {
        Self {
            mire,
            suppression: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl RdpServerInputHandler for Handler {
    fn keyboard(&mut self, event: KeyboardEvent) {
        info!(?event, "keyboard");
        // Lu par la suite bout en bout : l'ordre entre l'annonce du presse-papiers
        // du client et la touche qui colle (rdp-clipboard.spec.js).
        println!("touche du client");
    }

    fn mouse(&mut self, event: MouseEvent) {
        info!(?event, "mouse");
    }
}

const WIDTH: u16 = 1920;
const HEIGHT: u16 = 1080;

struct DisplayUpdates {
    /// Mire plein écran : sa cadence et son horloge, l'image suivante partant
    /// à l'échéance d'une grille fixe (un encodage lent ne décale pas les
    /// suivantes, il en fait sauter).
    mire: Option<(u32, tokio::time::Interval)>,
    rang: u32,
    /// Voir `Handler::suppression`.
    suppression: Arc<AtomicBool>,
    /// L'état déjà annoncé sur la sortie standard, pour n'écrire qu'aux
    /// changements.
    suspendue: bool,
}

/// Le pas d'attente d'une mire suspendue : de quoi reprendre sans délai
/// perceptible, sans tourner à vide.
const PAS_SUSPENDU: Duration = Duration::from_millis(50);

/// Une image plein écran qui change entièrement d'une fois sur l'autre : un
/// dégradé diagonal qui glisse de quatre pixels par image, avec un bandeau
/// clair qui défile. Rien ne se répète d'une image à la suivante, comme une
/// vidéo plein écran ; rien n'est du bruit, comme un vrai bureau.
fn image_de_mire(rang: u32) -> Vec<u8> {
    let (w, h) = (usize::from(WIDTH), usize::from(HEIGHT));
    let decalage = (rang as usize).wrapping_mul(4);
    let bandeau = (rang as usize).wrapping_mul(6) % h;
    let mut data = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let clair = y.abs_diff(bandeau) < 40;
        for x in 0..w {
            let v = (x + y + decalage) & 0xff;
            #[allow(clippy::cast_possible_truncation)]
            let (b, g, r) = (
                v as u8,
                ((x / 8 + decalage) & 0xff) as u8,
                ((y / 4) & 0xff) as u8,
            );
            if clair {
                data.extend_from_slice(&[b | 0xc0, g | 0xc0, r | 0xc0, 255]);
            } else {
                data.extend_from_slice(&[b, g, r, 255]);
            }
        }
    }
    data
}

#[async_trait::async_trait]
impl RdpServerDisplayUpdates for DisplayUpdates {
    async fn next_update(&mut self) -> anyhow::Result<Option<DisplayUpdate>> {
        // La bibliothèque savait recevoir le Suppress Output mais la mire n'en
        // lisait pas le drapeau : un client qui suspendait l'affichage recevait
        // tout de même 30 images pleines par seconde, et le banc ne pouvait
        // voir aucun gain (audit du 27 novembre 2026, PR-1). Le changement
        // d'état s'écrit sur la sortie standard, lisible par la suite bout en
        // bout comme « touche du client ».
        while self.suppression.load(Ordering::Relaxed) {
            if !self.suspendue {
                self.suspendue = true;
                println!("affichage suspendu par le client");
            }
            sleep(PAS_SUSPENDU).await;
        }
        if self.suspendue {
            self.suspendue = false;
            println!("affichage repris par le client");
        }
        if let Some((_, horloge)) = self.mire.as_mut() {
            horloge.tick().await;
            self.rang = self.rang.wrapping_add(1);
            let stride = NonZeroUsize::new(usize::from(WIDTH) * 4).expect("never zero");
            return Ok(Some(DisplayUpdate::Bitmap(BitmapUpdate {
                x: 0,
                y: 0,
                width: NonZeroU16::new(WIDTH).expect("never zero"),
                height: NonZeroU16::new(HEIGHT).expect("never zero"),
                format: PixelFormat::BgrA32,
                data: image_de_mire(self.rang).into(),
                stride,
            })));
        }
        sleep(Duration::from_millis(100)).await;
        let mut rng = rand::rng();

        let y: u16 = rng.random_range(0..HEIGHT);
        let height = rng.random_range(1..=HEIGHT.checked_sub(y).expect("never underflow"));
        let height = NonZeroU16::new(height).expect("never zero");

        let x: u16 = rng.random_range(0..WIDTH);
        let width = rng.random_range(1..=WIDTH.checked_sub(x).expect("never underflow"));
        let width = NonZeroU16::new(width).expect("never zero");

        let capacity = NonZeroUsize::from(width)
            .checked_mul(NonZeroUsize::from(height))
            .expect("never overflow")
            .get()
            .checked_mul(4)
            .expect("never overflow");
        let mut data = Vec::with_capacity(capacity);
        for _ in 0..(data.capacity() / 4) {
            data.push(rng.random());
            data.push(rng.random());
            data.push(rng.random());
            data.push(255);
        }

        info!("get_update +{x}+{y} {width}x{height}");
        let stride = NonZeroUsize::from(width)
            .checked_mul(NonZeroUsize::new(4).expect("never zero"))
            .expect("never overflow");
        let bitmap = BitmapUpdate {
            x,
            y,
            width,
            height,
            format: PixelFormat::BgrA32,
            data: data.into(),
            stride,
        };
        Ok(Some(DisplayUpdate::Bitmap(bitmap)))
    }
}

#[async_trait::async_trait]
impl RdpServerDisplay for Handler {
    async fn size(&mut self) -> DesktopSize {
        DesktopSize {
            width: WIDTH,
            height: HEIGHT,
        }
    }

    async fn updates(&mut self) -> anyhow::Result<Box<dyn RdpServerDisplayUpdates>> {
        let mire = self.mire.map(|hz| {
            let mut horloge = time::interval(Duration::from_secs(1) / hz);
            horloge.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
            (hz, horloge)
        });
        Ok(Box::new(DisplayUpdates {
            mire,
            rang: 0,
            suppression: self.suppression.clone(),
            suspendue: false,
        }))
    }
}

#[derive(Debug)]
pub struct Inner {
    ev_sender: Option<UnboundedSender<ServerEvent>>,
}

/// Texte que le « bureau distant » propose dans son presse-papiers. Le test
/// bout-en-bout vérifie qu'il arrive bien jusqu'au poste local.
pub const CLIP_TEXT: &str = "avash-cliprdr-test";

/// Texte que le client copie pour que le serveur de test lui offre le fichier
/// `--offrir` : c'est le seul moyen, pour un scénario, de déclencher la copie
/// de fichiers « sur le bureau distant ».
pub const DECLENCHEUR_OFFRE: &str = "avash-offre-fichiers";

/// Texte que le client copie pour que le serveur « recopie du texte » après
/// avoir offert des fichiers : une nouvelle FormatList qui remplace la liste
/// offerte. Le scénario vérifie alors que la pastille « N fichiers copiés »
/// disparaît (défaut de l'audit du 7 septembre 2026 : elle restait).
pub const DECLENCHEUR_CHANGE: &str = "avash-change-presse-papiers";

/// Morceau demandé au client quand le serveur reçoit ses fichiers.
const MORCEAU: u32 = 64 * 1024;

/// Réception, côté serveur, des fichiers que le client a offerts.
#[derive(Debug)]
struct Reception {
    files: Vec<FileDescriptor>,
    index: usize,
    dossier: PathBuf,
    fichier: Option<std::fs::File>,
    position: u64,
    flux: u32,
    data_id: Option<u32>,
}

/// Presse-papiers du serveur de test : annonce du texte dès que le canal est
/// prêt, le sert quand le client le réclame ; offre un fichier quand le
/// client copie le texte déclencheur ; et reçoit les fichiers que le client
/// offre, dans le dossier `--recevoir-dans`, en disant sur la sortie standard
/// ce qu'il a reçu.
#[derive(Debug)]
struct ClipBackend {
    sender: Option<UnboundedSender<ServerEvent>>,
    offrir: Option<PathBuf>,
    recevoir_dans: Option<PathBuf>,
    reception: Option<Reception>,
    prochain_flux: u32,
}

ironrdp::core::impl_as_any!(ClipBackend);

impl ClipBackend {
    fn send(&self, msg: ClipboardMessage) {
        if let Some(tx) = &self.sender {
            let _ = tx.send(ServerEvent::Clipboard(msg));
        }
    }

    /// Demande le morceau suivant du fichier en cours, ou passe au suivant.
    fn demander_suivant(&mut self) {
        loop {
            let Some(r) = self.reception.as_mut() else {
                return;
            };
            if r.index >= r.files.len() {
                println!("réception terminée : {} entrées", r.files.len());
                self.reception = None;
                return;
            }
            let d = &r.files[r.index];
            let est_dossier = d
                .attributes
                .is_some_and(|a| a.contains(ClipboardFileAttributes::DIRECTORY));
            let mut cible = r.dossier.clone();
            if let Some(rel) = d.relative_path.as_deref().filter(|p| !p.is_empty()) {
                for c in rel.split('\\') {
                    cible.push(c);
                }
            }
            cible.push(&d.name);
            if est_dossier {
                let _ = std::fs::create_dir_all(&cible);
                println!("reçu : dossier {}", cible.display());
                r.index += 1;
                continue;
            }
            if r.fichier.is_none() {
                if let Some(p) = cible.parent() {
                    let _ = std::fs::create_dir_all(p);
                }
                r.fichier = std::fs::File::create(&cible).ok();
                r.position = 0;
            }
            let taille = d.file_size.unwrap_or(0);
            if r.position >= taille {
                r.fichier = None;
                println!("reçu : {} ({taille} octets)", cible.display());
                r.index += 1;
                continue;
            }
            let longueur =
                u32::try_from((taille - r.position).min(u64::from(MORCEAU))).unwrap_or(MORCEAU);
            r.flux = self.prochain_flux;
            self.prochain_flux += 1;
            let req = FileContentsRequest {
                stream_id: r.flux,
                index: i32::try_from(r.index).unwrap_or(i32::MAX),
                flags: FileContentsFlags::RANGE,
                position: r.position,
                requested_size: longueur,
                data_id: r.data_id,
            };
            self.send(ClipboardMessage::SendFileContentsRequest(req));
            return;
        }
    }
}

impl CliprdrBackend for ClipBackend {
    #[allow(clippy::unnecessary_literal_bound)]
    fn temporary_directory(&self) -> &str {
        "."
    }
    fn client_capabilities(&self) -> ClipboardGeneralCapabilityFlags {
        ClipboardGeneralCapabilityFlags::STREAM_FILECLIP_ENABLED
            | ClipboardGeneralCapabilityFlags::CAN_LOCK_CLIPDATA
            | ClipboardGeneralCapabilityFlags::HUGE_FILE_SUPPORT_ENABLED
    }
    fn on_ready(&mut self) {
        // Le bureau distant « a copié quelque chose » : on l'annonce au client.
        self.send(ClipboardMessage::SendInitiateCopy(vec![
            ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT),
        ]));
    }
    fn on_request_format_list(&mut self) {
        self.send(ClipboardMessage::SendInitiateCopy(vec![
            ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT),
        ]));
    }
    fn on_process_negotiated_capabilities(&mut self, _caps: ClipboardGeneralCapabilityFlags) {}
    fn on_remote_copy(&mut self, formats: &[ClipboardFormat]) {
        println!("annonce du client");
        // Le client a copié : des fichiers (on demande leur liste, puis leur
        // contenu si un dossier de réception est donné), ou du texte (on le lit,
        // pour y reconnaître le déclencheur de l'offre).
        let liste = formats.iter().find(|f| {
            f.name
                .as_ref()
                .is_some_and(|n| n.value() == ClipboardFormatName::FILE_LIST.value())
        });
        if let Some(f) = liste {
            if self.recevoir_dans.is_some() {
                self.send(ClipboardMessage::SendInitiatePaste(f.id));
            }
        } else if formats
            .iter()
            .any(|f| f.id == ClipboardFormatId::CF_UNICODETEXT)
        {
            self.send(ClipboardMessage::SendInitiatePaste(
                ClipboardFormatId::CF_UNICODETEXT,
            ));
        }
    }
    fn on_format_data_request(&mut self, req: FormatDataRequest) {
        // Le client réclame le texte : on le sert.
        let resp = if req.format == ClipboardFormatId::CF_UNICODETEXT {
            FormatDataResponse::new_unicode_string(CLIP_TEXT).into_owned()
        } else {
            FormatDataResponse::new_error().into_owned()
        };
        self.send(ClipboardMessage::SendFormatData(resp));
    }
    fn on_format_data_response(&mut self, resp: FormatDataResponse<'_>) {
        if resp.is_error() {
            return;
        }
        let Ok(texte) = resp.to_unicode_string() else {
            return;
        };
        println!("texte du client : {texte}");
        if texte == DECLENCHEUR_OFFRE {
            if let Some(p) = self.offrir.clone() {
                let nom = p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let taille = std::fs::metadata(&p).map_or(0, |m| m.len());
                println!("offre : {nom} ({taille} octets)");
                self.send(ClipboardMessage::SendInitiateFileCopy(vec![
                    FileDescriptor::new(nom)
                        .with_file_size(taille)
                        .with_attributes(ClipboardFileAttributes::NORMAL),
                ]));
            }
        } else if texte == DECLENCHEUR_CHANGE {
            // Après l'offre de fichiers, le bureau distant « recopie du texte » :
            // une nouvelle annonce = une nouvelle FormatList côté client, qui
            // doit invalider la liste offerte et faire disparaître la pastille.
            self.send(ClipboardMessage::SendInitiateCopy(vec![
                ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT),
            ]));
        }
    }
    fn on_remote_file_list(&mut self, files: &[FileDescriptor], clip_data_id: Option<u32>) {
        let Some(dossier) = self.recevoir_dans.clone() else {
            return;
        };
        println!("liste du client : {} entrées", files.len());
        self.reception = Some(Reception {
            files: files.to_vec(),
            index: 0,
            dossier,
            fichier: None,
            position: 0,
            flux: 0,
            data_id: clip_data_id,
        });
        self.demander_suivant();
    }
    fn on_file_contents_request(&mut self, req: FileContentsRequest) {
        // Le client colle le fichier offert : on sert la taille ou la plage.
        let erreur = FileContentsResponse::new_error(req.stream_id);
        let Some(p) = self.offrir.clone().filter(|_| req.index == 0) else {
            self.send(ClipboardMessage::SendFileContentsResponse(erreur));
            return;
        };
        let resp = if req.flags.contains(FileContentsFlags::SIZE) {
            match std::fs::metadata(&p) {
                Ok(m) => FileContentsResponse::new_size_response(req.stream_id, m.len()),
                Err(_) => erreur,
            }
        } else {
            match std::fs::read(&p) {
                Ok(tout) => {
                    let debut = usize::try_from(req.position)
                        .unwrap_or(usize::MAX)
                        .min(tout.len());
                    let fin = debut
                        .saturating_add(req.requested_size as usize)
                        .min(tout.len());
                    FileContentsResponse::new_data_response(
                        req.stream_id,
                        tout[debut..fin].to_vec(),
                    )
                }
                Err(_) => erreur,
            }
        };
        self.send(ClipboardMessage::SendFileContentsResponse(resp));
    }
    fn on_file_contents_response(&mut self, resp: FileContentsResponse<'_>) {
        let Some(r) = self.reception.as_mut() else {
            return;
        };
        if resp.stream_id() != r.flux {
            return;
        }
        if resp.is_error() {
            println!("refus du client sur l'entrée {}", r.index);
            r.fichier = None;
            r.index += 1;
        } else {
            use std::io::Write as _;
            if let Some(f) = r.fichier.as_mut() {
                let _ = f.write_all(resp.data());
            }
            r.position += resp.data().len() as u64;
            if resp.data().is_empty() {
                r.index += 1;
                r.fichier = None;
            }
        }
        self.demander_suivant();
    }
    fn on_lock(&mut self, _id: LockDataId) {}
    fn on_unlock(&mut self, _id: LockDataId) {}
}

#[derive(Debug, Clone)]
struct ClipFactory {
    sender: Option<UnboundedSender<ServerEvent>>,
    offrir: Option<PathBuf>,
    recevoir_dans: Option<PathBuf>,
}

impl ServerEventSender for ClipFactory {
    fn set_sender(&mut self, sender: UnboundedSender<ServerEvent>) {
        self.sender = Some(sender);
    }
}

impl CliprdrBackendFactory for ClipFactory {
    fn build_cliprdr_backend(&self) -> Box<dyn CliprdrBackend> {
        Box::new(ClipBackend {
            sender: self.sender.clone(),
            offrir: self.offrir.clone(),
            recevoir_dans: self.recevoir_dans.clone(),
            reception: None,
            prochain_flux: 1,
        })
    }
}

impl CliprdrServerFactory for ClipFactory {}

struct StubSoundServerFactory {
    inner: Arc<Mutex<Inner>>,
}

impl ServerEventSender for StubSoundServerFactory {
    fn set_sender(&mut self, sender: UnboundedSender<ServerEvent>) {
        let mut inner = self.inner.lock().expect("poisoned");
        inner.ev_sender = Some(sender);
    }
}

impl SoundServerFactory for StubSoundServerFactory {
    fn build_backend(&self) -> Box<dyn RdpsndServerHandler> {
        Box::new(SndHandler {
            inner: Arc::clone(&self.inner),
            task: None,
        })
    }
}

#[derive(Debug)]
struct SndHandler {
    inner: Arc<Mutex<Inner>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl RdpsndServerHandler for SndHandler {
    fn get_formats(&self) -> &[AudioFormat] {
        &[
            AudioFormat {
                format: WaveFormat::OPUS,
                n_channels: 2,
                n_samples_per_sec: 48000,
                n_avg_bytes_per_sec: 192_000,
                n_block_align: 4,
                bits_per_sample: 16,
                data: None,
            },
            AudioFormat {
                format: WaveFormat::PCM,
                n_channels: 2,
                n_samples_per_sec: 44100,
                n_avg_bytes_per_sec: 176_400,
                n_block_align: 4,
                bits_per_sample: 16,
                data: None,
            },
        ]
    }

    fn choose_format<'a>(
        &mut self,
        common: &'a [NegotiatedFormat],
    ) -> Option<&'a NegotiatedFormat> {
        debug!(?common);

        // The crate hands us the formats common to both peers in our preference
        // order; take the most-preferred one.
        common.first()
    }

    fn start(&mut self, format: &NegotiatedFormat) -> Result<(), Box<dyn RdpsndError>> {
        let fmt = format.format().clone();

        let mut opus_enc = if fmt.format == WaveFormat::OPUS {
            let n_channels: opus2::Channels = match fmt.n_channels {
                1 => opus2::Channels::Mono,
                2 => opus2::Channels::Stereo,
                // Init failure: decline the format instead of leaving the channel
                // negotiated-but-silent (the crate logs the error and skips audio).
                n => {
                    return Err(Box::new(io::Error::other(format!(
                        "invalid OPUS channels: {n}"
                    ))))
                }
            };

            match opus2::Encoder::new(fmt.n_samples_per_sec, n_channels, opus2::Application::Audio)
            {
                Ok(enc) => Some(enc),
                Err(err) => {
                    return Err(Box::new(io::Error::other(format!(
                        "failed to create OPUS encoder: {err}"
                    ))));
                }
            }
        } else {
            None
        };

        let inner = Arc::clone(&self.inner);
        self.task = Some(tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_millis(20));
            let mut ts = 0;
            let mut phase = 0.0f32;
            loop {
                interval.tick().await;
                let wave = generate_sine_wave(fmt.n_samples_per_sec, 440.0, 20, &mut phase);

                let data = if let Some(ref mut enc) = opus_enc {
                    match enc.encode_vec(&wave, wave.len()) {
                        Ok(data) => data,
                        Err(err) => {
                            warn!("Failed to encode with OPUS: {}", err);
                            return;
                        }
                    }
                } else {
                    wave.into_iter().flat_map(i16::to_le_bytes).collect()
                };

                let inner = inner.lock().expect("poisoned");
                if let Some(sender) = inner.ev_sender.as_ref() {
                    let _ = sender.send(ServerEvent::Rdpsnd(RdpsndServerMessage::Wave(data, ts)));
                }
                ts = ts.wrapping_add(100);
            }
        }));

        Ok(())
    }

    fn stop(&mut self) {
        let Some(task) = self.task.take() else {
            return;
        };
        task.abort();
    }
}

fn generate_sine_wave(
    sample_rate: u32,
    frequency: f32,
    duration_ms: u64,
    phase: &mut f32,
) -> Vec<i16> {
    use core::f32::consts::PI;

    let total_samples = (u64::from(sample_rate) * duration_ms) / 1000;

    #[expect(clippy::as_conversions, clippy::cast_precision_loss)]
    let delta_phase = 2.0 * PI * frequency / sample_rate as f32;

    let amplitude = 32767.0; // Max amplitude for 16-bit audio

    let capacity = usize::try_from(total_samples).expect("u64-to-usize") * 2; // 2 channels
    let mut samples = Vec::with_capacity(capacity);

    for _ in 0..total_samples {
        let sample = (*phase).sin();
        *phase += delta_phase;

        // Wrap phase to maintain precision and avoid overflow.
        *phase %= 2.0 * PI;

        #[expect(clippy::as_conversions, clippy::cast_possible_truncation)]
        let sample_i16 = (sample * amplitude) as i16;

        // Write same sample to both channels (stereo)
        samples.push(sample_i16);
        samples.push(sample_i16);
    }

    samples
}

#[allow(clippy::too_many_arguments)]
async fn run(
    bind_addr: SocketAddr,
    hybrid: bool,
    username: String,
    password: String,
    cert: Option<PathBuf>,
    key: Option<PathBuf>,
    offrir: Option<PathBuf>,
    recevoir_dans: Option<PathBuf>,
    mire: Option<u32>,
) -> anyhow::Result<()> {
    info!(%bind_addr, ?cert, ?key, "run");

    let handler = Handler::new(mire);

    let server_builder = RdpServer::builder().with_addr(bind_addr);

    let server_builder = if let Some((cert_path, key_path)) = cert.as_deref().zip(key.as_deref()) {
        let identity = TlsIdentityCtx::init_from_paths(cert_path, key_path)
            .context("failed to init TLS identity")?;
        let acceptor = identity
            .make_acceptor()
            .context("failed to build TLS acceptor")?;

        if hybrid {
            server_builder.with_hybrid(acceptor, identity.pub_key)
        } else {
            server_builder.with_tls(acceptor)
        }
    } else {
        server_builder.with_no_security()
    };

    let sound = Box::new(StubSoundServerFactory {
        inner: Arc::new(Mutex::new(Inner { ev_sender: None })),
    });

    let mut server = server_builder
        .with_input_handler(handler.clone())
        .with_display_handler(handler.clone())
        .with_display_suppressed_handle(handler.suppression.clone())
        .with_cliprdr_factory(Some(Box::new(ClipFactory {
            sender: None,
            offrir,
            recevoir_dans,
        })))
        .with_sound_factory(Some(sound))
        // Le canal rdpdr n'est joint que si le client le demande ; le
        // scénario ne démarre qu'à l'annonce d'un lecteur. Sans lecteur, rien.
        .with_static_channel_factory(Some(Box::new(rdpdr::FabriqueRdpdr)))
        .build();

    server.set_credentials(Some(Credentials {
        username,
        password,
        domain: None,
    }));

    server.run().await
}

#[cfg(test)]
mod tests_suspension {
    use super::{AtomicBool, Duration, Handler, Ordering, RdpServerDisplay as _};
    use std::sync::Arc;

    /// Audit du 27 novembre 2026, PR-1 : la mire poussait ses images pleines
    /// à un client qui avait suspendu l'affichage. Suspendue, elle ne rend
    /// plus rien ; levée, elle reprend aussitôt.
    #[test]
    fn la_mire_suspendue_ne_pousse_plus_d_image_et_reprend_a_la_levee() {
        let rt = ironrdp::server::tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            let mut h = Handler::new(Some(30));
            let drapeau: Arc<AtomicBool> = h.suppression.clone();
            let mut maj = h.updates().await.unwrap();
            assert!(maj.next_update().await.unwrap().is_some(), "la mire émet");
            drapeau.store(true, Ordering::Relaxed);
            let attente = ironrdp::server::tokio::time::timeout(
                Duration::from_millis(400),
                maj.next_update(),
            );
            assert!(
                attente.await.is_err(),
                "aucune image tant que c'est suspendu"
            );
            drapeau.store(false, Ordering::Relaxed);
            let repris =
                ironrdp::server::tokio::time::timeout(Duration::from_secs(2), maj.next_update());
            assert!(
                repris.await.expect("la mire reprend").unwrap().is_some(),
                "une image neuve à la levée"
            );
        });
    }
}
