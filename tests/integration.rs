//! Tests d'intégration avash : serveur SSH+SFTP embarqué (russh server),
//! client avash réel dessus. Valide connect/auth/exec/PTY/SFTP bout-en-bout.

use async_trait::async_trait;
use russh::keys::key::KeyPair;
use russh::server::{Auth, Msg, Server as _, Session};
use russh::{Channel, ChannelId};
use russh_sftp::protocol::{File, FileAttributes, Handle, StatusCode, Status};
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use tokio::sync::Mutex;

// ---------- Serveur SSH de test ----------

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

#[async_trait]
impl russh::server::Handler for TestSshSession {
    type Error = anyhow::Error;

    async fn auth_publickey(
        &mut self,
        _user: &str,
        _key: &russh_keys::key::PublicKey,
    ) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        self.channels.lock().await.insert(channel.id(), channel);
        Ok(true)
    }

    async fn exec_request(
        &mut self,
        channel_id: ChannelId,
        request: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let channel = self.channels.lock().await.remove(&channel_id).unwrap();
        session.channel_success(channel_id);
        let output = format!("CMD:{}\r\n", String::from_utf8_lossy(request));
        session.data(channel_id, output.into_bytes().into());
        session.extended_data(channel_id, 1, b"stderr-ok".to_vec().into());
        session.exit_status_request(channel_id, 0);
        session.eof(channel_id);
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
        session.channel_success(channel_id);
        let banner = format!("\r\nPTY({term} {col_width}x{row_height})\r\n");
        session.data(channel_id, banner.into_bytes().into());
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel_id: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel_id);
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
        let echo = format!("ECHO:{}", String::from_utf8_lossy(data));
        session.data(channel_id, echo.into_bytes().into());
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
        session.data(channel_id, msg.into_bytes().into());
        Ok(())
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
            session.channel_success(channel_id);
            let sftp = TestSftpSession::default();
            tokio::spawn(async move {
                russh_sftp::server::run(channel.into_stream(), sftp).await;
            });
        } else {
            session.channel_failure(channel_id);
        }
        Ok(())
    }
}

// ---------- Système de fichiers SFTP factice en mémoire ----------

#[derive(Default)]
struct TestSftpSession {
    root_read_done: bool,
    /// Octets deja servis par read() : sans cet etat, le serveur renvoie le
    /// contenu indefiniment et le client telecharge en boucle infinie.
    file_read_done: bool,
}

impl russh_sftp::server::Handler for TestSftpSession {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    fn open(
        &mut self,
        id: u32,
        _filename: String,
        _pflags: russh_sftp::protocol::OpenFlags,
        _attrs: FileAttributes,
    ) -> impl Future<Output = Result<Handle, Self::Error>> + Send {
        async move { Ok(Handle { id, handle: "file".into() }) }
    }

    fn read(
        &mut self,
        id: u32,
        _handle: String,
        _offset: u64,
        _len: u32,
    ) -> impl Future<Output = Result<russh_sftp::protocol::Data, Self::Error>> + Send {
        let done = std::mem::replace(&mut self.file_read_done, true);
        async move {
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
            Ok(Status { id, status_code: StatusCode::Ok, error_message: "".into(), language_tag: "".into() })
        }
    }

    fn close(
        &mut self,
        id: u32,
        _handle: String,
    ) -> impl Future<Output = Result<Status, Self::Error>> + Send {
        async move {
            Ok(Status { id, status_code: StatusCode::Ok, error_message: "".into(), language_tag: "".into() })
        }
    }

    fn opendir(
        &mut self,
        id: u32,
        _path: String,
    ) -> impl Future<Output = Result<Handle, Self::Error>> + Send {
        async move { Ok(Handle { id, handle: "dir".into() }) }
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
                    File { filename: ".".into(), longname: "drwxr-xr-x".into(), attrs: FileAttributes { size: Some(0), permissions: Some(0o40755), ..Default::default() } },
                    File { filename: "rapport.md".into(), longname: "-rw-r--r-- rapport.md".into(), attrs: FileAttributes { size: Some(1234), permissions: Some(0o100644), ..Default::default() } },
                    File { filename: "data".into(), longname: "drwxr-xr-x data".into(), attrs: FileAttributes { size: Some(4096), permissions: Some(0o40755), ..Default::default() } },
                ],
            })
        }
    }

    fn stat(
        &mut self,
        id: u32,
        _path: String,
    ) -> impl Future<Output = Result<russh_sftp::protocol::Attrs, Self::Error>> + Send {
        async move {
            Ok(russh_sftp::protocol::Attrs {
                id,
                attrs: FileAttributes { size: Some(42), ..Default::default() },
            })
        }
    }
}

// ---------- Harnais de test ----------

/// Démarre le serveur SSH de test sur un port libre, retourne le port.
async fn spawn_test_sshd() -> u16 {
    let config = russh::server::Config {
        keys: vec![KeyPair::generate_ed25519().unwrap()],
        ..Default::default()
    };
    let config = Arc::new(config);
    let mut server = TestSshServer;
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = server.run_on_socket(config, &listener).await;
    });
    port
}

/// HOME virtuel pour ne pas toucher au known_hosts réel (TOFU du client).
fn virtual_home() -> String {
    let home = format!("/tmp/avash-it-home-{}", std::process::id());
    std::fs::create_dir_all(&home).unwrap();
    home
}

/// Clé éphémère pour l'auth.
fn temp_key_path() -> std::path::PathBuf {
    let home = virtual_home();
    let path = std::path::PathBuf::from(home).join("id_ed25519");
    if !path.exists() {
        let key = KeyPair::generate_ed25519().unwrap();
        let mut buf = Vec::new();
        russh_keys::encode_pkcs8_pem(&key, &mut buf).unwrap();
        std::fs::write(&path, buf).unwrap();
    }
    path
}

fn test_auth() -> avash::ssh::ClientAuth {
    std::env::set_var("HOME", virtual_home());
    avash::ssh::ClientAuth {
        user: "testuser".into(),
        key_path: Some(temp_key_path()),
        password: None,
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
    assert!(stdout.contains("CMD:uname -a"), "stdout inattendu : {stdout}");
    assert!(stdout.contains("stderr-ok"), "stderr manquant : {stdout}");
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
    assert!(banner.contains("PTY(xterm-256color 80x24)"), "banner : {banner}");

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
    let sftp = avash::sftp::SftpHandle::open(session).await.expect("SFTP open");

    // list
    let entries = sftp.list("/").await.unwrap();
    assert!(entries.iter().any(|e| e.name == "rapport.md"), "entries : {entries:?}");

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
/// Regression corrigee : le `match` sur check_known_hosts confondait
/// "hote inconnu" (Ok(false)) et "cle changee" (Err(KeyChanged)) dans un bras
/// `_` commun, et reapprenait la cle dans les deux cas.
#[tokio::test]
async fn changed_host_key_is_refused() {
    let port = spawn_test_sshd().await;
    let auth = test_auth(); // positionne HOME sur le home virtuel

    // On inscrit volontairement une cle qui n'est PAS celle du serveur.
    let decoy = KeyPair::generate_ed25519().unwrap();
    let decoy_pub = decoy.clone_public_key().unwrap();
    russh_keys::learn_known_hosts("127.0.0.1", port, &decoy_pub)
        .expect("ecriture known_hosts");

    // Le serveur presente sa vraie cle : elle differe de celle memorisee.
    let res = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth).await;
    assert!(
        res.is_err(),
        "une cle d'hote modifiee doit etre refusee, la connexion a reussi"
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

    // La cle doit desormais etre memorisee : une reconnexion passe aussi.
    avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("reconnexion sur hote connu : doit passer");
}
