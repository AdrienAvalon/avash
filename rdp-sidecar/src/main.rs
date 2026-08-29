//! Avash — sidecar client RDP (IronRDP), isole de russh (conflit de deps).
//!
//! Etape 1 : se connecter a un serveur RDP, decoder le bureau et l'enregistrer
//! en PNG. Valide la brique la plus risquee (connexion + CredSSP + graphiques)
//! avant le streaming vers l'interface.
//!
//! Usage : avash-rdp --host H [--port 3389] -u USER -p PASS [-o out.png]
//!                   [--width 1280] [--height 800] [--domain D] [--secs 6]

use anyhow::{Context, Result};
use ironrdp::connector::{self, Credentials};
use ironrdp::pdu::gcc::KeyboardType;
use ironrdp::pdu::rdp::capability_sets::MajorPlatformType;
use ironrdp::pdu::rdp::client_info::{PerformanceFlags, TimezoneInfo};
use ironrdp::session::image::DecodedImage;
use ironrdp::session::{ActiveStageBuilder, ActiveStageOutput};
use ironrdp::graphics::image_processing::PixelFormat;
use std::time::Duration;
use tokio::net::TcpStream;

struct Args {
    host: String,
    port: u16,
    user: String,
    pass: String,
    domain: Option<String>,
    output: String,
    width: u16,
    height: u16,
    secs: u64,
}

fn parse_args() -> Result<Args> {
    let mut a = pico_args();
    Ok(Args {
        host: a.req("--host")?,
        port: a.opt("--port")?.and_then(|s| s.parse().ok()).unwrap_or(3389),
        user: a.req2("-u", "--username")?,
        pass: a.req2("-p", "--password")?,
        domain: a.opt("--domain")?,
        output: a.opt("-o")?.or(a.opt("--output")?).unwrap_or_else(|| "out.png".into()),
        width: a.opt("--width")?.and_then(|s| s.parse().ok()).unwrap_or(1280),
        height: a.opt("--height")?.and_then(|s| s.parse().ok()).unwrap_or(800),
        secs: a.opt("--secs")?.and_then(|s| s.parse().ok()).unwrap_or(6),
    })
}

// Mini parseur d'arguments (evite une dependance).
struct Pa(Vec<String>);
fn pico_args() -> Pa {
    Pa(std::env::args().skip(1).collect())
}
impl Pa {
    fn opt(&mut self, key: &str) -> Result<Option<String>> {
        if let Some(i) = self.0.iter().position(|a| a == key) {
            let v = self.0.get(i + 1).cloned().context("valeur manquante")?;
            return Ok(Some(v));
        }
        Ok(None)
    }
    fn req(&mut self, key: &str) -> Result<String> {
        self.opt(key)?.with_context(|| format!("argument requis : {key}"))
    }
    fn req2(&mut self, k1: &str, k2: &str) -> Result<String> {
        if let Some(v) = self.opt(k1)? {
            return Ok(v);
        }
        self.req(k2)
    }
}

fn build_config(a: &Args) -> connector::Config {
    connector::Config {
        credentials: Credentials::UsernamePassword {
            username: a.user.clone(),
            password: a.pass.clone(),
        },
        domain: a.domain.clone(),
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
        .context("clé publique du certificat non alignée")
        .map(<[u8]>::to_vec)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args()?;
    let config = build_config(&args);

    let tcp = TcpStream::connect((args.host.as_str(), args.port))
        .await
        .with_context(|| format!("connexion TCP à {}:{}", args.host, args.port))?;
    let client_addr = tcp.local_addr()?;

    let mut framed = ironrdp_tokio::TokioFramed::new(tcp);
    let mut connector = connector::ClientConnector::new(config, client_addr);

    let should_upgrade = ironrdp_tokio::connect_begin(&mut framed, &mut connector)
        .await
        .context("début de connexion")?;
    let initial = framed.into_inner_no_leftover();
    let (upgraded_stream, cert) = ironrdp_tls::upgrade(initial, &args.host)
        .await
        .context("passage en TLS")?;
    let pubkey = server_public_key(&cert)?;
    let upgraded = ironrdp_tokio::mark_as_upgraded(should_upgrade, &mut connector);

    let mut framed = ironrdp_tokio::TokioFramed::new(upgraded_stream);
    let mut net = ironrdp_tokio::reqwest::ReqwestNetworkClient::new();
    let result = ironrdp_tokio::connect_finalize(
        upgraded,
        connector,
        &mut framed,
        &mut net,
        args.host.clone().into(),
        pubkey,
        None,
    )
    .await
    .context("finalisation (CredSSP/NLA)")?;

    eprintln!(
        "connecté : bureau {}x{}",
        result.desktop_size.width, result.desktop_size.height
    );

    let mut image = DecodedImage::new(
        PixelFormat::RgbA32,
        result.desktop_size.width,
        result.desktop_size.height,
    );
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

    // Boucle bornée : on laisse le serveur peindre le bureau quelques secondes.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(args.secs);
    'outer: loop {
        let read = tokio::time::timeout_at(deadline, framed.read_pdu());
        let (action, payload) = match read.await {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => return Err(anyhow::Error::new(e).context("lecture PDU")),
            Err(_) => break 'outer, // fenêtre écoulée : on enregistre ce qu'on a
        };
        for out in active.process(&mut image, action, &payload)? {
            match out {
                ActiveStageOutput::ResponseFrame(frame) => {
                    use ironrdp_tokio::FramedWrite as _;
                    framed.write_all(&frame).await.context("écriture réponse")?;
                }
                ActiveStageOutput::Terminate(_) => break 'outer,
                _ => {}
            }
        }
    }

    let buf: image::ImageBuffer<image::Rgba<u8>, _> = image::ImageBuffer::from_raw(
        u32::from(image.width()),
        u32::from(image.height()),
        image.data().to_vec(),
    )
    .context("image invalide")?;
    buf.save(&args.output).with_context(|| format!("écriture {}", args.output))?;
    eprintln!("bureau enregistré : {}", args.output);
    Ok(())
}
