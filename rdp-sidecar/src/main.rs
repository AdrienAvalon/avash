//! Avash — sidecar client RDP (IronRDP), isole de russh.
//!
//! Streame le bureau distant vers Avash et recoit les entrees, via stdio :
//!   stdout (sidecar -> app) : [u8 kind][u32 le len][payload]
//!     1 CONNECTED  = w:u16, h:u16
//!     2 FRAME      = x:u16, y:u16, w:u16, h:u16, puis w*h*4 octets RGBA
//!     3 ERROR      = message UTF-8
//!   stdin (app -> sidecar) : messages a taille fixe
//!     1 MOUSE_MOVE   = x:u16, y:u16
//!     2 MOUSE_BUTTON = button:u8, down:u8, x:u16, y:u16
//!     3 WHEEL        = delta:i16, x:u16, y:u16
//!     4 KEY          = scancode:u16, down:u8
//!
//! Usage : avash-rdp --host H [--port 3389] -u USER -p PASS
//!                   [--width W] [--height H] [--domain D]  (ou --shot out.png)

use anyhow::{Context, Result};
use ironrdp::connector::{self, Credentials};
use ironrdp::input::{Database, MousePosition, Operation, WheelRotations};
use ironrdp::pdu::gcc::KeyboardType;
use ironrdp::pdu::rdp::capability_sets::MajorPlatformType;
use ironrdp::pdu::rdp::client_info::{PerformanceFlags, TimezoneInfo};
use ironrdp::session::image::DecodedImage;
use ironrdp::session::{ActiveStage, ActiveStageBuilder, ActiveStageOutput};
use ironrdp::graphics::image_processing::PixelFormat;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::mpsc;

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
    fn opt(&self, key: &str) -> Option<String> {
        self.0.iter().position(|a| a == key).and_then(|i| self.0.get(i + 1).cloned())
    }
    fn req2(&self, k1: &str, k2: &str) -> Result<String> {
        self.opt(k1).or_else(|| self.opt(k2)).with_context(|| format!("argument requis : {k1}/{k2}"))
    }
}

fn parse_args() -> Result<Args> {
    let a = Pa(std::env::args().skip(1).collect());
    Ok(Args {
        host: a.opt("--host").context("argument requis : --host")?,
        port: a.opt("--port").and_then(|s| s.parse().ok()).unwrap_or(3389),
        user: a.req2("-u", "--username")?,
        pass: a.req2("-p", "--password")?,
        domain: a.opt("--domain"),
        width: a.opt("--width").and_then(|s| s.parse().ok()).unwrap_or(1280),
        height: a.opt("--height").and_then(|s| s.parse().ok()).unwrap_or(800),
        shot: a.opt("--shot"),
    })
}

fn build_config(a: &Args) -> connector::Config {
    connector::Config {
        credentials: Credentials::UsernamePassword { username: a.user.clone(), password: a.pass.clone() },
        domain: a.domain.clone(),
        enable_tls: true,
        enable_credssp: true,
        keyboard_type: KeyboardType::IbmEnhanced,
        keyboard_subtype: 0,
        keyboard_layout: 0,
        keyboard_functional_keys_count: 12,
        ime_file_name: String::new(),
        dig_product_id: String::new(),
        desktop_size: connector::DesktopSize { width: a.width, height: a.height },
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

/// Entrée reçue du front, déjà décodée.
enum Input {
    Move(u16, u16),
    Button { button: u8, down: bool, x: u16, y: u16 },
    Wheel { delta: i16 },
    Key { scancode: u16, down: bool },
}

/// Lit stdin (bloquant) sur un thread dédié et pousse des `Input` décodés.
fn spawn_stdin_reader(tx: mpsc::UnboundedSender<Input>) {
    std::thread::spawn(move || {
        use std::io::Read as _;
        let mut stdin = std::io::stdin().lock();
        let rd = |s: &mut dyn std::io::Read, n: usize| -> Option<Vec<u8>> {
            let mut b = vec![0u8; n];
            s.read_exact(&mut b).ok().map(|()| b)
        };
        loop {
            let mut kind = [0u8; 1];
            if stdin.read_exact(&mut kind).is_err() {
                break;
            }
            let msg = match kind[0] {
                1 => rd(&mut stdin, 4).map(|b| Input::Move(u16::from_le_bytes([b[0], b[1]]), u16::from_le_bytes([b[2], b[3]]))),
                2 => rd(&mut stdin, 6).map(|b| Input::Button {
                    button: b[0],
                    down: b[1] != 0,
                    x: u16::from_le_bytes([b[2], b[3]]),
                    y: u16::from_le_bytes([b[4], b[5]]),
                }),
                3 => rd(&mut stdin, 6).map(|b| Input::Wheel { delta: i16::from_le_bytes([b[0], b[1]]) }),
                4 => rd(&mut stdin, 3).map(|b| Input::Key { scancode: u16::from_le_bytes([b[0], b[1]]), down: b[2] != 0 }),
                _ => None,
            };
            match msg {
                Some(m) => {
                    if tx.send(m).is_err() {
                        break;
                    }
                }
                None => break,
            }
        }
    });
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

fn input_to_ops(i: Input) -> Vec<Operation> {
    match i {
        Input::Move(x, y) => vec![Operation::MouseMove(MousePosition { x, y })],
        Input::Button { button, down, x, y } => {
            let b = mouse_button(button);
            let click = if down { Operation::MouseButtonPressed(b) } else { Operation::MouseButtonReleased(b) };
            vec![Operation::MouseMove(MousePosition { x, y }), click]
        }
        Input::Wheel { delta } => vec![Operation::WheelRotations(WheelRotations { is_vertical: true, rotation_units: delta })],
        Input::Key { scancode, down } => {
            let sc = ironrdp::input::Scancode::from(scancode);
            vec![if down { Operation::KeyPressed(sc) } else { Operation::KeyReleased(sc) }]
        }
    }
}

/// Écrit un message encadré sur stdout.
async fn emit(out: &mut (impl AsyncWriteExt + Unpin), kind: u8, payload: &[u8]) -> Result<()> {
    let len = u32::try_from(payload.len()).unwrap_or(0);
    out.write_all(&[kind]).await?;
    out.write_all(&len.to_le_bytes()).await?;
    out.write_all(payload).await?;
    out.flush().await?;
    Ok(())
}

/// Extrait le rectangle (RGBA) de l'image complète.
fn crop(image: &DecodedImage, r: &ironrdp::pdu::geometry::InclusiveRectangle) -> (u16, u16, u16, u16, Vec<u8>) {
    let iw = usize::from(image.width());
    let data = image.data();
    let x = r.left;
    let y = r.top;
    let w = r.right - r.left + 1;
    let h = r.bottom - r.top + 1;
    let mut buf = Vec::with_capacity(usize::from(w) * usize::from(h) * 4);
    for row in 0..usize::from(h) {
        let sy = usize::from(y) + row;
        let start = (sy * iw + usize::from(x)) * 4;
        buf.extend_from_slice(&data[start..start + usize::from(w) * 4]);
    }
    (x, y, w, h, buf)
}

async fn connect(
    args: &Args,
) -> Result<(connector::ConnectionResult, ironrdp_tokio::TokioFramed<ironrdp_tls::TlsStream<TcpStream>>)> {
    let tcp = TcpStream::connect((args.host.as_str(), args.port))
        .await
        .with_context(|| format!("connexion TCP à {}:{}", args.host, args.port))?;
    let client_addr = tcp.local_addr()?;
    let mut framed = ironrdp_tokio::TokioFramed::new(tcp);
    let mut connector = connector::ClientConnector::new(build_config(args), client_addr);
    let should_upgrade = ironrdp_tokio::connect_begin(&mut framed, &mut connector).await.context("début de connexion")?;
    let initial = framed.into_inner_no_leftover();
    let (upgraded_stream, cert) = ironrdp_tls::upgrade(initial, &args.host).await.context("passage TLS")?;
    let pubkey = server_public_key(&cert)?;
    let upgraded = ironrdp_tokio::mark_as_upgraded(should_upgrade, &mut connector);
    let mut framed = ironrdp_tokio::TokioFramed::new(upgraded_stream);
    let mut net = ironrdp_tokio::reqwest::ReqwestNetworkClient::new();
    let result = ironrdp_tokio::connect_finalize(
        upgraded, connector, &mut framed, &mut net, args.host.clone().into(), pubkey, None,
    )
    .await
    .context("finalisation (CredSSP/NLA)")?;
    Ok((result, framed))
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args()?;
    let (result, mut framed) = connect(&args).await?;
    eprintln!("connecté : {}x{}", result.desktop_size.width, result.desktop_size.height);

    let (w, h) = (result.desktop_size.width, result.desktop_size.height);
    let mut image = DecodedImage::new(PixelFormat::RgbA32, w, h);
    let mut active: ActiveStage = ActiveStageBuilder {
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

    // Mode capture (--shot) : on tourne quelques secondes puis on enregistre.
    if let Some(path) = args.shot.clone() {
        return run_shot(&mut active, &mut image, &mut framed, &path).await;
    }

    let mut out = tokio::io::stdout();
    emit(&mut out, 1, &[w.to_le_bytes(), h.to_le_bytes()].concat()).await?;

    let (tx, mut rx) = mpsc::unbounded_channel::<Input>();
    spawn_stdin_reader(tx);
    let mut db = Database::new();

    use ironrdp_tokio::FramedWrite as _;
    loop {
        tokio::select! {
            biased;
            maybe = rx.recv() => {
                let Some(input) = maybe else { break }; // stdin fermé => fin
                let events = db.apply(input_to_ops(input));
                for o in active.process_fastpath_input(&mut image, &events)? {
                    if let ActiveStageOutput::ResponseFrame(f) = o {
                        framed.write_all(&f).await.context("écriture entrée")?;
                    }
                }
            }
            read = framed.read_pdu() => {
                let (action, payload) = read.context("lecture PDU")?;
                for o in active.process(&mut image, action, &payload)? {
                    match o {
                        ActiveStageOutput::ResponseFrame(f) => framed.write_all(&f).await.context("écriture réponse")?,
                        ActiveStageOutput::GraphicsUpdate(rect) => {
                            let (x, y, cw, ch, rgba) = crop(&image, &rect);
                            let mut payload = Vec::with_capacity(8 + rgba.len());
                            payload.extend_from_slice(&x.to_le_bytes());
                            payload.extend_from_slice(&y.to_le_bytes());
                            payload.extend_from_slice(&cw.to_le_bytes());
                            payload.extend_from_slice(&ch.to_le_bytes());
                            payload.extend_from_slice(&rgba);
                            emit(&mut out, 2, &payload).await?;
                        }
                        ActiveStageOutput::Terminate(_) => return Ok(()),
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
    use ironrdp_tokio::FramedWrite as _;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while let Ok(Ok((action, payload))) = tokio::time::timeout_at(deadline, framed.read_pdu()).await {
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
    let buf: image::ImageBuffer<image::Rgba<u8>, _> =
        image::ImageBuffer::from_raw(u32::from(image.width()), u32::from(image.height()), image.data().to_vec())
            .context("image invalide")?;
    buf.save(path)?;
    eprintln!("capture : {path}");
    Ok(())
}
