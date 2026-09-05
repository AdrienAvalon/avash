//! Terminateur VeNCrypt devant le serveur RFB de test : ce que le client
//! d'avash rencontre face à un serveur qui chiffre (TigerVNC, x11vnc avec
//! certificat), sans machine de plus.
//!
//! rustvncserver ne parle qu'en clair sur un `TcpStream`. Ce module écoute sur
//! un second port, mène lui-même la poignée de main RFB avec le client
//! (version, type de sécurité 19, VeNCrypt 0.2, sous-type X509Vnc), monte TLS
//! avec le certificat donné, puis ouvre une connexion en clair vers le serveur
//! interne, y refait la poignée de main jusqu'au choix de l'authentification
//! VNC, et relie les deux flux octet pour octet : le défi VNC, le résultat et
//! tout le protocole passent ainsi sous TLS sans que le serveur interne le
//! sache. Chaque étape est écrite sur la sortie standard pour le scénario.

use anyhow::Context as _;
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::rustls;
use tokio_rustls::rustls::pki_types::pem::PemObject as _;

const VERSION_RFB: &[u8; 12] = b"RFB 003.008\n";
const SECURITE_VENCRYPT: u8 = 19;
const SECURITE_VNC: u8 = 2;
const SOUS_TYPE_X509_VNC: u32 = 261;

/// Charge le certificat (chaîne PEM) et la clé privée (PEM), et prépare TLS.
fn accepteur(cert: &Path, cle: &Path) -> anyhow::Result<tokio_rustls::TlsAcceptor> {
    let certs: Vec<rustls::pki_types::CertificateDer<'static>> =
        rustls::pki_types::CertificateDer::pem_file_iter(cert)
            .with_context(|| format!("lecture de {}", cert.display()))?
            .collect::<Result<_, _>>()
            .context("certificat PEM illisible")?;
    let cle = rustls::pki_types::PrivateKeyDer::from_pem_file(cle)
        .with_context(|| format!("lecture de {}", cle.display()))?;
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, cle)
        .context("certificat ou clé refusé par rustls")?;
    Ok(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
}

/// Écoute sur `port_tls` et relie chaque client, après VeNCrypt, au serveur
/// interne en clair sur `port_interne`.
pub async fn ecouter(
    port_tls: u16,
    port_interne: u16,
    cert: &Path,
    cle: &Path,
) -> anyhow::Result<()> {
    let acc = accepteur(cert, cle)?;
    let ecoute = TcpListener::bind(("0.0.0.0", port_tls))
        .await
        .with_context(|| format!("écoute VeNCrypt sur le port {port_tls}"))?;
    println!("vencrypt : port {port_tls} vers le serveur interne {port_interne}");
    loop {
        let (client, adresse) = ecoute.accept().await.context("accept VeNCrypt")?;
        let acc = acc.clone();
        tokio::spawn(async move {
            match relier(client, acc, port_interne).await {
                Ok(()) => println!("vencrypt : client {adresse} parti"),
                Err(e) => println!("vencrypt : client {adresse} : {e:#}"),
            }
        });
    }
}

async fn relier(
    mut client: TcpStream,
    acc: tokio_rustls::TlsAcceptor,
    port_interne: u16,
) -> anyhow::Result<()> {
    client.set_nodelay(true).ok();
    // Poignée de main RFB avec le client, jusqu'à TLS.
    client.write_all(VERSION_RFB).await?;
    let mut version = [0u8; 12];
    client
        .read_exact(&mut version)
        .await
        .context("version du client")?;
    client.write_all(&[1, SECURITE_VENCRYPT]).await?;
    let choix = client.read_u8().await.context("type de sécurité choisi")?;
    anyhow::ensure!(
        choix == SECURITE_VENCRYPT,
        "le client a choisi le type {choix}, pas VeNCrypt"
    );
    println!("vencrypt : type 19 choisi");
    client.write_all(&[0, 2]).await?;
    let mut v = [0u8; 2];
    client
        .read_exact(&mut v)
        .await
        .context("version VeNCrypt du client")?;
    anyhow::ensure!(v == [0, 2], "version VeNCrypt {}.{} du client", v[0], v[1]);
    client.write_all(&[0]).await?;
    client.write_all(&[1]).await?;
    client.write_all(&SOUS_TYPE_X509_VNC.to_be_bytes()).await?;
    let sous_type = client.read_u32().await.context("sous-type choisi")?;
    anyhow::ensure!(
        sous_type == SOUS_TYPE_X509_VNC,
        "sous-type {sous_type} inattendu"
    );
    client.write_all(&[1]).await?;
    println!("vencrypt : sous-type X509Vnc, passage TLS");
    let mut tls = acc.accept(client).await.context("poignée de main TLS")?;
    println!("vencrypt : TLS établi");

    // Poignée de main avec le serveur interne, jusqu'au choix de l'auth VNC.
    let mut interne = TcpStream::connect(("127.0.0.1", port_interne))
        .await
        .context("serveur interne")?;
    interne.set_nodelay(true).ok();
    let mut v_interne = [0u8; 12];
    interne.read_exact(&mut v_interne).await?;
    interne.write_all(VERSION_RFB).await?;
    let n = interne.read_u8().await?;
    anyhow::ensure!(n > 0, "le serveur interne n'annonce aucun type de sécurité");
    let mut types = vec![0u8; usize::from(n)];
    interne.read_exact(&mut types).await?;
    anyhow::ensure!(
        types.contains(&SECURITE_VNC),
        "le serveur interne n'offre pas l'auth VNC"
    );
    interne.write_all(&[SECURITE_VNC]).await?;

    // Dès lors, tout passe tel quel : défi, résultat, ClientInit, et la session.
    tokio::io::copy_bidirectional(&mut tls, &mut interne)
        .await
        .context("relais")?;
    Ok(())
}
