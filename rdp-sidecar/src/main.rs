//! Avash — sidecar client RDP (IronRDP), isolé de russh.
//!
//! Sert le bureau distant à Avash via un **WebSocket local binaire** (vrai
//! ArrayBuffer côté webview : pas de base64, pas de JSON — débit maximal, même
//! en 3440×1440). Écoute sur 127.0.0.1:<port aléatoire> et n'accepte qu'un
//! client présentant le bon jeton. Imprime « PORT TOKEN » sur stdout au départ.
//!
//! Messages WebSocket (binaires, auto-délimités) :
//!   sidecar -> app : [1]=CONNECTED w:u16 h:u16 · [2]=FRAME x,y,w,h:u16 + RGBA · [3]=ERROR utf8
//!   app -> sidecar : [1]MOUSE_MOVE x,y · [2]BUTTON b,down,x,y · [3]WHEEL delta:i16 · [4]KEY sc:u16,down · [5]RESIZE w:u16,h:u16
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
use ironrdp::connector::connection_activation::ConnectionActivationState;
use ironrdp::connector::{self, Credentials};
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
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

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

fn parse_args() -> Result<Args> {
    let a = Pa(std::env::args().skip(1).collect());
    Ok(Args {
        host: a.opt("--host").context("argument requis : --host")?,
        port: a.opt("--port").and_then(|s| s.parse().ok()).unwrap_or(3389),
        user: a.req2("-u", "--username")?,
        pass: a.req2("-p", "--password")?,
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
        );
    let should_upgrade = ironrdp_tokio::connect_begin(&mut framed, &mut connector)
        .await
        .context("début de connexion")?;
    let initial = framed.into_inner_no_leftover();
    let (upgraded_stream, cert) = ironrdp_tls::upgrade(initial, &a.host)
        .await
        .context("passage TLS")?;
    let pubkey = server_public_key(&cert)?;
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
    let (result, mut framed) = connect(&args).await?;
    let (w, h) = (result.desktop_size.width, result.desktop_size.height);
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

    let (tcp, _) = listener.accept().await.context("acceptation WebSocket")?;
    tcp.set_nodelay(true).ok();
    let ws = tokio_tungstenite::accept_async(tcp)
        .await
        .context("handshake WebSocket")?;
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

    // --- Cadencement adaptatif sur ACK (anti-lag) ---------------------------
    // Au plus une trame « en vol ». Les mises à jour qui arrivent pendant le
    // rendu de la webview sont fusionnées (union des rectangles) ; à l'ACK, on
    // envoie l'état le plus récent. Rapide → chaque trame part aussitôt (aucune
    // latence ajoutée) ; lent → on fusionne, jamais de file qui s'accumule.
    let mut dirty: Option<InclusiveRectangle> = None;
    let mut awaiting_ack = false;
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
            if !awaiting_ack {
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
                    Some(Ok(Message::Binary(b))) => {
                        let events = db.apply(input_ops(&b));
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
                            image = DecodedImage::new(PixelFormat::RgbA32, size.width, size.height);
                            // Annonce la nouvelle taille à Avash (réutilise CONNECTED [1][w][h]).
                            let mut msg = vec![1u8];
                            msg.extend_from_slice(&size.width.to_le_bytes());
                            msg.extend_from_slice(&size.height.to_le_bytes());
                            sink.send(Message::Binary(msg)).await.context("annonce resize")?;
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
