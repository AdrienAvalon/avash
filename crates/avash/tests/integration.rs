//! Tests d'intégration avash : serveur SSH+SFTP embarqué (russh server),
//! client avash réel dessus. Valide connect/auth/exec/PTY/SFTP bout-en-bout.

use russh::keys::PrivateKey;
use russh::server::{Auth, Msg, Server as _, Session};
use russh::{Channel, ChannelId};
use russh_sftp::protocol::{File, FileAttributes, Handle, Status, StatusCode};
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use tokio::sync::Mutex;

// ---------- Serveur SSH de test ----------

/// Reponses recues par le serveur sur ses canaux `forwarded-tcpip` (test -R).
static REMOTE_REPLY: Mutex<Vec<Vec<u8>>> = Mutex::const_new(Vec::new());

#[derive(Clone)]
struct TestSshServer;

impl russh::server::Server for TestSshServer {
    type Handler = TestSshSession;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self::Handler {
        TestSshSession::default()
    }
}

#[derive(Default)]
struct TestSshSession {
    channels: Arc<Mutex<HashMap<ChannelId, Channel<Msg>>>>,
    /// Canaux ayant demande le sous-systeme SFTP : ils transportent du binaire,
    /// l'echo du test PTY les corromprait.
    sftp_channels: Arc<Mutex<std::collections::HashSet<ChannelId>>>,
}

impl russh::server::Handler for TestSshSession {
    type Error = anyhow::Error;

    async fn auth_publickey(
        &mut self,
        user: &str,
        _key: &russh::keys::PublicKey,
    ) -> Result<Auth, Self::Error> {
        // Les comptes « pam-* » refusent la clé : sans quoi le test
        // n'atteindrait jamais keyboard-interactive.
        if user.starts_with("pam-") {
            return Ok(Auth::reject());
        }
        // Un serveur qui accepte tout le monde ne peut pas exercer les chemins
        // d'échec : le marqueur PASSWORD_REQUIRED, sur lequel repose toute la
        // relance de saisie côté interface, n'était produit par aucun test.
        // L'utilisateur « refuse » sert précisément à cela.
        if user == "refuse" {
            return Ok(Auth::reject());
        }
        Ok(Auth::Accept)
    }

    /// Conversation PAM, telle qu'un hôte joint à un annuaire l'impose.
    ///
    /// Le serveur pose une invite masquée et attend la réponse : c'est le seul
    /// moyen d'exercer le chemin `keyboard-interactive`, qu'aucun test ne
    /// couvrait — et qu'Avash ne savait pas emprunter.
    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        user: &str,
        _submethods: &str,
        response: Option<russh::server::Response<'a>>,
    ) -> Result<Auth, Self::Error> {
        // « pam-seul » n'accepte QUE cette méthode, comme un serveur dont
        // PasswordAuthentication est désactivé.
        if user != "pam-seul" && user != "pam-double" && user != "pam-otp" {
            return Ok(Auth::reject());
        }
        let Some(mut r) = response else {
            // Premier tour : on pose la ou les questions.
            let prompts: Vec<(std::borrow::Cow<'static, str>, bool)> = match user {
                // Une invite en clair : Avash doit refuser d'y répondre plutôt
                // que d'y envoyer le mot de passe.
                "pam-otp" => vec![("Code à usage unique : ".into(), true)],
                "pam-double" => vec![
                    ("Password: ".into(), false),
                    ("Password again: ".into(), false),
                ],
                _ => vec![("Password: ".into(), false)],
            };
            return Ok(Auth::Partial {
                name: "PAM".into(),
                instructions: String::new().into(),
                prompts: prompts.into(),
            });
        };
        *DERNIER_UTILISATEUR.lock().unwrap() = Some(user.to_owned());
        let attendu = b"le-bon".as_slice();
        let toutes_bonnes = r.all(|rep| rep == attendu);
        if toutes_bonnes {
            Ok(Auth::Accept)
        } else {
            Ok(Auth::reject())
        }
    }

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        // Les comptes « pam-* » n'acceptent PAS le mot de passe simple : c'est
        // ce qui force le repli vers keyboard-interactive.
        if user.starts_with("pam-") {
            return Ok(Auth::reject());
        }
        // Consigné tel quel : c'est ce que le serveur voit réellement, et le
        // seul moyen de vérifier qu'un nom de domaine « DOMAINE\\utilisateur »
        // traverse la chaîne sans être abîmé.
        *DERNIER_UTILISATEUR.lock().unwrap() = Some(user.to_owned());
        // « refuse » n'accepte qu'un mot de passe précis : de quoi distinguer
        // « il en faut un » de « celui-ci est mauvais ».
        if user == "refuse" && password != "le-bon" {
            return Ok(Auth::reject());
        }
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        self.channels.lock().await.insert(channel.id(), channel);
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel_id: ChannelId,
        request: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let channel = self.channels.lock().await.remove(&channel_id).unwrap();
        let _ = session.channel_success(channel_id);
        let cmd = String::from_utf8_lossy(request).into_owned();
        let output = format!("CMD:{cmd}\r\n");
        let _ = session.data(channel_id, bytes::Bytes::from(output.into_bytes()));
        let _ = session.extended_data(channel_id, 1, bytes::Bytes::from_static(b"stderr-ok"));

        // `exit N` dans la commande -> code N, pour tester les codes non nuls.
        let code = cmd
            .split_whitespace()
            .skip_while(|w| *w != "exit")
            .nth(1)
            .and_then(|n| n.parse::<u32>().ok())
            .unwrap_or(0);

        // ⚠️ ORDRE REEL D'OPENSSH : data, puis EOF, puis exit-status, puis
        // close. Le code envoyait exit-status AVANT eof, ce qui masquait un
        // bug ou run() cassait sur Eof et renvoyait toujours 0.
        let _ = session.eof(channel_id);
        let _ = session.exit_status_request(channel_id, code);
        let _ = session.close(channel_id);
        let _ = channel;
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel_id: ChannelId,
        term: &str,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let _ = session.channel_success(channel_id);
        let banner = format!("\r\nPTY({term} {col_width}x{row_height})\r\n");
        let _ = session.data(channel_id, bytes::Bytes::from(banner.into_bytes()));
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel_id: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let _ = session.channel_success(channel_id);
        Ok(())
    }

    async fn data(
        &mut self,
        channel_id: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        // Un canal SFTP transporte du binaire : l'echo du test PTY le corromprait.
        if self.sftp_channels.lock().await.contains(&channel_id) {
            return Ok(());
        }
        // Un canal ouvert par le serveur lui-meme (forwarded-tcpip, test -R)
        // n'est pas dans la table : sa reponse est lue par la tache qui l'a
        // ouvert, pas renvoyee en echo.
        if !self.channels.lock().await.contains_key(&channel_id) {
            return Ok(());
        }
        let echo = format!("ECHO:{}", String::from_utf8_lossy(data));
        let _ = session.data(channel_id, bytes::Bytes::from(echo.into_bytes()));
        Ok(())
    }

    /// Le client a fini d'ecrire : un vrai sshd ferme alors la connexion
    /// vers la destination, puis le canal. On imite pour que le relais du
    /// client termine bien sa connexion.
    async fn channel_eof(
        &mut self,
        channel_id: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.lock().await.remove(&channel_id);
        let _ = session.close(channel_id);
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        channel_id: ChannelId,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let msg = format!("\r\nRESIZED:{col_width}x{row_height}\r\n");
        let _ = session.data(channel_id, bytes::Bytes::from(msg.into_bytes()));
        Ok(())
    }

    /// `ssh -L` / `-D` : le client demande a joindre une destination. Le
    /// serveur de test ne joint rien : il accepte et fait echo (via `data`),
    /// ce qui suffit a prouver que les octets traversent le tunnel.
    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host_to_connect: &str,
        port_to_connect: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        // Une destination sentinelle simule un refus (hote injoignable).
        if host_to_connect == "injoignable" {
            reply.reject(russh::ChannelOpenFailure::ConnectFailed).await;
            return Ok(());
        }
        // Vers une IP loopback : vrai pont TCP (permet un ProxyJump vers un
        // second sshd). Vers un nom quelconque : echo (tests de tunnels).
        let is_loopback = host_to_connect == "127.0.0.1" || host_to_connect == "localhost";
        reply.accept().await;
        if is_loopback {
            let target = format!("127.0.0.1:{port_to_connect}");
            tokio::spawn(async move {
                if let Ok(mut tcp) = tokio::net::TcpStream::connect(&target).await {
                    let mut stream = channel.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut tcp, &mut stream).await;
                }
            });
        } else {
            self.channels.lock().await.insert(channel.id(), channel);
        }
        Ok(())
    }

    /// `ssh -R` : le client demande qu'on ecoute pour lui. Le serveur de test
    /// n'ecoute rien : il ouvre aussitot un canal `forwarded-tcpip` vers le
    /// client, envoie « hello » et renvoie la reponse recue dans un second
    /// message, pour que le test constate le trajet complet.
    async fn tcpip_forward(
        &mut self,
        _address: &str,
        port: &mut u32,
        session: &mut Session,
    ) -> Result<bool, Self::Error> {
        if *port == 0 {
            *port = 40_000;
        }
        let port = *port;
        let handle = session.handle();
        tokio::spawn(async move {
            // Laisse au client le temps d'enregistrer la redirection.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let Ok(mut ch) = handle
                .channel_open_forwarded_tcpip("localhost", port, "10.9.8.7", 5555)
                .await
            else {
                return;
            };
            let _ = ch.data(&b"hello"[..]).await;
            // Relit la reponse et la stocke pour le test.
            while let Some(msg) = ch.wait().await {
                if let russh::ChannelMsg::Data { data } = msg {
                    REMOTE_REPLY.lock().await.push(data.to_vec());
                    break;
                }
            }
            let _ = ch.close().await;
        });
        Ok(true)
    }

    async fn subsystem_request(
        &mut self,
        channel_id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if name == "sftp" {
            self.sftp_channels.lock().await.insert(channel_id);
            let channel = self.channels.lock().await.remove(&channel_id).unwrap();
            let _ = session.channel_success(channel_id);
            let sftp = TestSftpSession::default();
            tokio::spawn(async move {
                russh_sftp::server::run(channel.into_stream(), sftp).await;
            });
        } else {
            let _ = session.channel_failure(channel_id);
        }
        Ok(())
    }
}

// ---------- Système de fichiers SFTP factice en mémoire ----------

/// Operations de modification recues par le simulacre SFTP.
static SFTP_OPS: Mutex<Vec<String>> = Mutex::const_new(Vec::new());

fn ok_status(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: String::new(),
        language_tag: String::new(),
    }
}

#[derive(Default)]
/// Trois drapeaux décrivent le chemin ouvert (coupure, gros fichier), un
/// quatrième l'état de lecture. Les regrouper en énumération alourdirait un
/// serveur de démonstration sans rien clarifier.
#[allow(clippy::struct_excessive_bools)]
struct TestSftpSession {
    root_read_done: bool,
    /// Octets deja servis par `read()` : sans cet etat, le serveur renvoie le
    /// contenu indefiniment et le client telecharge en boucle infinie.
    file_read_done: bool,
    /// Le chemin ouvert demande une coupure après le premier bloc.
    coupure_en_lecture: bool,
    /// Le chemin ouvert sert le fichier de démonstration, à décalage honoré.
    gros_fichier: bool,
    /// Le chemin ouvert annonce plus d'octets qu'il n'en sert.
    tronque: bool,
}

// Les methodes du trait Handler de russh-sftp sont declarees
// `fn ... -> impl Future<...> + Send`. On calque cette signature plutot que
// d'utiliser `async fn` : clippy suggere l'inverse, mais coller au trait rend
// l'implementation plus lisible face a la definition upstream.
#[allow(clippy::manual_async_fn)]
impl russh_sftp::server::Handler for TestSftpSession {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    fn open(
        &mut self,
        id: u32,
        filename: String,
        _pflags: russh_sftp::protocol::OpenFlags,
        _attrs: FileAttributes,
    ) -> impl Future<Output = Result<Handle, Self::Error>> + Send {
        self.coupure_en_lecture = filename.contains("coupure");
        self.gros_fichier = filename.contains("gros");
        // « tronque » annonce la taille du gros fichier mais n'en sert que le
        // premier quart : c'est le cas du journal en rotation, que huit lectures
        // concurrentes rendent bien plus probable qu'une lecture séquentielle.
        self.tronque = filename.contains("tronque");
        async move {
            // Deux chemins réservés pour exercer les échecs, que le reste du
            // mock accepte trop volontiers : « introuvable » échoue à
            // l'ouverture, « coupure » rend un bloc puis casse en pleine
            // lecture — c'est ce second cas qui laissait un fichier tronqué à
            // la place de la cible.
            if filename.contains("introuvable") {
                return Err(StatusCode::NoSuchFile);
            }
            Ok(Handle {
                id,
                handle: "file".into(),
            })
        }
    }

    fn read(
        &mut self,
        id: u32,
        _handle: String,
        offset: u64,
        len: u32,
    ) -> impl Future<Output = Result<russh_sftp::protocol::Data, Self::Error>> + Send {
        let done = std::mem::replace(&mut self.file_read_done, true);
        let coupure = self.coupure_en_lecture;
        let gros = self.gros_fichier;
        let tronque = self.tronque;
        async move {
            if done && coupure {
                // La liaison tombe après le premier bloc.
                return Err(StatusCode::Failure);
            }
            // Le fichier de démonstration honore décalage et longueur : sans
            // cela, un lecteur en bandes parallèles serait « validé » par un
            // serveur qui lui rend toujours le même bloc.
            if gros {
                let debut = usize::try_from(offset).unwrap_or(usize::MAX);
                let servi = if tronque {
                    GROS_FICHIER.len() / 4
                } else {
                    GROS_FICHIER.len()
                };
                if debut >= servi {
                    return Err(StatusCode::Eof);
                }
                let fin = (debut + len as usize).min(servi);
                return Ok(russh_sftp::protocol::Data {
                    id,
                    data: GROS_FICHIER[debut..fin].to_vec(),
                });
            }
            if done {
                // Fin de fichier : sans ce retour, le client relit sans fin.
                return Err(StatusCode::Eof);
            }
            Ok(russh_sftp::protocol::Data {
                id,
                data: b"CONTENU-FICHIER-TEST".to_vec(),
            })
        }
    }

    fn write(
        &mut self,
        id: u32,
        _handle: String,
        _offset: u64,
        data: Vec<u8>,
    ) -> impl Future<Output = Result<Status, Self::Error>> + Send {
        let _n = data.len();
        async move {
            Ok(Status {
                id,
                status_code: StatusCode::Ok,
                error_message: String::new(),
                language_tag: String::new(),
            })
        }
    }

    fn close(
        &mut self,
        id: u32,
        _handle: String,
    ) -> impl Future<Output = Result<Status, Self::Error>> + Send {
        async move {
            Ok(Status {
                id,
                status_code: StatusCode::Ok,
                error_message: String::new(),
                language_tag: String::new(),
            })
        }
    }

    fn realpath(
        &mut self,
        id: u32,
        path: String,
    ) -> impl Future<Output = Result<russh_sftp::protocol::Name, Self::Error>> + Send {
        async move {
            // "." → home absolu, comme un vrai serveur.
            let abs = if path == "." {
                "/home/testuser".to_string()
            } else {
                path
            };
            Ok(russh_sftp::protocol::Name {
                id,
                files: vec![File::dummy(abs)],
            })
        }
    }

    fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _attrs: FileAttributes,
    ) -> impl Future<Output = Result<Status, Self::Error>> + Send {
        async move {
            SFTP_OPS.lock().await.push(format!("mkdir {path}"));
            Ok(ok_status(id))
        }
    }

    fn remove(
        &mut self,
        id: u32,
        filename: String,
    ) -> impl Future<Output = Result<Status, Self::Error>> + Send {
        async move {
            SFTP_OPS.lock().await.push(format!("remove {filename}"));
            Ok(ok_status(id))
        }
    }

    fn rmdir(
        &mut self,
        id: u32,
        path: String,
    ) -> impl Future<Output = Result<Status, Self::Error>> + Send {
        async move {
            // Un dossier « plein » est refuse, comme le ferait OpenSSH.
            if path.ends_with("plein") {
                return Err(StatusCode::Failure);
            }
            SFTP_OPS.lock().await.push(format!("rmdir {path}"));
            Ok(ok_status(id))
        }
    }

    fn rename(
        &mut self,
        id: u32,
        oldpath: String,
        newpath: String,
    ) -> impl Future<Output = Result<Status, Self::Error>> + Send {
        async move {
            SFTP_OPS
                .lock()
                .await
                .push(format!("rename {oldpath} {newpath}"));
            Ok(ok_status(id))
        }
    }

    fn opendir(
        &mut self,
        id: u32,
        _path: String,
    ) -> impl Future<Output = Result<Handle, Self::Error>> + Send {
        async move {
            Ok(Handle {
                id,
                handle: "dir".into(),
            })
        }
    }

    fn readdir(
        &mut self,
        id: u32,
        _handle: String,
    ) -> impl Future<Output = Result<russh_sftp::protocol::Name, Self::Error>> + Send {
        async move {
            if self.root_read_done {
                return Err(StatusCode::Eof);
            }
            self.root_read_done = true;
            Ok(russh_sftp::protocol::Name {
                id,
                files: vec![
                    File {
                        filename: ".".into(),
                        longname: "drwxr-xr-x".into(),
                        attrs: FileAttributes {
                            size: Some(0),
                            permissions: Some(0o40755),
                            ..Default::default()
                        },
                    },
                    File {
                        filename: "rapport.md".into(),
                        longname: "-rw-r--r-- rapport.md".into(),
                        attrs: FileAttributes {
                            size: Some(1234),
                            permissions: Some(0o100_644),
                            ..Default::default()
                        },
                    },
                    File {
                        filename: "data".into(),
                        longname: "drwxr-xr-x data".into(),
                        attrs: FileAttributes {
                            size: Some(4096),
                            permissions: Some(0o40755),
                            ..Default::default()
                        },
                    },
                ],
            })
        }
    }

    fn stat(
        &mut self,
        id: u32,
        path: String,
    ) -> impl Future<Output = Result<russh_sftp::protocol::Attrs, Self::Error>> + Send {
        // Le lecteur en bandes se règle sur la taille annoncée : elle doit être
        // exacte pour le fichier de démonstration.
        let taille = if path.contains("gros") {
            GROS_FICHIER.len() as u64
        } else {
            42
        };
        async move {
            Ok(russh_sftp::protocol::Attrs {
                id,
                attrs: FileAttributes {
                    size: Some(taille),
                    ..Default::default()
                },
            })
        }
    }
}

/// Fichier de démonstration servi par le serveur SFTP de test, à décalage
/// honoré. 400 Kio, soit plus de deux blocs de 64 Kio : le téléchargement passe
/// donc par la lecture en bandes parallèles. Chaque octet dépend de sa position,
/// de sorte qu'un réassemblage erroné ne peut pas passer inaperçu.
static GROS_FICHIER: std::sync::LazyLock<Vec<u8>> =
    std::sync::LazyLock::new(|| (0..400 * 1024u32).map(|i| (i % 251) as u8).collect());

/// Dernier nom d'utilisateur reçu par le serveur de test, en authentification
/// par mot de passe. Un seul serveur à la fois pour les tests qui s'en servent.
static DERNIER_UTILISATEUR: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

// ---------- Harnais de test ----------

/// Clé d'hôte UNIQUE pour tous les serveurs de test.
///
/// Chaque serveur tirait la sienne. Or ils écoutent sur des ports éphémères et
/// partagent le même `known_hosts` (le répertoire personnel virtuel est commun à
/// tout le processus) : quand le système réattribuait à un serveur le port d'un
/// serveur précédent — libéré à la fin de son test —, la clé apprise pour ce port
/// ne correspondait plus, et le client refusait à bon droit une « interception ».
/// Vu en intégration continue, une fois sur quelques dizaines d'exécutions. Une
/// clé partagée rend le port indifférent ; le test de clé changée, lui, écrit son
/// propre leurre.
static CLE_HOTE: std::sync::LazyLock<PrivateKey> = std::sync::LazyLock::new(|| {
    PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap()
});

/// Démarre le serveur SSH de test sur un port libre, retourne le port.
async fn spawn_test_sshd() -> u16 {
    let config = russh::server::Config {
        keys: vec![CLE_HOTE.clone()],
        ..Default::default()
    };
    let config = Arc::new(config);
    let mut server = TestSshServer;
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = server.run_on_socket(config, &listener).await;
    });
    port
}

/// Répertoire personnel virtuel, pour ne pas toucher au `known_hosts` réel.
///
/// `/tmp` était codé en dur, et `HOME` seul n'isole rien sous Windows — où
/// `dirs::home_dir()` interroge le dossier de profil du système. On passe par
/// le répertoire temporaire de la plateforme et l'on pose aussi `AVASH_HOME`,
/// que le cœur honore partout.
fn virtual_home() -> std::path::PathBuf {
    let home = std::env::temp_dir().join(format!("avash-it-home-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    home
}

/// Clé éphémère pour l'auth.
fn temp_key_path() -> std::path::PathBuf {
    let path = virtual_home().join("id_ed25519");
    if !path.exists() {
        let key = PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
        let mut buf = Vec::new();
        russh::keys::encode_pkcs8_pem(&key, &mut buf).unwrap();
        std::fs::write(&path, buf).unwrap();
    }
    path
}

/// `HOME` n'est posé qu'une fois, et toujours sur la même valeur.
///
/// `set_var` porte sur tout le processus : l'appeler depuis chaque test — ils
/// s'exécutent en parallèle — est une mutation concurrente, même quand la
/// valeur ne change pas. Le crate a un verrou pour cela (`testutil`), mais un
/// garde ne se tient pas à travers un `.await` ; poser la variable une seule
/// fois, avant tout test, règle la question sans verrou.
static HOME_POSE: std::sync::LazyLock<()> = std::sync::LazyLock::new(|| {
    let home = virtual_home();
    std::env::set_var("HOME", &home);
    std::env::set_var("AVASH_HOME", &home);
});

fn test_auth() -> avash::ssh::ClientAuth {
    std::sync::LazyLock::force(&HOME_POSE);
    avash::ssh::ClientAuth {
        user: "testuser".into(),
        key_path: Some(temp_key_path()),
        password: None,
    }
}

/// Attend que les compteurs d'un tunnel atteignent l'état visé.
///
/// Les tests dormaient 100 ms avant de lire un instantané : sous charge, les
/// compteurs atomiques n'étaient pas encore à jour et la suite rougissait sans
/// la moindre régression. On attend un état, avec une échéance — et le message
/// d'échec dit ce qu'on attendait.
async fn attendre_compteurs(
    tunnel: &avash::tunnel::Tunnel,
    vise: impl Fn(&avash::tunnel::TunnelSnapshot) -> bool,
    quoi: &str,
) -> avash::tunnel::TunnelSnapshot {
    let echeance = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let snap = tunnel.snapshot();
        if vise(&snap) {
            return snap;
        }
        assert!(
            tokio::time::Instant::now() < echeance,
            "compteurs jamais arrivés à l'état attendu ({quoi}) : {snap:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

// ---------- Les tests ----------

#[tokio::test]
async fn connect_exec_roundtrip() {
    let port = spawn_test_sshd().await;
    let auth = test_auth();
    let mut session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion échouée");
    let (stdout, code) = session.run("uname -a").await.unwrap();
    assert_eq!(code, 0);
    assert!(
        stdout.contains("CMD:uname -a"),
        "stdout inattendu : {stdout}"
    );
    assert!(stdout.contains("stderr-ok"), "stderr manquant : {stdout}");
    session.disconnect().await.unwrap();
}

#[tokio::test]
async fn exec_rapporte_le_code_de_sortie() {
    // Non-regression : run() cassait sur Eof, or exit-status arrive APRES.
    // Le code etait donc toujours 0. Verifie ici sur un code non nul.
    let port = spawn_test_sshd().await;
    let auth = test_auth();
    let mut session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion echouee");
    let (_out, code) = session.run("sh -c exit 42").await.unwrap();
    assert_eq!(code, 42, "le code de sortie non nul doit remonter");

    let (_out, zero) = session.run("true").await.unwrap();
    assert_eq!(zero, 0);
    session.disconnect().await.unwrap();
}

#[tokio::test]
async fn pty_write_and_resize_roundtrip() {
    let port = spawn_test_sshd().await;
    let auth = test_auth();
    let mut session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion échouée");

    let mut pty = session.open_pty(80, 24, "xterm-256color").await.unwrap();

    // Le banner PTY doit arriver
    let first = tokio::time::timeout(std::time::Duration::from_secs(5), pty.out_rx.recv())
        .await
        .expect("timeout banner PTY")
        .expect("canal fermé");
    let banner = String::from_utf8_lossy(&first);
    assert!(
        banner.contains("PTY(xterm-256color 80x24)"),
        "banner : {banner}"
    );

    // Écrire au stdin doit revenir en ECHO:
    pty.in_tx.send(b"bonjour\r".to_vec()).await.unwrap();
    let echoed = tokio::time::timeout(std::time::Duration::from_secs(5), pty.out_rx.recv())
        .await
        .expect("timeout echo")
        .expect("canal fermé");
    let echo = String::from_utf8_lossy(&echoed);
    assert!(echo.contains("ECHO:bonjour"), "echo : {echo}");

    // Resize doit déclencher RESIZED:w×h
    pty.resize_tx.send((120, 40)).await.unwrap();
    let resized = tokio::time::timeout(std::time::Duration::from_secs(5), pty.out_rx.recv())
        .await
        .expect("timeout resize")
        .expect("canal fermé");
    let r = String::from_utf8_lossy(&resized);
    assert!(r.contains("RESIZED:120x40"), "resize : {r}");

    session.disconnect().await.unwrap();
}

#[tokio::test]
async fn sftp_list_download_upload() {
    let port = spawn_test_sshd().await;
    let auth = test_auth();
    let session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion échouée");
    let sftp = avash::sftp::SftpHandle::open(session)
        .await
        .expect("SFTP open");

    // list
    let entries = sftp.list("/").await.unwrap();
    assert!(
        entries.iter().any(|e| e.name == "rapport.md"),
        "entries : {entries:?}"
    );

    // download → fichier local temporaire
    let local = std::env::temp_dir().join(format!("avash-dl-{}.txt", std::process::id()));
    let n = sftp.download("/rapport.md", &local).await.unwrap();
    assert!(n > 0);
    let content = std::fs::read_to_string(&local).unwrap();
    assert_eq!(content, "CONTENU-FICHIER-TEST");

    // upload (le serveur factice accepte tout write)
    let up = std::env::temp_dir().join(format!("avash-up-{}.txt", std::process::id()));
    let payload: &[u8] = b"donnees-locales";
    std::fs::write(&up, payload).unwrap();
    let n = sftp.upload(&up, "/envoye.txt").await.unwrap();
    // Compare a la taille reelle : une constante en dur derive des qu'on
    // touche au contenu (c'etait 14 pour 15 octets).
    assert_eq!(n as usize, payload.len());

    sftp.close().await.unwrap();
}

/// Non-regression : une cle d'hote qui CHANGE doit faire echouer la connexion.
///
/// C'est le scenario d'interception (MITM) : l'hote est deja connu, mais la cle
/// presentee ne correspond plus. OpenSSH refuse et affiche
/// REMOTE HOST IDENTIFICATION HAS CHANGED ; avash doit faire de meme.
///
/// Regression corrigee : le `match` sur `check_known_hosts` confondait
/// "hote inconnu" (Ok(false)) et "cle changee" (Err(KeyChanged)) dans un bras
/// `_` commun, et reapprenait la cle dans les deux cas.
#[tokio::test]
async fn changed_host_key_is_refused() {
    let port = spawn_test_sshd().await;
    let auth = test_auth(); // positionne HOME sur le home virtuel

    // On inscrit volontairement une cle qui n'est PAS celle du serveur.
    let decoy = PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
    let decoy_pub = decoy.public_key().clone();
    // On vise EXPLICITEMENT le fichier que le code consultera. La résolution
    // implicite de russh passe par `std::env::home_dir()`, qui sous Windows
    // consulte `USERPROFILE` et ignore notre home de test : le leurre atterrissait
    // dans le vrai profil, la vérification ne le voyait pas, et la connexion
    // était acceptée comme un premier contact.
    let known_hosts = avash::ssh::chemin_known_hosts().expect("chemin known_hosts");
    russh::keys::known_hosts::learn_known_hosts_path("127.0.0.1", port, &decoy_pub, &known_hosts)
        .expect("ecriture known_hosts");

    // Le serveur presente sa vraie cle : elle differe de celle memorisee.
    let res = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth).await;
    let err = res
        .err()
        .expect("une cle d'hote modifiee doit etre refusee");

    // Le leurre ne doit pas survivre au test : le port sera réattribué à un
    // autre serveur de test, qui porte la clé commune — il passerait alors pour
    // une interception, dans un test qui n'a rien à voir.
    let _ = avash::ssh::forget_host_key_at("127.0.0.1", port, &known_hosts);

    // Le message doit etre exploitable tel quel dans l'interface : un
    // "Unknown key" opaque ne dit pas a l'utilisateur ce qui se passe ni quoi
    // faire. Il part sinon sur stderr, que personne ne lit dans une GUI.
    let msg = format!("{err:#}");
    assert!(
        msg.contains("CLÉ D'HÔTE A CHANGÉ"),
        "le message doit nommer le probleme : {msg}"
    );
    assert!(
        msg.contains("known_hosts"),
        "le message doit dire comment s'en sortir : {msg}"
    );
}

/// TOFU : un hote inconnu est appris au premier contact, la connexion passe.
#[tokio::test]
async fn unknown_host_is_learned_on_first_contact() {
    let port = spawn_test_sshd().await;
    let auth = test_auth();

    let session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("premier contact : la connexion doit passer (TOFU)");
    drop(session);

    // La clé doit désormais être mémorisée. Le vérifier DANS LE FICHIER : se
    // contenter d'une seconde connexion réussie ne prouve rien, puisqu'un
    // second « premier contact » passerait tout aussi bien.
    let known_hosts = avash::ssh::chemin_known_hosts().expect("chemin known_hosts");
    let apprises =
        russh::keys::known_hosts::known_host_keys_path("127.0.0.1", port, &known_hosts).unwrap();
    assert_eq!(apprises.len(), 1, "la clé d'hôte n'a pas été mémorisée");

    // Et la reconnexion passe, cette fois en « hôte connu ».
    avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("reconnexion sur hote connu : doit passer");
}

/// Non-regression : abandonner le canal de resize ne doit ni tuer la session,
/// ni faire tourner le pump a vide.
///
/// Regression corrigee : le bras `resize_rx.recv()` du select! traitait le
/// None par un no-op. Un canal ferme rendant Ready(None) immediatement et sans
/// fin, la boucle tournait a 100 % de CPU tant que la session vivait.
#[tokio::test]
async fn dropping_resize_channel_keeps_pty_alive_and_idle() {
    let port = spawn_test_sshd().await;
    let auth = test_auth();
    let mut session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion echouee");

    let mut pty = session.open_pty(80, 24, "xterm-256color").await.unwrap();

    // Banner initial
    tokio::time::timeout(std::time::Duration::from_secs(5), pty.out_rx.recv())
        .await
        .expect("timeout banner")
        .expect("canal ferme");

    // On abandonne le resize : le front peut le lacher sans fermer l'onglet.
    let resize = std::mem::replace(&mut pty.resize_tx, {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        tx
    });
    drop(resize);

    // La session doit rester utilisable : le clavier passe toujours.
    pty.in_tx
        .send(b"bonjour".to_vec())
        .await
        .expect("stdin ferme");
    let echoed = tokio::time::timeout(std::time::Duration::from_secs(5), pty.out_rx.recv())
        .await
        .expect("timeout apres abandon du resize")
        .expect("canal ferme apres abandon du resize");
    assert!(
        String::from_utf8_lossy(&echoed).contains("bonjour"),
        "le PTY doit rester vivant : {:?}",
        String::from_utf8_lossy(&echoed)
    );
}

// ---------- Tunnels ----------

use avash::tunnel::{Tunnel, TunnelDef, TunnelKind};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn connect_for_tunnel(port: u16) -> avash::ssh::AvashSession {
    avash::ssh::AvashSession::connect("127.0.0.1", port, &test_auth())
        .await
        .expect("connexion")
}

#[tokio::test]
async fn tunnel_local_relaie_les_octets_dans_les_deux_sens() {
    let port = spawn_test_sshd().await;
    let session = connect_for_tunnel(port).await;
    // Port 0 : on laisse l'OS choisir, puis on lit le port lie.
    let mut def = TunnelDef::new("test", TunnelKind::Local, 1, "db.interne", 5432, "");
    def.bind_port = 0;
    // validate() refuse 0 pour un humain ; ici on contourne pour le test en
    // ouvrant sur un port libre trouve nous-memes.
    let free = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    def.bind_port = free.local_addr().unwrap().port();
    drop(free);
    let tunnel = Tunnel::open(session, def)
        .await
        .expect("ouverture tunnel -L");
    assert_eq!(tunnel.bound_port(), tunnel.def().bind_port);

    let mut client = tokio::net::TcpStream::connect(("127.0.0.1", tunnel.bound_port()))
        .await
        .expect("le port local doit accepter");
    client.write_all(b"ping").await.unwrap();
    let mut buf = vec![0u8; 64];
    let n = tokio::time::timeout(std::time::Duration::from_secs(3), client.read(&mut buf))
        .await
        .expect("reponse dans les temps")
        .unwrap();
    assert_eq!(
        &buf[..n],
        b"ECHO:ping",
        "les octets doivent traverser le tunnel"
    );
    drop(client);

    // Les compteurs refletent la connexion.
    let snap = attendre_compteurs(
        &tunnel,
        |s| s.total == 1 && s.active == 0 && s.bytes_up == 4,
        "une connexion terminée, 4 octets montants",
    )
    .await;
    assert!(snap.alive);
    assert_eq!(snap.total, 1);
    assert_eq!(snap.active, 0, "connexion terminee");
    assert_eq!(snap.bytes_up, 4);
    assert_eq!(snap.bytes_down, "ECHO:ping".len() as u64);

    tunnel.close().await;
}

#[tokio::test]
async fn tunnel_local_signale_une_destination_injoignable() {
    let port = spawn_test_sshd().await;
    let session = connect_for_tunnel(port).await;
    let free = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let bind = free.local_addr().unwrap().port();
    drop(free);
    let def = TunnelDef::new("test", TunnelKind::Local, bind, "injoignable", 1, "");
    let tunnel = Tunnel::open(session, def).await.unwrap();

    let mut client = tokio::net::TcpStream::connect(("127.0.0.1", bind))
        .await
        .unwrap();
    // Le serveur refuse le canal : notre cote ferme la connexion locale.
    let mut buf = [0u8; 8];
    let n = tokio::time::timeout(std::time::Duration::from_secs(3), client.read(&mut buf))
        .await
        .expect("fermeture dans les temps")
        .unwrap();
    assert_eq!(n, 0, "connexion fermee sans donnees");
    let snap = attendre_compteurs(
        &tunnel,
        |s| s.last_error.is_some(),
        "une erreur de destination injoignable",
    )
    .await;
    assert!(snap.alive, "le tunnel lui-meme reste debout");
    assert!(
        snap.last_error
            .as_deref()
            .unwrap_or("")
            .contains("injoignable:1"),
        "l'erreur doit nommer la destination : {:?}",
        snap.last_error
    );
    tunnel.close().await;
}

#[tokio::test]
async fn tunnel_dynamique_negocie_socks5_puis_relaie() {
    let port = spawn_test_sshd().await;
    let session = connect_for_tunnel(port).await;
    let free = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let bind = free.local_addr().unwrap().port();
    drop(free);
    let def = TunnelDef::new("test", TunnelKind::Dynamic, bind, "", 0, "");
    let tunnel = Tunnel::open(session, def).await.unwrap();

    let mut c = tokio::net::TcpStream::connect(("127.0.0.1", bind))
        .await
        .unwrap();
    c.write_all(&[5, 1, 0]).await.unwrap();
    let mut rep = [0u8; 2];
    c.read_exact(&mut rep).await.unwrap();
    assert_eq!(rep, [5, 0]);
    let mut req = vec![5, 1, 0, 3, 9];
    req.extend_from_slice(b"intranet.");
    req.extend_from_slice(&80u16.to_be_bytes());
    c.write_all(&req).await.unwrap();
    let mut ok = [0u8; 10];
    c.read_exact(&mut ok).await.unwrap();
    assert_eq!(ok[1], 0, "CONNECT accepte");

    c.write_all(b"GET /").await.unwrap();
    let mut buf = vec![0u8; 64];
    let n = tokio::time::timeout(std::time::Duration::from_secs(3), c.read(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buf[..n], b"ECHO:GET /");
    tunnel.close().await;
}

#[tokio::test]
async fn tunnel_distant_relaie_vers_un_service_local() {
    // Service local que le serveur doit atteindre a travers nous.
    let local = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let local_port = local.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (mut s, _) = local.accept().await.unwrap();
        let mut buf = [0u8; 16];
        let n = s.read(&mut buf).await.unwrap();
        let up = String::from_utf8_lossy(&buf[..n]).to_uppercase();
        s.write_all(up.as_bytes()).await.unwrap();
    });

    let port = spawn_test_sshd().await;
    let session = connect_for_tunnel(port).await;
    let def = TunnelDef::new(
        "test",
        TunnelKind::Remote,
        40_000,
        "127.0.0.1",
        local_port,
        "",
    );
    let tunnel = Tunnel::open(session, def).await.expect("ouverture -R");
    assert_eq!(tunnel.bound_port(), 40_000);

    // Le serveur de test ouvre le canal de lui-meme et attend la reponse.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if let Some(reply) = REMOTE_REPLY.lock().await.first().cloned() {
            assert_eq!(
                reply, b"HELLO",
                "la reponse du service local doit revenir au serveur"
            );
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "aucune reponse recue via -R"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let snap = attendre_compteurs(
        &tunnel,
        |s| s.total == 1 && s.bytes_down == 5 && s.bytes_up == 5,
        "un aller-retour de 5 octets dans chaque sens",
    )
    .await;
    assert_eq!(snap.total, 1);
    assert_eq!(snap.bytes_down, 5, "« hello » vers le service local");
    assert_eq!(snap.bytes_up, 5, "« HELLO » vers le serveur");
    tunnel.close().await;
}

// ---------- SFTP : dossiers, renommage, suppression, progression ----------

#[tokio::test]
async fn sftp_mkdir_rename_remove_atteignent_le_serveur() {
    let port = spawn_test_sshd().await;
    let session = connect_for_tunnel(port).await;
    let sftp = avash::sftp::SftpHandle::open(session).await.unwrap();
    sftp.mkdir("/srv/nouveau").await.unwrap();
    sftp.rename("/srv/a.txt", "/srv/b.txt").await.unwrap();
    sftp.remove("/srv/b.txt", false).await.unwrap();
    sftp.remove("/srv/vide", true).await.unwrap();
    let err = sftp.remove("/srv/plein", true).await.unwrap_err();
    assert!(
        err.to_string().contains("doit être vide"),
        "le message doit expliquer pourquoi : {err:#}"
    );
    let ops = SFTP_OPS.lock().await.clone();
    for expected in [
        "mkdir /srv/nouveau",
        "rename /srv/a.txt /srv/b.txt",
        "remove /srv/b.txt",
        "rmdir /srv/vide",
    ] {
        assert!(
            ops.iter().any(|o| o == expected),
            "{expected} absent de {ops:?}"
        );
    }
    sftp.close().await.unwrap();
}

#[tokio::test]
async fn sftp_realpath_resout_le_point_en_chemin_absolu() {
    let port = spawn_test_sshd().await;
    let session = connect_for_tunnel(port).await;
    let sftp = avash::sftp::SftpHandle::open(session).await.unwrap();
    assert_eq!(sftp.realpath(".").await, "/home/testuser");
    // Un chemin deja absolu revient tel quel.
    assert_eq!(sftp.realpath("/srv").await, "/srv");
    sftp.close().await.unwrap();
}

#[tokio::test]
async fn sftp_download_rapporte_sa_progression() {
    let port = spawn_test_sshd().await;
    let session = connect_for_tunnel(port).await;
    let sftp = avash::sftp::SftpHandle::open(session).await.unwrap();
    let local = std::env::temp_dir().join(format!("avash-progress-{}.bin", std::process::id()));
    let mut seen = Vec::new();
    let n = sftp
        .download_with("/srv/fichier.txt", &local, |done, _total| seen.push(done))
        .await
        .unwrap();
    assert_eq!(n, "CONTENU-FICHIER-TEST".len() as u64);
    assert_eq!(
        seen.last().copied(),
        Some(n),
        "la derniere progression = total transfere"
    );
    assert_eq!(std::fs::read(&local).unwrap(), b"CONTENU-FICHIER-TEST");
    let _ = std::fs::remove_file(&local);
    sftp.close().await.unwrap();
}

/// La cible ne doit pas être touchée tant que le transfert n'a pas abouti.
///
/// `File::create` tronquait d'emblée : un double-clic sur un fichier déjà
/// présent dans ~/Téléchargements l'écrasait, et une coupure laissait à sa
/// place un fichier tronqué portant le bon nom. On vérifie ici qu'un
/// téléchargement voué à l'échec — chemin distant inexistant — laisse le
/// fichier local intact et ne sème pas de `.part`.
#[tokio::test]
async fn un_telechargement_qui_echoue_ne_touche_pas_le_fichier_local() {
    let port = spawn_test_sshd().await;
    let session = connect_for_tunnel(port).await;
    let sftp = avash::sftp::SftpHandle::open(session).await.unwrap();
    let local = std::env::temp_dir().join(format!("avash-intact-{}.bin", std::process::id()));
    std::fs::write(&local, b"PRECIEUX").unwrap();

    // Coupure APRÈS le premier bloc : c'est ce cas-là qui laissait un fichier
    // tronqué portant le bon nom. Un échec à l'ouverture, lui, n'a jamais rien
    // écrit — le tester ne prouverait rien.
    let echec = sftp
        .download_with("/srv/coupure.bin", &local, |_, _| {})
        .await;

    assert!(echec.is_err(), "le téléchargement aurait dû échouer");
    assert_eq!(
        std::fs::read(&local).unwrap(),
        b"PRECIEUX",
        "le fichier local a été touché alors que le transfert a échoué"
    );
    let partiel = local.with_extension("bin.part");
    assert!(
        !partiel.exists(),
        "un .part orphelin est resté : {}",
        partiel.display()
    );

    let _ = std::fs::remove_file(&local);
    sftp.close().await.unwrap();
}

/// Le téléchargement en bandes parallèles doit rendre EXACTEMENT le fichier.
///
/// `File` de russh-sftp n'émet qu'une requête de lecture à la fois : le débit
/// descendant plafonnait à un bloc par aller-retour, huit fois moins que la
/// montée, déjà pipelinée. On lit désormais par bandes, à décalages distincts —
/// ce qui n'a de valeur que si le réassemblage est juste. Le serveur de test
/// honore décalage et longueur, et chaque octet du fichier dépend de sa
/// position : une bande mal placée se verrait immédiatement.
#[tokio::test]
async fn un_telechargement_en_bandes_rend_le_fichier_a_l_octet_pres() {
    let port = spawn_test_sshd().await;
    let session = connect_for_tunnel(port).await;
    let sftp = avash::sftp::SftpHandle::open(session).await.unwrap();
    let local = std::env::temp_dir().join(format!("avash-bandes-{}.bin", std::process::id()));

    let mut progression = Vec::new();
    let n = sftp
        .download_with("/srv/gros.bin", &local, |fait, total| {
            progression.push((fait, total));
        })
        .await
        .unwrap();

    let attendu: &[u8] = &GROS_FICHIER;
    assert_eq!(n as usize, attendu.len(), "taille annoncée");
    assert_eq!(
        std::fs::read(&local).unwrap(),
        attendu,
        "le réassemblage des bandes ne rend pas le fichier d'origine"
    );
    // La progression reste croissante et finit sur le total, malgré des bandes
    // qui avancent en parallèle.
    assert_eq!(progression.last().map(|(f, _)| *f), Some(n));
    assert!(
        progression.windows(2).all(|p| p[0].0 <= p[1].0),
        "progression non monotone"
    );

    let _ = std::fs::remove_file(&local);
    sftp.close().await.unwrap();
}

/// Sans mot de passe, un refus doit porter le marqueur que l'interface guette.
///
/// C'est lui qui déclenche la demande de saisie puis la nouvelle tentative :
/// sans marqueur, l'utilisateur voit un échec sec et sans recours. Le serveur
/// de test acceptait jusqu'ici n'importe qui, si bien qu'aucun test Rust ne
/// produisait ce marqueur.
#[tokio::test]
async fn un_refus_sans_mot_de_passe_porte_le_marqueur_attendu() {
    let port = spawn_test_sshd().await;
    let _home = virtual_home();
    let mut auth = test_auth();
    auth.user = "refuse".into();
    auth.password = None;

    let issue = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth).await;
    let Err(e) = issue else {
        panic!("un refus doit remonter")
    };
    let e = e.to_string();
    assert!(
        e.contains(avash::ssh::PASSWORD_REQUIRED),
        "l'interface ne saura pas qu'il faut demander un mot de passe : {e}"
    );
}

/// Avec un mauvais mot de passe, le message doit dire l'échec — et surtout PAS
/// porter le marqueur, sans quoi l'interface redemanderait indéfiniment.
#[tokio::test]
async fn un_mauvais_mot_de_passe_ne_porte_pas_le_marqueur() {
    let port = spawn_test_sshd().await;
    let _home = virtual_home();
    let mut auth = test_auth();
    auth.user = "refuse".into();
    auth.password = Some("mauvais".into());

    let issue = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth).await;
    let Err(e) = issue else {
        panic!("un mauvais mot de passe doit remonter")
    };
    let e = e.to_string();
    assert!(
        !e.contains(avash::ssh::PASSWORD_REQUIRED),
        "marqueur en trop : {e}"
    );
    assert!(
        e.contains("Authentification échouée"),
        "message inattendu : {e}"
    );
    // Le message doit dire ce que le serveur accepte encore : sans cela,
    // l'utilisateur ne peut pas distinguer « mauvais mot de passe » de
    // « cette méthode n'est pas proposée », qui appellent des gestes opposés.
    assert!(
        e.contains("Le serveur propose encore"),
        "l'échec ne nomme pas les méthodes restantes : {e}"
    );
}

/// Le bon mot de passe passe : sans ce cas, les deux tests ci-dessus
/// pourraient passer sur un serveur qui refuse tout, quoi qu'on lui envoie.
#[tokio::test]
async fn le_bon_mot_de_passe_est_accepte() {
    let port = spawn_test_sshd().await;
    let _home = virtual_home();
    let mut auth = test_auth();
    auth.user = "refuse".into();
    auth.password = Some("le-bon".into());

    assert!(avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .is_ok());
}

/// Un serveur qui annonce plus qu'il ne sert ne doit pas produire un « succès ».
///
/// Les bandes écrivent à des décalages disjoints : si les dernières tombent sur
/// une fin de fichier prématurée, le `.part` contient les premières données,
/// **des zéros au milieu**, et se voyait promu sur la cible, transfert annoncé
/// réussi. Le chemin séquentiel, lui, ne pouvait que tronquer — jamais trouer.
#[tokio::test]
async fn un_fichier_plus_court_que_promis_ne_passe_pas_pour_un_succes() {
    let port = spawn_test_sshd().await;
    let session = connect_for_tunnel(port).await;
    let sftp = avash::sftp::SftpHandle::open(session).await.unwrap();
    let local = std::env::temp_dir().join(format!("avash-troue-{}.bin", std::process::id()));
    let _ = std::fs::remove_file(&local);

    let issue = sftp
        .download_with("/srv/gros-tronque.bin", &local, |_, _| {})
        .await;

    let Err(e) = issue else {
        panic!("un fichier incomplet ne doit pas être un succès")
    };
    assert!(
        e.to_string().contains("Transfert incomplet"),
        "message inattendu : {e}"
    );
    assert!(
        !local.exists(),
        "la cible ne doit pas exister : {}",
        local.display()
    );
    let partiel = local.with_extension("bin.part");
    assert!(!partiel.exists(), "un .part orphelin est resté");

    sftp.close().await.unwrap();
}

/// Un compte de domaine `DOMAINE\utilisateur` doit arriver INTACT au serveur.
///
/// C'est la forme qu'impose un hôte Linux joint à un annuaire Active Directory.
/// La contre-oblique traverse la saisie, l'IPC de Tauri (donc du JSON, où elle
/// s'échappe) et la requête d'authentification SSH : si l'une de ces étapes la
/// mangeait ou la doublait, le serveur verrait un autre compte et refuserait,
/// sans que rien n'indique pourquoi.
#[tokio::test]
async fn un_compte_de_domaine_arrive_intact_au_serveur() {
    let port = spawn_test_sshd().await;
    let _home = virtual_home();
    *DERNIER_UTILISATEUR.lock().unwrap() = None;

    let auth = avash::ssh::ClientAuth {
        user: "TEST\\Adrien".into(),
        key_path: None,
        password: Some("secret".into()),
    };
    let session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion");

    assert_eq!(
        DERNIER_UTILISATEUR.lock().unwrap().as_deref(),
        Some("TEST\\Adrien"),
        "le nom de domaine n'est pas arrivé intact"
    );
    session.disconnect().await.unwrap();
}

/// Un serveur qui n'accepte QUE `keyboard-interactive` doit être joignable.
///
/// C'est la configuration courante d'un hôte Linux joint à un annuaire :
/// `PasswordAuthentication` désactivé, la conversation confiée à PAM. OpenSSH
/// bascule tout seul ; Avash ne savait pas, et rendait « authentification
/// échouée » avec un mot de passe pourtant juste.
#[tokio::test]
async fn un_serveur_qui_n_accepte_que_pam_est_joignable() {
    let port = spawn_test_sshd().await;
    let _home = virtual_home();
    let auth = avash::ssh::ClientAuth {
        user: "pam-seul".into(),
        key_path: None,
        password: Some("le-bon".into()),
    };
    let session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("un serveur en keyboard-interactive doit être joignable");
    session.disconnect().await.unwrap();
}

/// Plusieurs invites masquées d'affilée : chacune reçoit le mot de passe.
#[tokio::test]
async fn plusieurs_invites_masquees_sont_toutes_honorees() {
    let port = spawn_test_sshd().await;
    let _home = virtual_home();
    let auth = avash::ssh::ClientAuth {
        user: "pam-double".into(),
        key_path: None,
        password: Some("le-bon".into()),
    };
    assert!(avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .is_ok());
}

/// Une invite EN CLAIR n'est pas un mot de passe — code à usage unique,
/// question de sécurité. Y envoyer le mot de passe le livrerait à l'écran du
/// serveur sans aboutir. On renonce en nommant ce qui était demandé.
#[tokio::test]
async fn une_invite_en_clair_n_est_pas_remplie_avec_le_mot_de_passe() {
    let port = spawn_test_sshd().await;
    let _home = virtual_home();
    let auth = avash::ssh::ClientAuth {
        user: "pam-otp".into(),
        key_path: None,
        password: Some("le-bon".into()),
    };
    let issue = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth).await;
    let Err(e) = issue else {
        panic!("une invite en clair ne doit pas être remplie à l'aveugle")
    };
    let msg = e.to_string();
    assert!(
        msg.contains("Code à usage unique"),
        "l'invite doit être citée : {msg}"
    );
    assert!(
        !msg.contains("le-bon"),
        "le mot de passe ne doit pas fuiter dans le message"
    );
}

// ---------- ProxyJump ----------

#[tokio::test]
async fn proxy_jump_connecte_la_cible_a_travers_un_rebond() {
    // Deux serveurs : un rebond (jump) et la cible. On se connecte a la cible
    // UNIQUEMENT via le rebond, comme `ssh -J jump cible`.
    let jump_port = spawn_test_sshd().await;
    let target_port = spawn_test_sshd().await;
    let hop = avash::ssh::Hop {
        addr: "127.0.0.1".into(),
        port: jump_port,
        auth: test_auth(),
    };
    let mut session =
        avash::ssh::AvashSession::connect_via(&[hop], "127.0.0.1", target_port, &test_auth())
            .await
            .expect("connexion via rebond");
    // On atteint bien la cible : elle repond a l'exec.
    let (out, code) = session.run("echo via-jump").await.unwrap();
    assert!(
        out.contains("CMD:echo via-jump"),
        "réponse de la cible : {out:?}"
    );
    assert_eq!(code, 0);
    session.disconnect().await.unwrap();
}

#[tokio::test]
async fn proxy_jump_a_deux_rebonds() {
    // Chaine de deux rebonds avant la cible.
    let j1 = spawn_test_sshd().await;
    let j2 = spawn_test_sshd().await;
    let target = spawn_test_sshd().await;
    let hops = vec![
        avash::ssh::Hop {
            addr: "127.0.0.1".into(),
            port: j1,
            auth: test_auth(),
        },
        avash::ssh::Hop {
            addr: "127.0.0.1".into(),
            port: j2,
            auth: test_auth(),
        },
    ];
    let mut session =
        avash::ssh::AvashSession::connect_via(&hops, "127.0.0.1", target, &test_auth())
            .await
            .expect("connexion via 2 rebonds");
    let (out, _) = session.run("echo deux-rebonds").await.unwrap();
    assert!(out.contains("CMD:echo deux-rebonds"), "{out:?}");
    session.disconnect().await.unwrap();
}

#[tokio::test]
async fn connect_via_sans_rebond_equivaut_a_connect() {
    let port = spawn_test_sshd().await;
    let mut session = avash::ssh::AvashSession::connect_via(&[], "127.0.0.1", port, &test_auth())
        .await
        .expect("connexion directe via liste vide");
    let (out, _) = session.run("echo direct").await.unwrap();
    assert!(out.contains("CMD:echo direct"), "{out:?}");
    session.disconnect().await.unwrap();
}

// ---------- known_hosts : oubli d'une clé ----------

#[tokio::test]
async fn forget_host_key_retire_la_cle_apprise() {
    // Fichier known_hosts dedie : aucune dependance a HOME, donc aucune course
    // avec les autres tests (HOME est global au processus).
    let path = std::env::temp_dir().join(format!(
        "avash-kh-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_file(&path);
    let key =
        russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
    // Apprend une cle pour un hote fictif dans CE fichier.
    russh::keys::known_hosts::learn_known_hosts_path("10.9.8.7", 2222, key.public_key(), &path)
        .unwrap();

    let before = russh::keys::known_hosts::known_host_keys_path("10.9.8.7", 2222, &path).unwrap();
    assert_eq!(before.len(), 1, "cle apprise");

    let removed = avash::ssh::forget_host_key_at("10.9.8.7", 2222, &path).unwrap();
    assert_eq!(removed, 1);
    let after = russh::keys::known_hosts::known_host_keys_path("10.9.8.7", 2222, &path).unwrap();
    assert!(after.is_empty(), "cle oubliee");

    // Oublier une cle absente ne casse rien.
    assert_eq!(
        avash::ssh::forget_host_key_at("10.9.8.7", 2222, &path).unwrap(),
        0
    );
    let _ = std::fs::remove_file(&path);
}
