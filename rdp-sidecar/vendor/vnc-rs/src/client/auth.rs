use super::security;
use crate::{VncError, VncVersion};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Plus longue raison d'échec que le client accepte de lire (RFB 3.8 §7.1.2).
/// Ces textes sont des phrases (« Authentication failed. ») : mille octets les
/// contiennent toutes, et une annonce plus longue est le fait d'un serveur qui
/// cherche à faire lire, pas à s'expliquer.
pub(super) const RAISON_MAX: u32 = 1024;

/// Lit la raison qui suit un refus (échec d'authentification ou de connexion) :
/// une longueur sur quatre octets, puis exactement autant d'octets, bornés à
/// [`RAISON_MAX`]. Une fin de flux rend une raison vide : certains serveurs
/// (rustvncserver) raccrochent sans rien dire, et c'est encore un refus, pas
/// une erreur de lecture (« unexpected end of file » à l'écran).
///
/// Trouvé par l'audit du 9 septembre 2026 : la longueur annoncée était lue
/// puis jetée, et la raison ramassée par `read_to_string`, qui ne s'arrête qu'à
/// la fermeture du flux. Un serveur qui refuse puis garde la connexion ouverte
/// en y déversant des octets tenait le sidecar en lecture jusqu'au délai de
/// connexion (25 s, `rdp-sidecar/src/vnc.rs`), à empiler tout ce qu'il envoyait.
/// C'est le principe déjà appliqué au nom de bureau de ServerInit : borner
/// côté client plutôt que faire confiance à ce que le serveur annonce.
pub(super) async fn lire_raison<S>(reader: &mut S) -> String
where
    S: AsyncRead + Unpin,
{
    let Ok(annoncee) = reader.read_u32().await else {
        return String::new();
    };
    let mut brut = vec![0u8; annoncee.min(RAISON_MAX) as usize];
    if reader.read_exact(&mut brut).await.is_err() {
        return String::new();
    }
    String::from_utf8_lossy(&brut).into_owned()
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum SecurityType {
    Invalid = 0,
    None = 1,
    VncAuth = 2,
    RA2 = 5,
    RA2ne = 6,
    Tight = 16,
    Ultra = 17,
    Tls = 18,
    VeNCrypt = 19,
    GtkVncSasl = 20,
    Md5Hash = 21,
    ColinDeanXvp = 22,
}

impl TryFrom<u8> for SecurityType {
    type Error = VncError;
    fn try_from(num: u8) -> Result<Self, Self::Error> {
        match num {
            0 | 1 | 2 | 5 | 6 | 16 | 17 | 18 | 19 | 20 | 21 | 22 => {
                Ok(unsafe { std::mem::transmute::<u8, SecurityType>(num) })
            }
            invalid => Err(VncError::InvalidSecurityTyep(invalid)),
        }
    }
}

impl From<SecurityType> for u8 {
    fn from(e: SecurityType) -> Self {
        e as u8
    }
}

impl SecurityType {
    pub(super) async fn read<S>(reader: &mut S, version: &VncVersion) -> Result<Vec<Self>, VncError>
    where
        S: AsyncRead + Unpin,
    {
        match version {
            VncVersion::RFB33 => {
                let security_type = reader.read_u32().await?;
                let security_type = (security_type as u8).try_into()?;
                if let SecurityType::Invalid = security_type {
                    return Err(VncError::General(lire_raison(reader).await));
                }
                Ok(vec![security_type])
            }
            _ => {
                // +--------------------------+-------------+--------------------------+
                // | No. of bytes             | Type        | Description              |
                // |                          | [Value]     |                          |
                // +--------------------------+-------------+--------------------------+
                // | 1                        | U8          | number-of-security-types |
                // | number-of-security-types | U8 array    | security-types           |
                // +--------------------------+-------------+--------------------------+
                let num = reader.read_u8().await?;

                if num == 0 {
                    return Err(VncError::General(lire_raison(reader).await));
                }
                // Le protocole veut que le client ignore les types de sécurité
                // qu'il ne connaît pas et en choisisse un qu'il parle. Un
                // `try_into()?` dans la boucle faisait au contraire tomber toute
                // la poignée de main au premier octet hors énumération — or le
                // partage d'écran macOS (ARD 30, 33, 35, 36), UltraVNC MS-Logon
                // (113) et RealVNC (129, 130) en annoncent à côté de VncAuth.
                // On lit donc toujours les `num` octets (garder le flux aligné)
                // et on n'écarte que les inconnus, sans échouer tant qu'un type
                // connu reste. Trouvé par l'audit du 7 septembre 2026.
                let mut sec_types = vec![];
                let mut inconnus = vec![];
                for _ in 0..num {
                    let brut = reader.read_u8().await?;
                    match SecurityType::try_from(brut) {
                        Ok(t) => sec_types.push(t),
                        Err(_) => inconnus.push(brut),
                    }
                }
                if !inconnus.is_empty() {
                    tracing::debug!("types de sécurité inconnus ignorés : {inconnus:?}");
                }
                if sec_types.is_empty() {
                    return Err(VncError::General(format!(
                        "le serveur n'offre que des types de sécurité inconnus : {inconnus:?}"
                    )));
                }
                tracing::trace!("Server supported security type: {:?}", sec_types);
                Ok(sec_types)
            }
        }
    }

    pub(super) async fn write<S>(&self, writer: &mut S) -> Result<(), VncError>
    where
        S: AsyncWrite + Unpin,
    {
        writer.write_all(&[(*self).into()]).await?;
        Ok(())
    }
}

#[allow(dead_code)]
#[repr(u32)]
pub(super) enum AuthResult {
    Ok = 0,
    Failed = 1,
}

impl From<u32> for AuthResult {
    // Le résultat vient du serveur, qui n'est pas tenu de n'envoyer que 0 ou
    // 1 : un `transmute` d'une autre valeur vers cette énumération à deux
    // variantes était un comportement indéfini. Tout ce qui n'est pas « ok »
    // est un échec, et rien d'autre.
    fn from(num: u32) -> Self {
        if num == 0 {
            AuthResult::Ok
        } else {
            AuthResult::Failed
        }
    }
}

impl From<AuthResult> for u32 {
    fn from(e: AuthResult) -> Self {
        e as u32
    }
}

pub(super) struct AuthHelper {
    challenge: [u8; 16],
    key: [u8; 8],
}

impl AuthHelper {
    pub(super) async fn read<S>(reader: &mut S, credential: &str) -> Result<Self, VncError>
    where
        S: AsyncRead + Unpin,
    {
        let mut challenge = [0; 16];
        reader.read_exact(&mut challenge).await?;

        let credential_len = credential.len();
        let mut key = [0u8; 8];
        for (i, key_i) in key.iter_mut().enumerate() {
            let c = if i < credential_len {
                credential.as_bytes()[i]
            } else {
                0
            };
            let mut cs = 0u8;
            for j in 0..8 {
                cs |= ((c >> j) & 1) << (7 - j)
            }
            *key_i = cs;
        }

        Ok(Self { challenge, key })
    }

    pub(super) async fn write<S>(&self, writer: &mut S) -> Result<(), VncError>
    where
        S: AsyncWrite + Unpin,
    {
        let encrypted = security::des::encrypt(&self.challenge, &self.key);
        writer.write_all(&encrypted).await?;
        Ok(())
    }

    pub(super) async fn finish<S>(self, reader: &mut S) -> Result<AuthResult, VncError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let result = reader.read_u32().await?;
        Ok(result.into())
    }
}
