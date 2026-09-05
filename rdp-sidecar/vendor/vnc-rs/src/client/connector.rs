use super::{
    auth::{AuthHelper, AuthResult, SecurityType},
    connection::VncClient,
};
use std::future::Future;
use std::pin::Pin;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tracing::{info, trace};

use crate::{PixelFormat, VncEncoding, VncError, VncVersion};

pub enum VncState<S, F>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
    F: Future<Output = Result<String, VncError>> + Send + Sync + 'static,
{
    Handshake(VncConnector<S, F>),
    Authenticate(VncConnector<S, F>),
    Connected(VncClient),
}

impl<S, F> VncState<S, F>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
    F: Future<Output = Result<String, VncError>> + Send + Sync + 'static,
{
    pub fn try_start(
        self,
    ) -> Pin<Box<dyn Future<Output = Result<Self, VncError>> + Send + Sync + 'static>> {
        Box::pin(async move {
            match self {
                VncState::Handshake(mut connector) => {
                    // Read the rfbversion informed by the server
                    let rfbversion = VncVersion::read(&mut connector.stream).await?;
                    trace!(
                        "Our version {:?}, server version {:?}",
                        connector.rfb_version,
                        rfbversion
                    );
                    let rfbversion = if connector.rfb_version < rfbversion {
                        connector.rfb_version
                    } else {
                        rfbversion
                    };

                    // Record the negotiated rfbversion
                    connector.rfb_version = rfbversion;
                    trace!("Negotiated rfb version: {:?}", rfbversion);
                    rfbversion.write(&mut connector.stream).await?;
                    Ok(VncState::Authenticate(connector).try_start().await?)
                }
                VncState::Authenticate(mut connector) => {
                    let security_types =
                        SecurityType::read(&mut connector.stream, &connector.rfb_version).await?;

                    // Une liste vide vient d'un serveur, pas d'un défaut du
                    // client : une erreur, jamais une panique.
                    if security_types.is_empty() {
                        return Err(VncError::General(
                            "le serveur n'annonce aucun type de sécurité".to_owned(),
                        ));
                    }

                    // VeNCrypt d'abord, quand l'appelant sait monter TLS : un
                    // serveur qui l'offre à côté de l'authentification VNC
                    // classique préfère qu'on chiffre (portage avash).
                    if security_types.contains(&SecurityType::VeNCrypt)
                        && connector.tls_upgrader.is_some()
                        && connector.rfb_version != VncVersion::RFB33
                    {
                        return vencrypt(connector).await;
                    }

                    if security_types.contains(&SecurityType::None) {
                        match connector.rfb_version {
                            VncVersion::RFB33 => {
                                // If the security-type is 1, for no authentication, the server does not
                                // send the SecurityResult message but proceeds directly to the
                                // initialization messages (Section 7.3).
                                info!("No auth needed in vnc3.3");
                            }
                            VncVersion::RFB37 => {
                                // After the security handshake, if the security-type is 1, for no
                                // authentication, the server does not send the SecurityResult message
                                // but proceeds directly to the initialization messages (Section 7.3).
                                info!("No auth needed in vnc3.7");
                                SecurityType::write(&SecurityType::None, &mut connector.stream)
                                    .await?;
                            }
                            VncVersion::RFB38 => {
                                info!("No auth needed in vnc3.8");
                                SecurityType::write(&SecurityType::None, &mut connector.stream)
                                    .await?;
                                let mut ok = [0; 4];
                                connector.stream.read_exact(&mut ok).await?;
                            }
                        }
                    } else {
                        // choose a auth method
                        if security_types.contains(&SecurityType::VncAuth) {
                            if connector.rfb_version != VncVersion::RFB33 {
                                // In the security handshake (Section 7.1.2), rather than a two-way
                                // negotiation, the server decides the security type and sends a single
                                // word:

                                //            +--------------+--------------+---------------+
                                //            | No. of bytes | Type [Value] | Description   |
                                //            +--------------+--------------+---------------+
                                //            | 4            | U32          | security-type |
                                //            +--------------+--------------+---------------+

                                // The security-type may only take the value 0, 1, or 2.  A value of 0
                                // means that the connection has failed and is followed by a string
                                // giving the reason, as described in Section 7.1.2.
                                SecurityType::write(&SecurityType::VncAuth, &mut connector.stream)
                                    .await?;
                            }
                        } else {
                            let msg = "Security type apart from Vnc Auth has not been implemented";
                            return Err(VncError::General(msg.to_owned()));
                        }

                        // get password
                        if connector.auth_methond.is_none() {
                            return Err(VncError::NoPassword);
                        }

                        let credential = (connector.auth_methond.take().unwrap()).await?;

                        // auth
                        let auth = AuthHelper::read(&mut connector.stream, &credential).await?;
                        auth.write(&mut connector.stream).await?;
                        let result = auth.finish(&mut connector.stream).await?;
                        if let AuthResult::Failed = result {
                            if let VncVersion::RFB37 = connector.rfb_version {
                                // In VNC Authentication (Section 7.2.2), if the authentication fails,
                                // the server sends the SecurityResult message, but does not send an
                                // error message before closing the connection.
                                return Err(VncError::WrongPassword);
                            } else {
                                // La raison du serveur (« Authentication failed »,
                                // le plus souvent) ne dit rien de plus que l'échec
                                // lui-même : on la trace, et l'appelant reçoit le
                                // même `WrongPassword` qu'en 3.7, qu'il sait
                                // présenter. Certains serveurs (rustvncserver)
                                // raccrochent sans l'envoyer : une fin de flux
                                // ici est encore un refus, pas une erreur de
                                // lecture (« unexpected end of file » à l'écran).
                                let mut err_msg = String::new();
                                if connector.stream.read_u32().await.is_ok() {
                                    let _ = connector.stream.read_to_string(&mut err_msg).await;
                                }
                                trace!("authentification refusée : {err_msg}");
                                return Err(VncError::WrongPassword);
                            }
                        }
                    }
                    info!("auth done, client connected");

                    Ok(VncState::Connected(
                        VncClient::new(
                            connector.stream,
                            connector.allow_shared,
                            connector.pixel_format,
                            connector.encodings,
                        )
                        .await?,
                    ))
                }
                _ => unreachable!(),
            }
        })
    }

    pub fn finish(self) -> Result<VncClient, VncError> {
        if let VncState::Connected(client) = self {
            Ok(client)
        } else {
            Err(VncError::ConnectError)
        }
    }
}

/// VeNCrypt (portage avash) : version 0.2, sous-type X.509, TLS monté par
/// l'appelant, puis le défi VNC (X509Vnc) ou le seul résultat (X509None).
///
/// Ce que le serveur annonce est lu tel quel, borné : au plus 32 sous-types.
async fn vencrypt<S, F>(connector: VncConnector<S, F>) -> Result<VncState<S, F>, VncError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
    F: Future<Output = Result<String, VncError>> + Send + Sync + 'static,
{
    use tokio::io::AsyncWriteExt as _;
    let VncConnector {
        mut stream,
        auth_methond,
        rfb_version: _,
        allow_shared,
        pixel_format,
        encodings,
        tls_upgrader,
    } = connector;
    SecurityType::write(&SecurityType::VeNCrypt, &mut stream).await?;
    // Version du serveur (majeur, mineur) ; on répond 0.2, la seule qui
    // porte les sous-types sur quatre octets.
    let mut version = [0u8; 2];
    stream.read_exact(&mut version).await?;
    if version[0] != 0 || version[1] < 2 {
        return Err(VncError::General(format!(
            "version VeNCrypt {}.{} non prise en charge",
            version[0], version[1]
        )));
    }
    stream.write_all(&[0, 2]).await?;
    if stream.read_u8().await? != 0 {
        return Err(VncError::General(
            "le serveur refuse la version VeNCrypt 0.2".to_owned(),
        ));
    }
    let n = stream.read_u8().await?;
    if n == 0 || n > 32 {
        return Err(VncError::General(format!(
            "liste de sous-types VeNCrypt invalide ({n})"
        )));
    }
    let mut sous_types = Vec::with_capacity(usize::from(n));
    for _ in 0..n {
        sous_types.push(stream.read_u32().await?);
    }
    trace!("sous-types VeNCrypt du serveur : {sous_types:?}");
    let choisi = if sous_types.contains(&VENCRYPT_X509_VNC) {
        VENCRYPT_X509_VNC
    } else if sous_types.contains(&VENCRYPT_X509_NONE) {
        VENCRYPT_X509_NONE
    } else {
        return Err(VncError::General(
            "Le serveur VeNCrypt ne propose aucun sous-type X.509 : les sous-types \
             TLS anonymes (sans certificat) ne prouvent pas à qui l'on parle, et ce \
             client ne les accepte pas."
                .to_owned(),
        ));
    };
    stream.write_all(&choisi.to_be_bytes()).await?;
    if stream.read_u8().await? != 1 {
        return Err(VncError::General(
            "le serveur a refusé le sous-type VeNCrypt choisi".to_owned(),
        ));
    }
    // Le flux passe sous TLS : c'est là que le certificat est jugé.
    let upgrader = tls_upgrader.expect("VeNCrypt n'est choisi qu'avec un monteur TLS");
    let mut stream = upgrader(stream).await?;
    if choisi == VENCRYPT_X509_VNC {
        let Some(auth_methond) = auth_methond else {
            return Err(VncError::NoPassword);
        };
        let credential = auth_methond.await?;
        let auth = AuthHelper::read(&mut stream, &credential).await?;
        auth.write(&mut stream).await?;
        if let AuthResult::Failed = auth.finish(&mut stream).await? {
            let mut err_msg = String::new();
            if stream.read_u32().await.is_ok() {
                let _ = stream.read_to_string(&mut err_msg).await;
            }
            trace!("authentification VeNCrypt refusée : {err_msg}");
            return Err(VncError::WrongPassword);
        }
    } else if stream.read_u32().await? != 0 {
        // X509None : le serveur envoie quand même un résultat de sécurité.
        return Err(VncError::General(
            "le serveur a refusé la session VeNCrypt".to_owned(),
        ));
    }
    info!("VeNCrypt établi, client connecté");
    Ok(VncState::Connected(
        VncClient::new(stream, allow_shared, pixel_format, encodings).await?,
    ))
}

/// Connection Builder to setup a vnc client
pub struct VncConnector<S, F>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    F: Future<Output = Result<String, VncError>> + Send + Sync + 'static,
{
    stream: S,
    auth_methond: Option<F>,
    rfb_version: VncVersion,
    allow_shared: bool,
    pixel_format: Option<PixelFormat>,
    encodings: Vec<VncEncoding>,
    /// Monte TLS sur le flux au moment où VeNCrypt le demande (portage avash).
    /// Sans lui, le type de sécurité 19 n'est pas choisi.
    tls_upgrader: Option<TlsUpgrader<S>>,
}

/// Ce qui transforme le flux en clair en flux chiffré, à l'instant que VeNCrypt
/// fixe (après l'accord sur le sous-type X.509). Le type reste `S` : c'est à
/// l'appelant de fournir un flux qui sait être l'un ou l'autre.
pub type TlsUpgrader<S> = Box<
    dyn FnOnce(S) -> Pin<Box<dyn Future<Output = Result<S, VncError>> + Send + Sync + 'static>>
        + Send
        + Sync
        + 'static,
>;

/// Sous-types VeNCrypt (RFB 3.8, extension VeNCrypt 0.2). Seuls les X.509 sont
/// choisis : les `TLS*` reposent sur un TLS anonyme (Diffie-Hellman sans
/// certificat) que rustls ne parle pas, et qui ne prouve rien de toute façon.
const VENCRYPT_X509_NONE: u32 = 260;
const VENCRYPT_X509_VNC: u32 = 261;

impl<S, F> VncConnector<S, F>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
    F: Future<Output = Result<String, VncError>> + Send + Sync + 'static,
{
    /// To new a vnc client configuration with stream `S`
    ///
    /// `S` should implement async I/O methods
    ///
    /// ```no_run
    /// use vnc::{PixelFormat, VncConnector, VncError};
    /// use tokio::{self, net::TcpStream};
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), VncError> {
    ///     let tcp = TcpStream::connect("127.0.0.1:5900").await?;
    ///     let vnc = VncConnector::new(tcp)
    ///         .set_auth_method(async move { Ok("password".to_string()) })
    ///         .add_encoding(vnc::VncEncoding::Tight)
    ///         .add_encoding(vnc::VncEncoding::Zrle)
    ///         .add_encoding(vnc::VncEncoding::CopyRect)
    ///         .add_encoding(vnc::VncEncoding::Raw)
    ///         .allow_shared(true)
    ///         .set_pixel_format(PixelFormat::bgra())
    ///         .build()?
    ///         .try_start()
    ///         .await?
    ///         .finish()?;
    ///     Ok(())
    /// }
    /// ```
    ///
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            auth_methond: None,
            allow_shared: true,
            rfb_version: VncVersion::RFB38,
            pixel_format: None,
            encodings: Vec::new(),
            tls_upgrader: None,
        }
    }

    /// Accepte VeNCrypt (type de sécurité 19) : `upgrader` monte TLS sur le
    /// flux quand le serveur et nous sommes tombés d'accord sur un sous-type
    /// X.509, et c'est lui qui juge le certificat (portage avash).
    pub fn set_tls_upgrader(mut self, upgrader: TlsUpgrader<S>) -> Self {
        self.tls_upgrader = Some(upgrader);
        self
    }

    /// An async callback which is used to query credentials if the vnc server has set
    ///
    /// ```no_compile
    /// connector = connector.set_auth_method(async move { Ok("password".to_string()) })
    /// ```
    ///
    /// if you're building a wasm app,
    /// the async callback also allows you to combine it to a promise
    ///
    /// ```no_compile
    /// #[wasm_bindgen]
    /// extern "C" {
    ///     fn get_password() -> js_sys::Promise;
    /// }
    ///
    /// connector = connector
    ///        .set_auth_method(async move {
    ///            let auth = JsFuture::from(get_password()).await.unwrap();
    ///            Ok(auth.as_string().unwrap())
    ///     });
    /// ```
    ///
    /// While in the js code
    ///
    ///
    /// ```javascript
    /// var password = '';
    /// function get_password() {
    ///     return new Promise((reslove, reject) => {
    ///        document.getElementById("submit_password").addEventListener("click", () => {
    ///             password = window.document.getElementById("input_password").value
    ///             reslove(password)
    ///         })
    ///     });
    /// }
    /// ```
    ///
    /// The future won't be polled if the sever doesn't apply any password protections to the session
    ///
    pub fn set_auth_method(mut self, auth_callback: F) -> Self {
        self.auth_methond = Some(auth_callback);
        self
    }

    /// The max vnc version that we supported
    ///
    /// Version should be one of the [VncVersion]
    ///
    pub fn set_version(mut self, version: VncVersion) -> Self {
        self.rfb_version = version;
        self
    }

    /// Set the rgb order which you will use to resolve the image data
    ///
    /// In most of the case, use `PixelFormat::bgra()` on little endian PCs
    ///
    /// And use `PixelFormat::rgba()` on wasm apps (with canvas)
    ///
    /// Also, customized format is allowed
    ///
    /// Will use the default format informed by the vnc server if not set
    ///
    /// In this condition, the client will get a [crate::VncEvent::SetPixelFormat] event notified
    ///
    pub fn set_pixel_format(mut self, pf: PixelFormat) -> Self {
        self.pixel_format = Some(pf);
        self
    }

    /// Shared-flag is non-zero (true) if the server should try to share the
    ///
    /// desktop by leaving other clients connected, and zero (false) if it
    ///
    /// should give exclusive access to this client by disconnecting all
    ///
    /// other clients.
    ///
    pub fn allow_shared(mut self, allow_shared: bool) -> Self {
        self.allow_shared = allow_shared;
        self
    }

    /// Client encodings that we want to use
    ///
    /// One of [VncEncoding]
    ///
    /// [VncEncoding::Raw] must be sent as the RFC required
    ///
    /// The order to add encodings is the order to inform the server
    ///
    pub fn add_encoding(mut self, encoding: VncEncoding) -> Self {
        self.encodings.push(encoding);
        self
    }

    /// Complete the client configuration
    ///
    pub fn build(self) -> Result<VncState<S, F>, VncError> {
        if self.encodings.is_empty() {
            return Err(VncError::NoEncoding);
        }
        Ok(VncState::Handshake(self))
    }
}
