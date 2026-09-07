//! VeNCrypt : le VNC sous TLS, avec la même confiance au premier contact que
//! le RDP.
//!
//! Le client VNC porté sait négocier VeNCrypt (sous-types X.509) mais ne monte
//! pas TLS lui-même : il rend le flux à un « monteur » que ce module fournit.
//! Le flux est un `MaybeTls`, en clair jusqu'à l'accord, chiffré ensuite, du
//! même type avant et après pour que le client n'en sache rien. La CHAÎNE du
//! certificat n'est pas jugée par une autorité (comme pour le RDP), mais la
//! signature de la poignée de main l'est — sans quoi l'épinglage serait
//! contournable — et le certificat est épinglé : sa clé publique est mémorisée
//! au premier contact sous
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
        .with_custom_certificate_verifier(
            std::sync::Arc::new(VerifieSignatureSansChaine::nouveau()),
        )
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

/// On ne fait pas juger la CHAÎNE du certificat par une autorité — la confiance
/// vient de l'épinglage TOFU fait après la poignée de main. Mais la SIGNATURE de
/// `CertificateVerify` doit être vérifiée pour de vrai : elle seule prouve que le
/// pair possède la clé privée du certificat qu'il présente. Sans elle, l'épinglage
/// (fait sur la clé PUBLIQUE) est contournable — un interposeur rejoue le
/// certificat public légitime d'un serveur déjà connu, sans en avoir la clé
/// privée, la poignée de main aboutit, et le mot de passe VNC part chez lui.
/// Trouvé par l'audit du 7 septembre 2026 : les deux `verify_tls*_signature`
/// renvoyaient `Ok` sans condition (le VNC, contrairement au RDP sous CredSSP,
/// n'a aucune liaison de canal qui rattraperait la signature non vérifiée).
#[derive(Debug)]
struct VerifieSignatureSansChaine {
    algos: rustls::crypto::WebPkiSupportedAlgorithms,
}

impl VerifieSignatureSansChaine {
    fn nouveau() -> Self {
        Self {
            algos: rustls::crypto::aws_lc_rs::default_provider().signature_verification_algorithms,
        }
    }
}

impl rustls::client::danger::ServerCertVerifier for VerifieSignatureSansChaine {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        // La chaîne n'est pas jugée ici : c'est l'épinglage sur la clé publique
        // (juger_certificat, après la poignée de main) qui décide de la confiance.
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algos)
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algos)
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.algos.supported_schemes()
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

    /// Certificat RSA-2048 auto-signé jetable (DER), généré une fois par
    /// openssl. Sert à prouver que le vérificateur rejette une signature qui ne
    /// correspond pas à sa clé publique.
    const CERT_TEST_DER: &[u8] = include_bytes!("vnc_tls_cert_test.der");

    /// Trouvé par l'audit du 7 septembre 2026 : le vérificateur renvoyait `Ok`
    /// sans regarder la signature de `CertificateVerify`. L'épinglage se fait
    /// sur la clé publique du certificat ; si la signature n'est pas vérifiée,
    /// un interposeur rejoue le certificat public d'un serveur déjà connu, sans
    /// en posséder la clé privée, et la connexion aboutit. Ici on présente le
    /// certificat de test avec une signature bidon : elle DOIT être rejetée.
    /// Contrôle négatif : avec l'ancien corps (`Ok(...)` inconditionnel), les
    /// deux `assert!(...is_err())` échouaient.
    #[test]
    fn une_signature_de_poignee_de_main_invalide_est_rejetee() {
        use rustls::client::danger::ServerCertVerifier as _;
        use rustls::internal::msgs::codec::{Codec as _, Reader};
        let verif = VerifieSignatureSansChaine::nouveau();
        let cert = rustls::pki_types::CertificateDer::from(CERT_TEST_DER);
        // Signature RSA-PSS-SHA256 de 256 octets nuls : structurellement
        // plausible, cryptographiquement fausse pour ce certificat. Construite
        // par sa forme filaire (schéma u16 = 0x0804, longueur u16 = 256, puis
        // la signature), le constructeur direct étant privé à rustls.
        let mut filaire = vec![0x08u8, 0x04, 0x01, 0x00];
        filaire.extend(std::iter::repeat_n(0u8, 256));
        let bidon =
            rustls::DigitallySignedStruct::read(&mut Reader::init(&filaire)).expect("DSS filaire");
        assert!(
            verif
                .verify_tls13_signature(b"transcript", &cert, &bidon)
                .is_err(),
            "une signature TLS 1.3 invalide doit être refusée"
        );
        assert!(
            verif
                .verify_tls12_signature(b"transcript", &cert, &bidon)
                .is_err(),
            "une signature TLS 1.2 invalide doit être refusée"
        );
        // Et le vérificateur annonce bien des schémas (sinon rustls n'appelle
        // jamais verify_tls*_signature, et la vérification serait morte).
        assert!(!verif.supported_verify_schemes().is_empty());
    }
}
