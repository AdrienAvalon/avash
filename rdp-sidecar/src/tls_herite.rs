//! Chemin TLS hérité (`--tls-herite`) : la pile TLS du système (Schannel sous
//! Windows, SecureTransport sous macOS, OpenSSL embarquée sous Linux) pour les
//! serveurs qui n'offrent aucune suite moderne.
//!
//! Trouvé le 11 septembre 2026 contre un Windows Server 2012 R2 : après une
//! négociation X.224 normale (HYBRID), Schannel coupait la connexion par un
//! RST juste après le ClientHello, sans alerte. Ce système n'a aucune suite
//! ECDHE_RSA avec AES-GCM (elles datent de Windows 10 et Server 2016) ; ses
//! seules suites AEAD exigent un certificat ECDSA ou un échange de clé RSA, et
//! rustls, qui n'offre qu'ECDHE avec AES-GCM ou ChaCha20, n'a donc rien en
//! commun avec lui. C'était le « os error 10054 » sous Windows, le « rompu
//! pendant l'établissement du canal chiffré » sous Linux.
//!
//! Ce chemin n'est jamais pris en silence : l'interface pose la question, une
//! fois par serveur, et retient le choix. Il ne relâche que le choix des suites
//! (AES-CBC, échange de clé RSA sans confidentialité persistante) : le
//! certificat reste épinglé par sa clé publique, et NLA reste exigé.

use anyhow::{anyhow, Context as _, Result};
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

/// Le canal chiffré vers le serveur, quelle que soit la pile qui l'a monté.
///
/// Les deux flux sont `Unpin` (ils enveloppent un `TcpStream`) : la délégation
/// se fait par `Pin::new`, sans projection. Le flux rustls est boxé : il pèse
/// bien plus lourd que l'autre variante, et clippy le fait remarquer.
pub enum Flux {
    /// rustls, la voie par défaut : TLS 1.2 et 1.3, suites AEAD seulement.
    Moderne(Box<ironrdp_tls::TlsStream<TcpStream>>),
    /// La pile du système, sur décision explicite de l'utilisateur.
    Herite(tokio_native_tls::TlsStream<TcpStream>),
}

impl AsyncRead for Flux {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Flux::Moderne(s) => Pin::new(&mut **s).poll_read(cx, buf),
            Flux::Herite(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Flux {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Flux::Moderne(s) => Pin::new(&mut **s).poll_write(cx, buf),
            Flux::Herite(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Flux::Moderne(s) => Pin::new(&mut **s).poll_flush(cx),
            Flux::Herite(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Flux::Moderne(s) => Pin::new(&mut **s).poll_shutdown(cx),
            Flux::Herite(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

/// Monte le canal chiffré avec la pile du système et rend le certificat que
/// le serveur a présenté, pour l'épinglage.
///
/// Aucune vérification de chaîne ni de nom : c'est l'épinglage de la clé
/// publique (`empreintes`) qui authentifie le serveur, comme sur la voie
/// rustls. Pas de SNI non plus : un serveur RDP n'en attend pas, et une
/// adresse IP n'en porte pas.
///
/// # Errors
///
/// Rend l'erreur de la pile TLS (poignée refusée, connexion coupée) ou un
/// certificat absent ou indéchiffrable.
pub async fn monter(tcp: TcpStream, hote: &str) -> Result<(Flux, x509_cert::Certificate)> {
    use x509_cert::der::Decode as _;

    let connecteur = native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(true)
        .danger_accept_invalid_hostnames(true)
        .use_sni(false)
        .build()
        .context("pile TLS du système")?;
    let connecteur = tokio_native_tls::TlsConnector::from(connecteur);
    let flux = connecteur
        .connect(hote, tcp)
        .await
        .context("poignée TLS héritée")?;
    let cert = flux
        .get_ref()
        .peer_certificate()
        .context("certificat du serveur")?
        .ok_or_else(|| anyhow!("le serveur n'a présenté aucun certificat"))?;
    let der = cert.to_der().context("certificat du serveur en DER")?;
    let cert = x509_cert::Certificate::from_der(&der).context("certificat du serveur illisible")?;
    Ok((Flux::Herite(flux), cert))
}

/// Marqueur que l'interface reconnaît pour proposer le chemin hérité.
pub const TLS_HERITE_INDISPONIBLE: &str = "[AVASH_RDP_TLS_HERITE]";

/// La phrase à afficher quand le serveur coupe pendant la poignée TLS.
///
/// Sans le chemin hérité, on propose de l'essayer, marqueur en tête : c'est
/// le comportement connu de Windows Server 2012 R2 et antérieurs. Avec lui,
/// il n'y a plus de repli à proposer : c'est le serveur qui n'a pas de
/// certificat valide, et le message envoie chercher là.
#[must_use]
pub fn message_coupure(tls_herite: bool) -> String {
    if tls_herite {
        "Ce serveur a rompu la connexion pendant l'établissement du canal chiffré, \
         y compris avec les suites TLS héritées du système. C'est le plus souvent un \
         certificat RDP absent ou abîmé côté serveur, ou une couche de sécurité \
         réglée sur « RDP » au lieu de « SSL ». Renoncer à l'authentification \
         réseau n'y changerait rien : ce repli passe lui aussi par TLS."
            .to_owned()
    } else {
        format!(
            "{TLS_HERITE_INDISPONIBLE} Ce serveur a accepté la négociation puis a rompu \
             la connexion pendant l'établissement du canal chiffré, sans retenir aucune \
             suite TLS moderne (AES-GCM ou ChaCha20 avec ECDHE). C'est le comportement \
             de Windows Server 2012 R2 et des versions antérieures. Un certificat RDP \
             absent ou abîmé côté serveur donne la même chose."
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{message_coupure, TLS_HERITE_INDISPONIBLE};

    /// Sans le chemin hérité, le marqueur ouvre la porte au repli ; avec lui,
    /// plus de marqueur : l'interface n'a rien de plus à proposer.
    #[test]
    fn le_marqueur_ne_part_que_si_le_repli_reste_possible() {
        let m = message_coupure(false);
        assert!(m.starts_with(TLS_HERITE_INDISPONIBLE), "{m}");
        assert!(
            m.contains("2012 R2"),
            "la cause la plus probable est nommée : {m}"
        );
        let m = message_coupure(true);
        assert!(!m.contains(TLS_HERITE_INDISPONIBLE), "{m}");
        assert!(
            m.contains("certificat"),
            "reste à chercher côté serveur : {m}"
        );
    }

    /// Une coupure TCP ne doit jamais porter le marqueur NLA : les deux
    /// replis sont distincts et l'interface les traite l'un après l'autre.
    #[test]
    fn le_message_ne_confond_pas_les_deux_replis() {
        assert!(!message_coupure(false).contains("[AVASH_RDP_SANS_NLA]"));
    }
}
