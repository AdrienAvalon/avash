//! Avash — sidecar client RDP (IronRDP), isolé de russh.
//!
//! Sert le bureau distant à Avash via un **WebSocket local binaire** (vrai
//! ArrayBuffer côté webview : pas de base64, pas de JSON — débit maximal, même
//! en 3440×1440). Écoute sur 127.0.0.1:<port aléatoire> et n'accepte qu'un
//! client présentant le bon jeton. Imprime « PORT TOKEN » sur stdout au départ.
//!
//! Messages WebSocket (binaires, auto-délimités) :
//!   sidecar -> app : [1]=CONNECTED w:u16 h:u16 · [2]=FRAME x,y,w,h:u16 + RGBA · [3]=ERROR utf8
//!   app -> sidecar : [1]MOUSE_MOVE x,y · [2]BUTTON b,down,x,y · [3]WHEEL delta:i16 · [4]KEY sc:u16,down
//!
//! Usage : avash-rdp --host H [--port 3389] -u USER -p PASS [--width W --height H] [--domain D] [--shot out.png]

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use ironrdp::connector::{self, Credentials};
use ironrdp::graphics::image_processing::PixelFormat;
use ironrdp::input::{Database, MousePosition, Operation, WheelRotations};
use ironrdp::pdu::gcc::KeyboardType;
use ironrdp::pdu::rdp::capability_sets::MajorPlatformType;
use ironrdp::pdu::rdp::client_info::{PerformanceFlags, TimezoneInfo};
use ironrdp::session::image::DecodedImage;
use ironrdp::session::{ActiveStage, ActiveStageBuilder, ActiveStageOutput};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

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
        self.0.iter().position(|a| a == k).and_then(|i| self.0.get(i + 1).cloned())
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
    cert.tbs_certificate.subject_public_key_info.subject_public_key.as_bytes()
        .context("clé publique non alignée").map(<[u8]>::to_vec)
}

fn mouse_button(n: u8) -> ironrdp::input::MouseButton {
    use ironrdp::input::MouseButton::{Left, Middle, Right, X1, X2};
    match n { 1 => Middle, 2 => Right, 3 => X1, 4 => X2, _ => Left }
}

/// Décode un message d'entrée binaire en opérations IronRDP.
fn input_ops(b: &[u8]) -> Vec<Operation> {
    let u16le = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    match b.first().copied() {
        Some(1) if b.len() >= 5 => vec![Operation::MouseMove(MousePosition { x: u16le(1), y: u16le(3) })],
        Some(2) if b.len() >= 7 => {
            let bt = mouse_button(b[1]);
            let click = if b[2] != 0 { Operation::MouseButtonPressed(bt) } else { Operation::MouseButtonReleased(bt) };
            vec![Operation::MouseMove(MousePosition { x: u16le(3), y: u16le(5) }), click]
        }
        Some(3) if b.len() >= 3 => {
            let d = i16::from_le_bytes([b[1], b[2]]);
            vec![Operation::WheelRotations(WheelRotations { is_vertical: true, rotation_units: d })]
        }
        Some(4) if b.len() >= 4 => {
            let sc = ironrdp::input::Scancode::from(u16le(1));
            vec![if b[3] != 0 { Operation::KeyPressed(sc) } else { Operation::KeyReleased(sc) }]
        }
        _ => Vec::new(),
    }
}

/// Rectangle mis à jour -> message FRAME binaire [2][x][y][w][h][RGBA].
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
) -> Result<(connector::ConnectionResult, ironrdp_tokio::TokioFramed<ironrdp_tls::TlsStream<TcpStream>>)> {
    let tcp = TcpStream::connect((a.host.as_str(), a.port)).await
        .with_context(|| format!("connexion TCP à {}:{}", a.host, a.port))?;
    let client_addr = tcp.local_addr()?;
    let mut framed = ironrdp_tokio::TokioFramed::new(tcp);
    let mut connector = connector::ClientConnector::new(build_config(a), client_addr);
    let should_upgrade = ironrdp_tokio::connect_begin(&mut framed, &mut connector).await.context("début de connexion")?;
    let initial = framed.into_inner_no_leftover();
    let (upgraded_stream, cert) = ironrdp_tls::upgrade(initial, &a.host).await.context("passage TLS")?;
    let pubkey = server_public_key(&cert)?;
    let upgraded = ironrdp_tokio::mark_as_upgraded(should_upgrade, &mut connector);
    let mut framed = ironrdp_tokio::TokioFramed::new(upgraded_stream);
    let mut net = ironrdp_tokio::reqwest::ReqwestNetworkClient::new();
    let result = ironrdp_tokio::connect_finalize(
        upgraded, connector, &mut framed, &mut net, a.host.clone().into(), pubkey, None,
    ).await.context("finalisation (CredSSP/NLA)")?;
    Ok((result, framed))
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args()?;
    let (result, mut framed) = connect(&args).await?;
    let (w, h) = (result.desktop_size.width, result.desktop_size.height);
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
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.context("écoute WebSocket")?;
    let port = listener.local_addr()?.port();
    let token = format!("{:016x}", rand::random::<u64>());
    // Annonce le point de connexion à Avash.
    let mut out = tokio::io::stdout();
    out.write_all(format!("{port} {token}\n").as_bytes()).await?;
    out.flush().await?;

    let (tcp, _) = listener.accept().await.context("acceptation WebSocket")?;
    let ws = tokio_tungstenite::accept_async(tcp).await.context("handshake WebSocket")?;
    let (mut sink, mut stream) = ws.split();

    // Premier message du client = le jeton (sinon on refuse).
    match stream.next().await {
        Some(Ok(Message::Binary(t))) if t == token.as_bytes() => {}
        _ => return Ok(()),
    }

    // CONNECTED [1][w][h]
    let mut hello = vec![1u8];
    hello.extend_from_slice(&w.to_le_bytes());
    hello.extend_from_slice(&h.to_le_bytes());
    sink.send(Message::Binary(hello)).await?;

    let mut db = Database::new();
    use ironrdp_tokio::FramedWrite as _;
    loop {
        tokio::select! {
            biased;
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Binary(b))) => {
                        let events = db.apply(input_ops(&b));
                        for o in active.process_fastpath_input(&mut image, &events)? {
                            if let ActiveStageOutput::ResponseFrame(f) = o {
                                framed.write_all(&f).await.context("écriture entrée")?;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break, // onglet fermé
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }
            read = framed.read_pdu() => {
                let (action, payload) = read.context("lecture PDU")?;
                for o in active.process(&mut image, action, &payload)? {
                    match o {
                        ActiveStageOutput::ResponseFrame(f) => framed.write_all(&f).await.context("écriture réponse")?,
                        ActiveStageOutput::GraphicsUpdate(rect) => {
                            sink.send(Message::Binary(frame_msg(&image, &rect))).await.context("envoi frame")?;
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
        if done { break; }
    }
    let buf: image::ImageBuffer<image::Rgba<u8>, _> =
        image::ImageBuffer::from_raw(u32::from(image.width()), u32::from(image.height()), image.data().to_vec())
            .context("image invalide")?;
    buf.save(path)?;
    eprintln!("capture : {path}");
    Ok(())
}
