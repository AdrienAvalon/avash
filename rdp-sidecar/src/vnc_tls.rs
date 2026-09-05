//! VeNCrypt : le VNC sous TLS, avec la même confiance au premier contact que
//! le RDP.
//!
//! Le client VNC porté sait négocier VeNCrypt (sous-types X.509) mais ne monte
//! pas TLS lui-même : il rend le flux à un « monteur » que ce module fournit.
//! Le flux est un `MaybeTls`, en clair jusqu'à l'accord, chiffré ensuite, du
//! même type avant et après pour que le client n'en sache rien. Le certificat
//! n'est pas vérifié par une autorité (rustls acceptant tout, comme pour le
//! RDP) mais épinglé : sa clé publique est mémorisée au premier contact sous
//! `vnc:<hôte>:<port>` dans le fichier des empreintes, et un changement refuse
//! la connexion avant que le mot de passe ne parte.

use crate::empreintes::{
    empreinte, empreinte_memorisee, juger_certificat, memoriser_empreinte, server_public_key,
    VerdictCert,
};
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::rustls;

/// Le flux du client VNC : TCP en clair, ou TLS par-dessus une fois VeNCrypt
/// négocié. Un seul type, pour que le client porté garde le sien.
pub(crate) enum MaybeTls {
    Clair(TcpStream),
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
}

impl AsyncRead for MaybeTls {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            MaybeTls::Clair(s) => Pin::new(s).poll_read(cx, buf),
            MaybeTls::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for MaybeTls {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            MaybeTls::Clair(s) => Pin::new(s).poll_write(cx, buf),
            MaybeTls::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            MaybeTls::Clair(s) => Pin::new(s).poll_flush(cx),
            MaybeTls::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            MaybeTls::Clair(s) => Pin::new(s).poll_shutdown(cx),
            MaybeTls::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

/// Le monteur que le client VNC appelle : TLS, puis le certificat est jugé.
pub(crate) fn monteur(hote: &str, port: u16) -> vnc::TlsUpgrader<MaybeTls> {
    let hote = hote.to_owned();
    Box::new(move |flux: MaybeTls| {
        Box::pin(async move {
            let MaybeTls::Clair(tcp) = flux else {
                return Err(vnc::VncError::General(
                    "VeNCrypt demandé sur un flux déjà chiffré".to_owned(),
                ));
            };
            monter(tcp, &hote, port)
                .await
                .map(|s| MaybeTls::Tls(Box::new(s)))
                .map_err(|e| vnc::VncError::General(format!("{e:#}")))
        })
    })
}

/// Monte TLS sur `tcp`, puis applique le TOFU sur la clé publique du serveur.
async fn monter(
    tcp: TcpStream,
    hote: &str,
    port: u16,
) -> anyhow::Result<tokio_rustls::client::TlsStream<TcpStream>> {
    use anyhow::Context as _;
    let mut config = rustls::client::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(std::sync::Arc::new(AccepteTout))
        .with_no_client_auth();
    config.resumption = rustls::client::Resumption::disabled();
    // Le nom du serveur ne sert qu'au SNI : une adresse IP passe aussi.
    let nom = rustls::pki_types::ServerName::try_from(hote.to_owned())
        .or_else(|_| rustls::pki_types::ServerName::try_from("vnc.invalid".to_owned()))
        .context("nom de serveur TLS")?;
    let flux = tokio_rustls::TlsConnector::from(std::sync::Arc::new(config))
        .connect(nom, tcp)
        .await
        .context("passage TLS (VeNCrypt)")?;
    let der = flux
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|c| c.first())
        .context("le serveur VeNCrypt n'a présenté aucun certificat")?
        .to_vec();
    let cert = {
        use x509_cert::der::Decode as _;
        x509_cert::Certificate::from_der(&der).context("certificat du serveur VNC illisible")?
    };
    let presentee = empreinte(&server_public_key(&cert)?);
    let cle = format!("vnc:{hote}:{port}");
    match juger_certificat(empreinte_memorisee(&cle).as_deref(), &presentee) {
        VerdictCert::Connu => {}
        VerdictCert::PremierContact => memoriser_empreinte(&cle, &presentee)
            .context("mémorisation de l'empreinte du serveur VNC")?,
        VerdictCert::Change { attendue } => anyhow::bail!(
            "Le certificat de {cle} a changé.\n\nSoit le serveur a été réinstallé, \
             soit quelqu'un intercepte la connexion.\n\nEmpreinte présentée : {presentee}\n\
             Empreinte attendue  : {attendue}\n\nSi le changement est légitime, retirez \
             la ligne « {cle} » de rdp_known_hosts."
        ),
    }
    Ok(flux)
}

/// rustls ne juge pas le certificat : c'est l'épinglage ci-dessus qui décide,
/// avant que quoi que ce soit d'autre ne parte.
#[derive(Debug)]
struct AccepteTout;

impl rustls::client::danger::ServerCertVerifier for AccepteTout {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::aws_lc_rs::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::io::AsyncReadExt as _;
    use tokio::net::TcpListener;

    /// `AVASH_HOME` posé sur un répertoire jetable le temps du test, sous le
    /// verrou que partagent tous les tests qui touchent à cette variable ;
    /// remis en place à la sortie, même sur panique. Sans lui, le test
    /// écrirait dans le fichier de confiance RÉEL du poste (vu une fois, à la
    /// main, avec un `vnc:127.0.0.1:35911` semé dans `~/.config/avash`).
    struct Bac {
        chemin: std::path::PathBuf,
        precedent: Option<std::ffi::OsString>,
        _verrou: std::sync::MutexGuard<'static, ()>,
    }

    impl Bac {
        fn poser() -> Self {
            let verrou = crate::empreintes::VERROU_AVASH_HOME
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let chemin = std::env::temp_dir().join(format!("avash-vnc-tls-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&chemin);
            let precedent = std::env::var_os("AVASH_HOME");
            unsafe { std::env::set_var("AVASH_HOME", &chemin) };
            Self {
                chemin,
                precedent,
                _verrou: verrou,
            }
        }

        fn fichier_de_confiance(&self) -> std::path::PathBuf {
            self.chemin
                .join(".config")
                .join("avash")
                .join("rdp_known_hosts")
        }
    }

    impl Drop for Bac {
        fn drop(&mut self) {
            unsafe {
                match self.precedent.take() {
                    Some(v) => std::env::set_var("AVASH_HOME", v),
                    None => std::env::remove_var("AVASH_HOME"),
                }
            }
            let _ = std::fs::remove_dir_all(&self.chemin);
        }
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    /// Un serveur TLS jetable : un certificat auto-signé neuf par appel, donc
    /// deux appels donnent deux empreintes.
    fn serveur_tls() -> Arc<rustls::ServerConfig> {
        let cle = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        let cert = cle.cert.der().clone();
        let prive = rustls::pki_types::PrivateKeyDer::Pkcs8(cle.signing_key.serialize_der().into());
        Arc::new(
            rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(vec![cert], prive)
                .unwrap(),
        )
    }

    /// Accepte la prochaine connexion, monte TLS côté serveur, puis tient le
    /// flux jusqu'à ce que le client raccroche. Une poignée de main qui
    /// échoue n'est pas une panique : c'est au client de le dire.
    fn accueillir(
        ecoute: &Arc<TcpListener>,
        config: Arc<rustls::ServerConfig>,
    ) -> tokio::task::JoinHandle<()> {
        let ecoute = Arc::clone(ecoute);
        tokio::spawn(async move {
            let (tcp, _) = ecoute.accept().await.unwrap();
            if let Ok(mut flux) = tokio_rustls::TlsAcceptor::from(config).accept(tcp).await {
                let _ = flux.read(&mut [0u8; 1]).await;
            }
        })
    }

    /// Le cas complet, tel que la suite bout en bout le joue contre le serveur
    /// de test : premier contact mémorisé, même certificat reconnu sans rien
    /// réécrire, certificat changé refusé avec les deux empreintes, et
    /// l'empreinte d'origine reste celle du fichier.
    #[test]
    fn le_montage_epingle_le_certificat_et_refuse_qu_il_change() {
        let bac = Bac::poser();
        runtime().block_on(async {
            let ecoute = Arc::new(TcpListener::bind("127.0.0.1:0").await.unwrap());
            let port = ecoute.local_addr().unwrap().port();
            let origine = serveur_tls();
            let remplacant = serveur_tls();

            let serveur = accueillir(&ecoute, Arc::clone(&origine));
            let tcp = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            monter(tcp, "127.0.0.1", port)
                .await
                .expect("le premier contact est accepté");
            serveur.await.unwrap();
            let contenu = std::fs::read_to_string(bac.fichier_de_confiance()).unwrap();
            assert!(
                contenu.starts_with(&format!("vnc:127.0.0.1:{port} ")),
                "l'empreinte est mémorisée sous la clé VNC : {contenu:?}"
            );

            let serveur = accueillir(&ecoute, origine);
            let tcp = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            monter(tcp, "127.0.0.1", port)
                .await
                .expect("le même certificat est reconnu");
            serveur.await.unwrap();
            assert_eq!(
                std::fs::read_to_string(bac.fichier_de_confiance()).unwrap(),
                contenu,
                "un serveur connu ne fait rien réécrire"
            );

            let serveur = accueillir(&ecoute, remplacant);
            let tcp = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            let refus = monter(tcp, "127.0.0.1", port)
                .await
                .expect_err("un certificat changé est refusé");
            serveur.await.unwrap();
            let msg = format!("{refus:#}");
            for attendu in [
                "a changé",
                "Empreinte présentée",
                "Empreinte attendue",
                "rdp_known_hosts",
            ] {
                assert!(msg.contains(attendu), "{attendu:?} absent de {msg:?}");
            }
            assert_eq!(
                std::fs::read_to_string(bac.fichier_de_confiance()).unwrap(),
                contenu,
                "le refus ne touche pas à l'empreinte d'origine"
            );
        });
    }

    /// Un serveur qui accepte puis raccroche sans parler TLS : le montage
    /// échoue proprement, sans panique, et le message dit VeNCrypt. Le
    /// premier jet de ce test gardait la connexion ouverte sans répondre, et
    /// attendait la poignée de main pour toujours.
    #[test]
    fn un_serveur_qui_ne_parle_pas_tls_fait_echouer_le_montage() {
        runtime().block_on(async {
            let ecoute = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = ecoute.local_addr().unwrap().port();
            let serveur = tokio::spawn(async move {
                let (tcp, _) = ecoute.accept().await.unwrap();
                drop(tcp);
            });
            let client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            let issue = monteur("127.0.0.1", port)(MaybeTls::Clair(client)).await;
            serveur.await.unwrap();
            let Err(vnc::VncError::General(msg)) = issue else {
                panic!("un TLS sans serveur TLS doit échouer");
            };
            assert!(msg.contains("VeNCrypt"), "{msg}");
        });
    }

    /// La clé d'épinglage porte le protocole : un serveur RDP et un serveur VNC
    /// sur la même adresse ne partagent pas leur empreinte.
    #[test]
    fn la_cle_d_epinglage_distingue_le_vnc() {
        assert_eq!(format!("vnc:{}:{}", "h", 5901), "vnc:h:5901");
        assert_ne!(format!("vnc:{}:{}", "h", 3389), format!("{}:{}", "h", 3389));
    }
}
