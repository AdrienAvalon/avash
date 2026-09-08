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

/// État partagé des tests d'inondation (marqueurs « INONDE » / « GOUTTE »).
///
/// Le serveur émet des données sans fin sur le canal exec ; ces compteurs, lus
/// côté test, prouvent que le client borne bien le FLUX et pas seulement
/// l'attente : au plafond (`run`) comme à l'échéance (`run_borne`), il ferme le
/// canal, ce que `channel_close` constate, et le flux se tarit alors.
#[derive(Clone, Default)]
struct EtatInondation {
    /// Octets remis à `handle.data` par la tâche d'inondation.
    octets: Arc<std::sync::atomic::AtomicU64>,
    /// Levé par `channel_close` : preuve que le client a envoyé `CHANNEL_CLOSE`,
    /// et signal d'arrêt pour la tâche d'inondation.
    close_recu: Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Clone, Default)]
struct TestSshServer {
    /// Connexions TCP acceptées par CETTE instance : de quoi prouver qu'une
    /// opération n'a pas rouvert de session derrière le dos du test.
    connexions: Arc<std::sync::atomic::AtomicUsize>,
    /// Partagé avec chaque session ouverte, pour les tests de bornage du flux.
    inondation: EtatInondation,
    /// Dernier nom d'utilisateur reçu par CETTE instance (auth par mot de passe
    /// ou clavier). Porté par serveur — non plus par un global partagé — pour
    /// qu'un test ne lise pas le nom posé par le serveur d'un test parallèle.
    /// Trouvé par l'audit du 8 septembre 2026.
    dernier_utilisateur: Arc<std::sync::Mutex<Option<String>>>,
}

impl russh::server::Server for TestSshServer {
    type Handler = TestSshSession;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self::Handler {
        self.connexions
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        TestSshSession {
            inondation: self.inondation.clone(),
            dernier_utilisateur: self.dernier_utilisateur.clone(),
            ..Default::default()
        }
    }
}

#[derive(Default)]
struct TestSshSession {
    channels: Arc<Mutex<HashMap<ChannelId, Channel<Msg>>>>,
    /// Canaux ayant demande le sous-systeme SFTP : ils transportent du binaire,
    /// l'echo du test PTY les corromprait.
    sftp_channels: Arc<Mutex<std::collections::HashSet<ChannelId>>>,
    /// État d'inondation partagé avec le serveur (tests de bornage du flux).
    inondation: EtatInondation,
    /// Utilisateur authentifié sur cette connexion, retenu pour `agent_request`
    /// (qui ne reçoit pas le nom) : « sans-agent » simule `AllowAgentForwarding
    /// no`. Audit du 7 septembre 2026.
    user: Option<String>,
    /// Handle partagé avec le serveur : dernier nom reçu, lisible par le test
    /// qui a lancé CE serveur (voir `spawn_test_sshd_observe`).
    dernier_utilisateur: Arc<std::sync::Mutex<Option<String>>>,
}

impl russh::server::Handler for TestSshSession {
    type Error = anyhow::Error;

    async fn auth_publickey(
        &mut self,
        user: &str,
        _key: &russh::keys::PublicKey,
    ) -> Result<Auth, Self::Error> {
        self.user = Some(user.to_owned());
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
        *self.dernier_utilisateur.lock().unwrap() = Some(user.to_owned());
        let attendu = b"le-bon".as_slice();
        let toutes_bonnes = r.all(|rep| rep == attendu);
        if toutes_bonnes {
            Ok(Auth::Accept)
        } else {
            Ok(Auth::reject())
        }
    }

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        self.user = Some(user.to_owned());
        // Les comptes « pam-* » n'acceptent PAS le mot de passe simple : c'est
        // ce qui force le repli vers keyboard-interactive.
        if user.starts_with("pam-") {
            return Ok(Auth::reject());
        }
        // Consigné tel quel : c'est ce que le serveur voit réellement, et le
        // seul moyen de vérifier qu'un nom de domaine « DOMAINE\\utilisateur »
        // traverse la chaîne sans être abîmé.
        *self.dernier_utilisateur.lock().unwrap() = Some(user.to_owned());
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
        // Marqueurs de test « sonde-* » : le serveur, en dehors de toute
        // commande lancée avec redirection d'agent, tente d'ouvrir un canal que
        // le client ne doit prêter que sur demande explicite, puis rapporte le
        // verdict du client sur le canal exec (préfixe `SONDE:`). Servent à
        // exercer le chemin de REFUS de `server_channel_open_agent_forward` et
        // de son jumeau `server_channel_open_forwarded_tcpip`, sans test jusque
        // là (audit du 7 septembre 2026).
        //
        // L'ouverture se fait dans une tâche détachée, PAS ici : `exec_request`
        // tourne dans la boucle `select!` de la session serveur, celle-là même
        // qui traite la confirmation du canal ouvert ; un `.await` d'ouverture
        // posé directement l'attendrait d'une boucle qu'il bloque
        // (interblocage). On ne renvoie donc pas eof/exit-status ici : la tâche
        // les émet une fois le verdict écrit, sinon `run()` sortirait sur
        // `Close` avant de l'avoir lu.
        if cmd.contains("sonde-agent") || cmd.contains("sonde-forward") {
            let handle = session.handle();
            let ouvre_agent = cmd.contains("sonde-agent");
            tokio::spawn(async move {
                let verdict = if ouvre_agent {
                    handle.channel_open_agent().await.map(|_| ())
                } else {
                    // Un port jamais passé à `remote_forward` : le client doit
                    // refuser cette destination locale arbitraire.
                    handle
                        .channel_open_forwarded_tcpip("localhost", 59_999, "10.9.8.7", 5555)
                        .await
                        .map(|_| ())
                };
                let msg = match verdict {
                    Ok(()) => "SONDE:ok".to_string(),
                    Err(e) => format!("SONDE:{e:?}"),
                };
                let _ = handle
                    .data(channel_id, bytes::Bytes::from(msg.into_bytes()))
                    .await;
                let _ = handle.eof(channel_id).await;
                let _ = handle.exit_status_request(channel_id, 0).await;
                let _ = handle.close(channel_id).await;
                drop(channel);
            });
            return Ok(());
        }
        // Marqueurs de test « INONDE » (plein débit) et « GOUTTE » (lent) : le
        // serveur émet des données sans fin sur le canal exec, en comptant les
        // octets remis, jusqu'à ce que le client ferme le canal (`channel_close`
        // lève `close_recu`). Prouve que le client borne le FLUX : `run` au
        // plafond de 1 Mio, `run_borne` à l'échéance. Audit du 7 septembre 2026.
        if cmd.contains("INONDE") || cmd.contains("GOUTTE") {
            let handle = session.handle();
            let etat = self.inondation.clone();
            let lent = cmd.contains("GOUTTE");
            tokio::spawn(async move {
                use std::sync::atomic::Ordering;
                // Bloc large pour saturer vite la fenêtre initiale (2 Mio) et
                // faire atteindre le plafond ; petit et espacé pour « GOUTTE »,
                // qui doit rester SOUS le plafond jusqu'à l'échéance.
                let bloc = if lent {
                    bytes::Bytes::from(vec![b'Z'; 256])
                } else {
                    bytes::Bytes::from(vec![b'Z'; 64 * 1024])
                };
                while !etat.close_recu.load(Ordering::SeqCst) {
                    if handle.data(channel_id, bloc.clone()).await.is_err() {
                        break; // session terminée
                    }
                    etat.octets.fetch_add(bloc.len() as u64, Ordering::SeqCst);
                    if lent {
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                }
                // Le canal reste vivant tant qu'on inonde : le lâcher enverrait
                // CLOSE au client, qui sortirait avant d'atteindre le plafond.
                drop(channel);
            });
            return Ok(());
        }
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
        // Marqueur de test : simule une commande interrompue — canal fermé SANS
        // exit-status (lien coupé, processus tué). Sert à vérifier que
        // `run_avec_agent` ne prend pas ce silence pour un succès.
        if !cmd.contains("SANS_STATUT") {
            let _ = session.exit_status_request(channel_id, code);
        }
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

    /// Redirection d'agent (`auth-agent-req@openssh.com`).
    ///
    /// Trouvé par l'audit du 7 septembre 2026 : `run_avec_agent` avalait le
    /// refus du serveur. « sans-agent » simule un serveur durci
    /// `AllowAgentForwarding no` ; tout autre compte l'accepte.
    ///
    /// ⚠️ russh 0.63 : le DÉFAUT de `agent_request` (rendre un `bool`) répond
    /// par un message GLOBAL `REQUEST_SUCCESS`/`REQUEST_FAILURE`
    /// (`server/session.rs`), JAMAIS par un `CHANNEL_SUCCESS`/`CHANNEL_FAILURE` —
    /// le client ne le voit pas sur le
    /// canal (il dépile `open_global_requests`, vide ici). Un simulacre qui se
    /// contenterait de rendre `Ok(user != "sans-agent")` n'émettrait donc aucun
    /// verdict de canal, et l'attente bornée de `run_avec_agent` filerait droit
    /// au délai de garde dans les deux cas. On appelle donc explicitement
    /// `channel_success`/`channel_failure` (ce que prescrit la doc de
    /// `Handler::agent_request`) et on rend `Ok(true)` pour ne pas pousser en
    /// plus un `REQUEST_FAILURE` global contradictoire.
    async fn agent_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<bool, Self::Error> {
        if self.user.as_deref() == Some("sans-agent") {
            let _ = session.channel_failure(channel);
        } else {
            let _ = session.channel_success(channel);
        }
        Ok(true)
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

    /// Le client ferme un canal. On le note pour les tests de bornage du flux :
    /// c'est la preuve que le client a envoyé `CHANNEL_CLOSE` (et pas seulement
    /// lâché son canal), et le signal d'arrêt de la tâche d'inondation.
    async fn channel_close(
        &mut self,
        _channel_id: ChannelId,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.inondation
            .close_recu
            .store(true, std::sync::atomic::Ordering::SeqCst);
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
    /// Le chemin ouvert sert PLUS d'octets qu'il n'en annonce : le fichier a
    /// grossi entre la lecture de sa taille et sa lecture (journal en cours
    /// d'écriture). Le transfert reste complet et doit rester un succès.
    petit_agrandi: bool,
    /// Descripteurs de dossiers /fs déjà lus : la seconde lecture rend Eof.
    dossiers_lus: std::collections::HashSet<String>,
}

// ---------- Système de fichiers en mémoire, sous /fs/ ----------
//
// Le simulacre historique répond la même chose à tout le monde (un fichier de
// démonstration, des écritures jetées) : assez pour les listes et les
// transferts simples, pas pour les dossiers récursifs, la reprise ou le
// relais d'hôte à hôte. Sous `/fs/`, ce serveur tient un vrai système de
// fichiers en mémoire : les octets écrits se relisent, à leur décalage, les
// dossiers se listent, et chaque lecture est journalisée avec son décalage
// pour qu'un test voie ce qu'une reprise a évité de redemander.

/// Fichiers en mémoire : chemin absolu → octets.
static FS_FICHIERS: std::sync::Mutex<Option<std::collections::HashMap<String, Vec<u8>>>> =
    std::sync::Mutex::new(None);
/// Dossiers en mémoire : chemins absolus.
static FS_DOSSIERS: std::sync::Mutex<Option<std::collections::HashSet<String>>> =
    std::sync::Mutex::new(None);
/// Liens symboliques en mémoire : chemin absolu du lien → longueur de la cible
/// (la « taille » qu'un `lstat` rend pour un lien). Ces entrées apparaissent
/// dans `readdir` avec des permissions `0o120xxx` (`S_IFLNK`) mais n'ont ni octets
/// dans `FS_FICHIERS` ni dossier dans `FS_DOSSIERS` : `stat` et `open` échouent
/// dessus (lien cassé), ce qui reproduit le cas de l'audit du 7 septembre 2026.
static FS_LIENS: std::sync::Mutex<Option<std::collections::HashMap<String, u64>>> =
    std::sync::Mutex::new(None);
/// Lectures servies sous /fs/ : (chemin, décalage).
static FS_LECTURES: std::sync::Mutex<Vec<(String, u64)>> = std::sync::Mutex::new(Vec::new());

fn fs_fichiers(
) -> std::sync::MutexGuard<'static, Option<std::collections::HashMap<String, Vec<u8>>>> {
    let mut g = FS_FICHIERS.lock().unwrap();
    if g.is_none() {
        *g = Some(std::collections::HashMap::new());
    }
    g
}
fn fs_dossiers() -> std::sync::MutexGuard<'static, Option<std::collections::HashSet<String>>> {
    let mut g = FS_DOSSIERS.lock().unwrap();
    if g.is_none() {
        let mut s = std::collections::HashSet::new();
        s.insert("/fs".to_owned());
        *g = Some(s);
    }
    g
}
/// Écrit un fichier en mémoire (et ses dossiers parents), depuis un test.
fn fs_poser(chemin: &str, contenu: &[u8]) {
    let mut d = fs_dossiers();
    let mut p = chemin
        .rsplit_once('/')
        .map(|(a, _)| a.to_owned())
        .unwrap_or_default();
    while !p.is_empty() && p != "/fs" {
        d.as_mut().unwrap().insert(p.clone());
        p = p
            .rsplit_once('/')
            .map(|(a, _)| a.to_owned())
            .unwrap_or_default();
    }
    fs_fichiers()
        .as_mut()
        .unwrap()
        .insert(chemin.to_owned(), contenu.to_vec());
}
fn fs_liens() -> std::sync::MutexGuard<'static, Option<std::collections::HashMap<String, u64>>> {
    let mut g = FS_LIENS.lock().unwrap();
    if g.is_none() {
        *g = Some(std::collections::HashMap::new());
    }
    g
}
/// Pose un lien symbolique cassé (sans cible servable) dans un dossier, depuis
/// un test : il sera listé mais ni ouvrable ni statable.
fn fs_poser_lien(chemin: &str, taille_cible: u64) {
    let mut d = fs_dossiers();
    let parent = chemin.rsplit_once('/').map(|(a, _)| a.to_owned());
    if let Some(p) = parent {
        if !p.is_empty() && p != "/fs" {
            d.as_mut().unwrap().insert(p);
        }
    }
    fs_liens()
        .as_mut()
        .unwrap()
        .insert(chemin.to_owned(), taille_cible);
}
fn fs_lire(chemin: &str) -> Option<Vec<u8>> {
    fs_fichiers().as_ref().unwrap().get(chemin).cloned()
}
fn fs_est_dossier(chemin: &str) -> bool {
    fs_dossiers().as_ref().unwrap().contains(chemin)
}
fn fs_lectures_de(chemin: &str) -> Vec<u64> {
    FS_LECTURES
        .lock()
        .unwrap()
        .iter()
        .filter(|(c, _)| c == chemin)
        .map(|(_, o)| *o)
        .collect()
}
fn fs_oublier_lectures() {
    FS_LECTURES.lock().unwrap().clear();
}
fn sous_fs(chemin: &str) -> bool {
    chemin == "/fs" || chemin.starts_with("/fs/")
}
fn parent_de(chemin: &str) -> &str {
    chemin.rsplit_once('/').map_or("", |(a, _)| a)
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
        pflags: russh_sftp::protocol::OpenFlags,
        _attrs: FileAttributes,
    ) -> impl Future<Output = Result<Handle, Self::Error>> + Send {
        self.coupure_en_lecture = filename.contains("coupure");
        self.gros_fichier = filename.contains("gros");
        // « tronque » annonce la taille du gros fichier mais n'en sert que le
        // premier quart : c'est le cas du journal en rotation, que huit lectures
        // concurrentes rendent bien plus probable qu'une lecture séquentielle.
        self.tronque = filename.contains("tronque");
        self.petit_agrandi = filename.contains("petit-agrandi");
        async move {
            if sous_fs(&filename) {
                use russh_sftp::protocol::OpenFlags;
                let existe = fs_lire(&filename).is_some();
                if !existe && !pflags.contains(OpenFlags::CREATE) {
                    return Err(StatusCode::NoSuchFile);
                }
                if !existe || pflags.contains(OpenFlags::TRUNCATE) {
                    fs_poser(&filename, b"");
                }
                // Le descripteur porte le chemin : le serveur n'a pas d'état.
                return Ok(Handle {
                    id,
                    handle: format!("fs:{filename}"),
                });
            }
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
        handle: String,
        offset: u64,
        len: u32,
    ) -> impl Future<Output = Result<russh_sftp::protocol::Data, Self::Error>> + Send {
        let done = std::mem::replace(&mut self.file_read_done, true);
        let coupure = self.coupure_en_lecture;
        let gros = self.gros_fichier;
        let tronque = self.tronque;
        let agrandi = self.petit_agrandi;
        async move {
            if let Some(chemin) = handle.strip_prefix("fs:") {
                let Some(contenu) = fs_lire(chemin) else {
                    return Err(StatusCode::NoSuchFile);
                };
                FS_LECTURES
                    .lock()
                    .unwrap()
                    .push((chemin.to_owned(), offset));
                let debut = usize::try_from(offset).unwrap_or(usize::MAX);
                if debut >= contenu.len() {
                    return Err(StatusCode::Eof);
                }
                let fin = (debut + len as usize).min(contenu.len());
                return Ok(russh_sftp::protocol::Data {
                    id,
                    data: contenu[debut..fin].to_vec(),
                });
            }
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
            if agrandi {
                // Le fichier a grossi depuis la lecture de sa taille : on sert
                // 40 octets là où `stat` en annonce 20. Le transfert est
                // complet (done >= total) et doit rester un succès.
                if done {
                    return Err(StatusCode::Eof);
                }
                return Ok(russh_sftp::protocol::Data {
                    id,
                    data: (0..40u8).collect(),
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
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> impl Future<Output = Result<Status, Self::Error>> + Send {
        async move {
            if let Some(chemin) = handle.strip_prefix("fs:") {
                let mut g = fs_fichiers();
                let Some(f) = g.as_mut().unwrap().get_mut(chemin) else {
                    return Err(StatusCode::NoSuchFile);
                };
                let debut = usize::try_from(offset).unwrap_or(usize::MAX);
                if f.len() < debut + data.len() {
                    f.resize(debut + data.len(), 0);
                }
                f[debut..debut + data.len()].copy_from_slice(&data);
            }
            Ok(ok_status(id))
        }
    }

    fn close(
        &mut self,
        id: u32,
        _handle: String,
    ) -> impl Future<Output = Result<Status, Self::Error>> + Send {
        async move { Ok(ok_status(id)) }
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
            if sous_fs(&path) {
                if fs_est_dossier(&path) {
                    return Err(StatusCode::Failure);
                }
                fs_dossiers().as_mut().unwrap().insert(path);
                return Ok(ok_status(id));
            }
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
            if sous_fs(&filename) {
                return match fs_fichiers().as_mut().unwrap().remove(&filename) {
                    Some(_) => Ok(ok_status(id)),
                    None => Err(StatusCode::NoSuchFile),
                };
            }
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
            if sous_fs(&path) {
                let plein = fs_fichiers()
                    .as_ref()
                    .unwrap()
                    .keys()
                    .any(|c| parent_de(c) == path);
                if plein {
                    return Err(StatusCode::Failure);
                }
                fs_dossiers().as_mut().unwrap().remove(&path);
                return Ok(ok_status(id));
            }
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
            if sous_fs(&oldpath) {
                let mut g = fs_fichiers();
                return match g.as_mut().unwrap().remove(&oldpath) {
                    Some(c) => {
                        g.as_mut().unwrap().insert(newpath, c);
                        Ok(ok_status(id))
                    }
                    None => Err(StatusCode::NoSuchFile),
                };
            }
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
        path: String,
    ) -> impl Future<Output = Result<Handle, Self::Error>> + Send {
        async move {
            if sous_fs(&path) {
                if !fs_est_dossier(&path) {
                    return Err(StatusCode::NoSuchFile);
                }
                return Ok(Handle {
                    id,
                    handle: format!("fsdir:{path}"),
                });
            }
            Ok(Handle {
                id,
                handle: "dir".into(),
            })
        }
    }

    fn readdir(
        &mut self,
        id: u32,
        handle: String,
    ) -> impl Future<Output = Result<russh_sftp::protocol::Name, Self::Error>> + Send {
        // Un descripteur de dossier /fs se lit une fois : la seconde lecture
        // rend Eof, comme un vrai serveur.
        let deja_lu = !self.dossiers_lus.insert(handle.clone());
        async move {
            if let Some(chemin) = handle.strip_prefix("fsdir:") {
                if deja_lu {
                    return Err(StatusCode::Eof);
                }
                let mut files = Vec::new();
                // Un dossier menteur : une entrée dont le nom sort du dossier.
                if chemin == "/fs/hostile" {
                    files.push(File {
                        filename: "../evasion.txt".into(),
                        longname: String::new(),
                        attrs: FileAttributes {
                            size: Some(3),
                            permissions: Some(0o100_644),
                            ..Default::default()
                        },
                    });
                }
                for (c, contenu) in fs_fichiers().as_ref().unwrap() {
                    if parent_de(c) == chemin {
                        files.push(File {
                            filename: c.rsplit('/').next().unwrap_or(c).to_owned(),
                            longname: String::new(),
                            attrs: FileAttributes {
                                size: Some(contenu.len() as u64),
                                permissions: Some(0o100_644),
                                ..Default::default()
                            },
                        });
                    }
                }
                for d in fs_dossiers().as_ref().unwrap() {
                    if parent_de(d) == chemin {
                        files.push(File {
                            filename: d.rsplit('/').next().unwrap_or(d).to_owned(),
                            longname: String::new(),
                            attrs: FileAttributes {
                                size: Some(0),
                                permissions: Some(0o40755),
                                ..Default::default()
                            },
                        });
                    }
                }
                // Les liens symboliques : permissions `S_IFLNK` (0o120xxx), que le
                // client traduit en FileType::Symlink (ni fichier ni dossier).
                for (l, taille) in fs_liens().as_ref().unwrap() {
                    if parent_de(l) == chemin {
                        files.push(File {
                            filename: l.rsplit('/').next().unwrap_or(l).to_owned(),
                            longname: String::new(),
                            attrs: FileAttributes {
                                size: Some(*taille),
                                permissions: Some(0o120_777),
                                ..Default::default()
                            },
                        });
                    }
                }
                return Ok(russh_sftp::protocol::Name { id, files });
            }
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
        // Le lecteur (en bandes comme séquentiel) se règle sur la taille
        // annoncée : elle doit correspondre à ce que `read` sert réellement,
        // sinon un transfert complet passerait pour incomplet (le mock
        // annonçait 42 là où il ne servait que 20, ce qui masquait le défaut du
        // chemin séquentiel une fois la garde posée).
        let taille = if path.contains("gros") {
            GROS_FICHIER.len() as u64
        } else if path.contains("petit-tronque") {
            // Annonce 80 mais `read` ne sert que 20 puis Eof : la troncature
            // séquentielle que la garde de `download_with` doit rejeter.
            80
        } else if path.contains("petit-agrandi") {
            // Annonce 20 mais `read` en sert 40 : le fichier a grossi, le
            // transfert reste complet et doit réussir.
            20
        } else {
            // Ce que le `read` générique sert réellement (« CONTENU-FICHIER-TEST »).
            20
        };
        async move {
            if sous_fs(&path) {
                if let Some(c) = fs_lire(&path) {
                    return Ok(russh_sftp::protocol::Attrs {
                        id,
                        attrs: FileAttributes {
                            size: Some(c.len() as u64),
                            permissions: Some(0o100_644),
                            mtime: Some(1_700_000_000),
                            ..Default::default()
                        },
                    });
                }
                if fs_est_dossier(&path) {
                    return Ok(russh_sftp::protocol::Attrs {
                        id,
                        attrs: FileAttributes {
                            size: Some(0),
                            permissions: Some(0o40755),
                            ..Default::default()
                        },
                    });
                }
                return Err(StatusCode::NoSuchFile);
            }
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
    spawn_test_sshd_compte().await.0
}

/// Comme `spawn_test_sshd`, avec le compteur de connexions de ce serveur.
async fn spawn_test_sshd_compte() -> (u16, Arc<std::sync::atomic::AtomicUsize>) {
    let config = russh::server::Config {
        keys: vec![CLE_HOTE.clone()],
        ..Default::default()
    };
    let config = Arc::new(config);
    let mut server = TestSshServer::default();
    let connexions = server.connexions.clone();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = server.run_on_socket(config, &listener).await;
    });
    (port, connexions)
}

/// Comme `spawn_test_sshd`, avec le handle « dernier utilisateur reçu » de CE
/// serveur : un test qui l'observe ne lit plus le nom posé par un autre serveur
/// de test tournant en parallèle.
async fn spawn_test_sshd_observe() -> (u16, Arc<std::sync::Mutex<Option<String>>>) {
    let config = Arc::new(russh::server::Config {
        keys: vec![CLE_HOTE.clone()],
        ..Default::default()
    });
    let mut server = TestSshServer::default();
    let dernier = server.dernier_utilisateur.clone();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = server.run_on_socket(config, &listener).await;
    });
    (port, dernier)
}

/// Comme `spawn_test_sshd`, avec l'état d'inondation partagé de ce serveur,
/// pour les tests de bornage du flux (marqueurs « INONDE » / « GOUTTE »).
async fn spawn_test_sshd_inondation() -> (u16, EtatInondation) {
    let config = russh::server::Config {
        keys: vec![CLE_HOTE.clone()],
        ..Default::default()
    };
    let config = Arc::new(config);
    let mut server = TestSshServer::default();
    let etat = server.inondation.clone();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = server.run_on_socket(config, &listener).await;
    });
    (port, etat)
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

/// Clé éphémère pour l'auth, générée UNE fois par processus.
///
/// Elle l'était « si le fichier n'existe pas », depuis chaque test : deux
/// tests en parallèle passaient tous deux ce contrôle, ou le second voyait
/// le fichier à moitié écrit et lisait une clé tronquée — « Could not read
/// key », six tests rouges d'un coup, régression vue en CI GitHub le
/// 2026-09-03 sur un commit qui ne touchait ni au cœur ni à ces tests. Le
/// `LazyLock` fait attendre tout le monde jusqu'à ce que la clé soit là, et
/// le renommage garantit qu'on ne voit jamais un fichier partiel.
static CLE_TEST: std::sync::LazyLock<std::path::PathBuf> = std::sync::LazyLock::new(|| {
    let path = virtual_home().join("id_ed25519");
    let key = PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
    let mut buf = Vec::new();
    russh::keys::encode_pkcs8_pem(&key, &mut buf).unwrap();
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, buf).unwrap();
    std::fs::rename(&tmp, &path).unwrap();
    path
});

fn temp_key_path() -> std::path::PathBuf {
    CLE_TEST.clone()
}

/// Huit fils demandent la clé en même temps : un seul chemin, une clé
/// lisible. Avec l'ancien « si le fichier n'existe pas », l'un d'eux lisait
/// un fichier en cours d'écriture.
#[test]
fn la_cle_de_test_est_generee_une_fois_meme_en_parallele() {
    // Lancer les huit fils AVANT d'en joindre aucun : un `.map(spawn).map(join)`
    // paresseux joignait chaque fil avant de lancer le suivant, donc rien de
    // parallèle. Trouvé par l'audit du 8 septembre 2026.
    let fils: Vec<_> = (0..8).map(|_| std::thread::spawn(temp_key_path)).collect();
    let chemins: Vec<_> = fils.into_iter().map(|h| h.join().unwrap()).collect();
    assert!(
        chemins.iter().all(|c| c == &chemins[0]),
        "chemins : {chemins:?}"
    );
    let pem = std::fs::read_to_string(&chemins[0]).unwrap();
    assert!(
        russh::keys::decode_secret_key(&pem, None).is_ok(),
        "la clé de test doit se relire"
    );
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
    // Aucun agent SSH joignable pendant les tests : `ouvrir_agent_local` rend
    // alors None, et la garde d'agent, quand le drapeau est levé, refuse en
    // `ConnectFailed` — verdict déterministe qui ne dépend pas de l'agent réel
    // du poste. Posé ici une seule fois, pour ne pas muter l'environnement
    // depuis un test parallèle (cf. `un_canal_d_agent_hors_commande...`).
    std::env::set_var("SSH_AUTH_SOCK", home.join("agent-inexistant.sock"));
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

/// Attend que le serveur signale `CHANNEL_CLOSE`, avec une échéance : sinon le
/// test rougirait par un `assert` clair plutôt que de pendre.
async fn attendre_close(etat: &EtatInondation, quoi: &str) {
    let echeance = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
    while !etat.close_recu.load(std::sync::atomic::Ordering::SeqCst) {
        assert!(
            tokio::time::Instant::now() < echeance,
            "le canal n'a pas été fermé ({quoi}) : CHANNEL_CLOSE jamais reçu par le serveur"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

/// Le flux doit s'être tari : on laisse d'abord la dernière itération en vol se
/// poser, puis on vérifie que le compteur d'octets servis ne bouge plus.
async fn affirmer_flux_tari(etat: &EtatInondation) {
    use std::sync::atomic::Ordering;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let avant = etat.octets.load(Ordering::SeqCst);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let apres = etat.octets.load(Ordering::SeqCst);
    assert_eq!(
        avant, apres,
        "le serveur inonde encore après la fermeture du canal ({avant} -> {apres} octets)"
    );
}

/// Trouvé par l'audit du 7 septembre 2026 : `run` sortait au plafond de 1 Mio
/// sans fermer le canal. `russh::Channel` n'envoie pas de `CHANNEL_CLOSE` à sa
/// chute et le client réalimente la fenêtre : un serveur qui répond
/// `cat /dev/zero` continuait de nous inonder toute la vie de la session. On
/// vérifie que `run` ferme le canal (le serveur reçoit `CHANNEL_CLOSE`), que le
/// flux se tarit, et que la session reste utilisable derrière.
#[tokio::test]
async fn le_plafond_de_sortie_ferme_le_canal() {
    let (port, etat) = spawn_test_sshd_inondation().await;
    let auth = test_auth();
    let mut session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion échouée");

    let (stdout, _code) = session.run("INONDE").await.unwrap();
    assert!(
        stdout.contains("plafond de 1 Mio atteint"),
        "la sortie aurait dû être tronquée au plafond : {}",
        &stdout[..stdout.len().min(120)]
    );

    attendre_close(&etat, "plafond").await;
    affirmer_flux_tari(&etat).await;

    // La même session reste utilisable après la fermeture du canal inondé.
    let (ok, code) = session.run("echo ok").await.unwrap();
    assert_eq!(code, 0);
    assert!(ok.contains("CMD:echo ok"), "session inutilisable : {ok}");
    session.disconnect().await.unwrap();
}

/// Trouvé par l'audit du 7 septembre 2026 : la sonde d'OS bornait l'attente par
/// un `timeout` posé par-dessus `run`, mais lâchait son canal à l'échéance sans
/// le fermer. Un serveur qui débite lentement (jamais le plafond) continuait
/// donc d'inonder la session longue de l'onglet. `run_borne` ferme le canal à
/// l'échéance : on vérifie qu'elle rend une erreur, que le serveur reçoit
/// `CHANNEL_CLOSE`, que le flux se tarit et que la session reste utilisable.
#[tokio::test]
async fn la_sonde_d_os_ferme_son_canal_a_l_echeance() {
    let (port, etat) = spawn_test_sshd_inondation().await;
    let auth = test_auth();
    let mut session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion échouée");

    // Débit lent : le plafond n'est jamais atteint, seule l'échéance tranche.
    let probe = session
        .run_borne("GOUTTE", std::time::Duration::from_millis(300))
        .await;
    assert!(
        probe.is_err(),
        "la sonde bornée aurait dû rendre une erreur d'échéance : {probe:?}"
    );

    attendre_close(&etat, "échéance").await;
    affirmer_flux_tari(&etat).await;

    let (ok, code) = session.run("echo ok").await.unwrap();
    assert_eq!(code, 0);
    assert!(ok.contains("CMD:echo ok"), "session inutilisable : {ok}");
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
/// Le panneau SFTP d'un onglet ouvre un canal sur la session du terminal, pas
/// une seconde connexion : le serveur ne doit voir qu'une session, et le
/// terminal doit rester utilisable pendant que le panneau travaille.
#[tokio::test]
async fn sftp_sur_la_session_du_terminal_ne_rouvre_pas_de_connexion() {
    use std::sync::atomic::Ordering;
    let (port, connexions) = spawn_test_sshd_compte().await;
    let auth = test_auth();
    let mut session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion échouée");
    let mut pty = session.open_pty(80, 24, "xterm").await.expect("PTY");
    let _ = pty.out_rx.recv().await.expect("bannière du PTY");

    let sftp = avash::sftp::SftpHandle::open_on(&mut session)
        .await
        .expect("SFTP sur la session existante");
    let entries = sftp.list("/").await.unwrap();
    assert!(
        entries.iter().any(|e| e.name == "rapport.md"),
        "{entries:?}"
    );

    // Le terminal vit toujours sur la même session : l'écho du serveur revient.
    pty.in_tx.send(b"ping\n".to_vec()).await.unwrap();
    let echo = tokio::time::timeout(std::time::Duration::from_secs(5), pty.out_rx.recv())
        .await
        .expect("le PTY n'a plus répondu après l'ouverture du SFTP")
        .expect("PTY fermé");
    assert!(!echo.is_empty());

    assert_eq!(
        connexions.load(Ordering::SeqCst),
        1,
        "le serveur a vu plus d'une connexion pour un seul onglet"
    );
    sftp.close().await.unwrap();
    // La session mère n'est pas emportée par la fermeture du canal SFTP.
    let (out, code) = session.run("echo encore").await.unwrap();
    assert_eq!(code, 0, "{out}");
}

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

    // Trouvé par l'audit du 8 septembre 2026 : on nettoyait par
    // `forget_host_key_at`, une réécriture complète du `known_hosts` PARTAGÉ qui
    // court avec les `learn_known_hosts_path` (ajouts) des tests parallèles et
    // pouvait en perdre. On APPREND plutôt la vraie clé du serveur pour ce port
    // (un simple ajout) : `juger_cle_hote` accepte une clé parmi plusieurs, donc
    // un futur serveur de test qui hérite du port est reconnu, et le leurre peut
    // rester sans nuire.
    russh::keys::known_hosts::learn_known_hosts_path(
        "127.0.0.1",
        port,
        CLE_HOTE.public_key(),
        &known_hosts,
    )
    .expect("apprentissage de la vraie clé");

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
    // Réponse propre à CE test. Trouvé par l'audit du 7 septembre 2026 : ce
    // test et `une_redirection_distante_en_port_zero_prend_le_port_du_serveur`
    // tournent en parallèle et poussent tous deux « HELLO » dans le statique
    // partagé REMOTE_REPLY ; avec une charge utile identique, la boucle de l'un
    // pouvait être satisfaite par la réponse de l'autre. On marque la réponse
    // par le port local (unique au test) et on filtre dessus.
    let attendu = format!("HELLO-{local_port}");
    let reponse = attendu.clone();
    tokio::spawn(async move {
        let (mut s, _) = local.accept().await.unwrap();
        let mut buf = [0u8; 16];
        let _ = s.read(&mut buf).await.unwrap();
        s.write_all(reponse.as_bytes()).await.unwrap();
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
        if REMOTE_REPLY
            .lock()
            .await
            .iter()
            .any(|r| r == attendu.as_bytes())
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "aucune reponse marquée recue via -R"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    // `bytes_up` compte le sens serveur -> service local (le « hello » de 5
    // octets), `bytes_down` le retour service local -> serveur (notre réponse
    // marquée) : cf. ForwardCounters::relay(a = canal serveur, b = local).
    let attendu_len = attendu.len() as u64;
    let snap = attendre_compteurs(
        &tunnel,
        |s| s.total == 1 && s.bytes_up == 5 && s.bytes_down == attendu_len,
        "un aller de 5 octets et le retour marqué de ce test",
    )
    .await;
    assert_eq!(snap.total, 1);
    assert_eq!(snap.bytes_up, 5, "« hello » vers le service local");
    assert_eq!(
        snap.bytes_down, attendu_len,
        "la réponse marquée vers le serveur"
    );
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

/// Quand le renommage final échoue, l'erreur dit où est le fichier complet et
/// le `.part` reste.
///
/// Trouvé par l'audit du 7 septembre 2026 : le transfert termine par
/// `rename(.part, cible)`. Sous Windows, ce remplacement échoue par violation de
/// partage si la cible est ouverte ailleurs sans `FILE_SHARE_DELETE` (Acrobat,
/// Excel, un aperçu de l'Explorateur) ou porte l'attribut lecture seule : le
/// transfert est pourtant COMPLET (tout entier dans le `.part`), mais l'échec
/// était annoncé sans un mot sur le `.part` laissé à côté, transfert à 100 %
/// affiché en erreur. On force ici le même échec sous Unix — cible finale qui
/// est un répertoire, `rename` d'un fichier dessus rend EISDIR — et l'on exige
/// que l'erreur pointe le `.part` et que ce `.part` complet survive.
#[tokio::test]
async fn un_renommage_final_impossible_pointe_le_part_complet() {
    let port = spawn_test_sshd().await;
    let session = connect_for_tunnel(port).await;
    let sftp = avash::sftp::SftpHandle::open(session).await.unwrap();
    // `/srv/fichier.txt` est petit : chemin séquentiel de `download_with`.
    let local = std::env::temp_dir().join(format!("avash-verrou-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&local);
    let _ = std::fs::remove_file(&local);
    std::fs::create_dir(&local).unwrap();

    let issue = sftp
        .download_with("/srv/fichier.txt", &local, |_, _| {})
        .await;

    let e = issue.expect_err("le renommage vers un répertoire aurait dû échouer");
    let msg = format!("{e:#}");
    assert!(
        msg.contains("le fichier complet est dans"),
        "l'erreur ne dit pas où est le fichier complet : {msg}"
    );
    // Le `.part` porte le transfert complet et n'a pas été supprimé.
    let partiel = std::path::PathBuf::from(format!("{}.part", local.display()));
    assert_eq!(
        std::fs::read(&partiel).unwrap(),
        b"CONTENU-FICHIER-TEST",
        "le .part complet doit rester à côté de la cible"
    );

    let _ = std::fs::remove_file(&partiel);
    let _ = std::fs::remove_dir_all(&local);
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

/// Jumeau séquentiel du test précédent : un PETIT fichier (<= 128 Kio, donc
/// chemin séquentiel de `download_with`) tronqué en cours de lecture ne doit
/// pas non plus passer pour un succès.
///
/// Trouvé par l'audit du 7 septembre 2026 : le chemin séquentiel renommait le
/// `.part` sur la cible sans vérifier `done == total`. Un journal distant
/// rotationné/tronqué pendant la lecture atteint un EOF propre plus tôt
/// (`stat` annonce 80, `read` sert 20 puis Eof) : sans garde, le préfixe de
/// 20 octets était promu sur la cible, transfert annoncé réussi.
#[tokio::test]
async fn un_petit_fichier_tronque_ne_passe_pas_pour_un_succes() {
    let port = spawn_test_sshd().await;
    let session = connect_for_tunnel(port).await;
    let sftp = avash::sftp::SftpHandle::open(session).await.unwrap();
    let local =
        std::env::temp_dir().join(format!("avash-petit-tronque-{}.bin", std::process::id()));
    let _ = std::fs::remove_file(&local);

    let issue = sftp
        .download_with("/srv/petit-tronque.bin", &local, |_, _| {})
        .await;

    let Err(e) = issue else {
        panic!("un petit fichier incomplet ne doit pas être un succès")
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

/// Miroir du précédent : un PETIT fichier qui a GROSSI pendant la lecture
/// (`stat` annonce 20, `read` en sert 40 puis Eof) reste un transfert complet
/// et doit RÉUSSIR — c'est ce qui impose une garde `done < total`, et non
/// `done != total` qui rejetterait à tort un journal en cours d'écriture.
#[tokio::test]
async fn un_petit_fichier_qui_a_grossi_reste_un_succes() {
    let port = spawn_test_sshd().await;
    let session = connect_for_tunnel(port).await;
    let sftp = avash::sftp::SftpHandle::open(session).await.unwrap();
    let local =
        std::env::temp_dir().join(format!("avash-petit-agrandi-{}.bin", std::process::id()));
    let _ = std::fs::remove_file(&local);

    let n = sftp
        .download_with("/srv/petit-agrandi.bin", &local, |_, _| {})
        .await
        .expect("un fichier complet, même plus gros qu'annoncé, doit réussir");

    assert_eq!(n, 40, "les 40 octets réellement servis sont écrits");
    assert_eq!(
        std::fs::read(&local).unwrap(),
        (0..40u8).collect::<Vec<u8>>(),
        "la cible porte les octets servis, pas le préfixe annoncé"
    );
    let partiel = local.with_extension("bin.part");
    assert!(!partiel.exists(), "un .part orphelin est resté");

    let _ = std::fs::remove_file(&local);
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
    let (port, dernier) = spawn_test_sshd_observe().await;
    let _home = virtual_home();

    let auth = avash::ssh::ClientAuth {
        user: "TEST\\Adrien".into(),
        key_path: None,
        password: Some("secret".into()),
    };
    let session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion");

    assert_eq!(
        dernier.lock().unwrap().as_deref(),
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

/// Trouvé par l'audit du 7 septembre 2026 : `authenticate` faisait `?` sur le
/// chargement de la clé. Une clé chiffrée par phrase de passe (chargée dans
/// l'agent, donc `ssh` fonctionne) faisait échouer la connexion AVANT même
/// d'essayer l'agent ou le mot de passe. On utilise l'utilisateur « refuse »,
/// qui rejette la clé publique : un éventuel agent SSH réel de la machine ne
/// peut donc pas aboutir, et l'on éprouve bien le repli par mot de passe.
#[tokio::test]
async fn une_cle_chiffree_ne_bloque_pas_le_repli_mot_de_passe() {
    let port = spawn_test_sshd().await;
    std::sync::LazyLock::force(&HOME_POSE); // pose HOME
    let cle = PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
    let chiffree = cle.encrypt(&mut rand::rng(), "phrase-de-passe").unwrap();
    let pem = chiffree
        .to_openssh(russh::keys::ssh_key::LineEnding::LF)
        .unwrap();
    let path = virtual_home().join(format!("id_chiffree-{}", std::process::id()));
    std::fs::write(&path, pem.as_bytes()).unwrap();
    // Garde-fou : la clé ne doit PAS se charger sans phrase, sinon le test ne
    // prouverait rien.
    assert!(
        russh::keys::load_secret_key(&path, None).is_err(),
        "la clé de test doit être chiffrée"
    );

    // Clé chiffrée + mot de passe accepté : la connexion aboutit par le mot de
    // passe, au lieu d'échouer au chargement de la clé.
    let auth = avash::ssh::ClientAuth {
        user: "refuse".into(),
        key_path: Some(path.clone()),
        password: Some("le-bon".into()),
    };
    let mut session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("le mot de passe doit prendre le relais d'une clé chiffrée");
    let (out, _) = session.run("echo ok").await.unwrap();
    assert!(out.contains("CMD:echo ok"), "{out:?}");
    session.disconnect().await.unwrap();

    // Sans mot de passe : l'échec porte le marqueur (l'interface demandera un
    // mot de passe) et nomme la clé inutilisable.
    let auth_sans = avash::ssh::ClientAuth {
        user: "refuse".into(),
        key_path: Some(path.clone()),
        password: None,
    };
    let err = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth_sans)
        .await
        .err()
        .expect("sans mot de passe, la connexion doit échouer");
    let msg = format!("{err:#}");
    assert!(
        msg.contains(avash::ssh::PASSWORD_REQUIRED),
        "le marqueur d'invite de mot de passe est attendu : {msg}"
    );
    assert!(
        msg.contains("id_chiffree"),
        "la clé inutilisable doit être nommée : {msg}"
    );
    let _ = std::fs::remove_file(&path);
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
async fn un_rebond_a_cle_changee_garde_le_marqueur_sous_le_format_alterne() {
    // Trouvé par l'audit du 7 septembre 2026 : `connect_via` enrobe l'échec d'un
    // rebond d'un « Rebond hôte:port », ce qui enterre le marqueur
    // `[AVASH_HOST_KEY_CHANGED]`. Le `Display` d'anyhow (`to_string()`, ce que
    // faisait `etablir`) n'affiche que ce contexte externe et perd le marqueur ;
    // l'interface, qui le repère par inclusion, ne proposait alors pas d'oublier
    // la clé changée d'un rebond. `{e:#}` déroule toute la chaîne, marqueur
    // compris.
    let jump_port = spawn_test_sshd().await;
    let _auth = test_auth(); // pose HOME
                             // Fausse clé mémorisée pour le REBOND : sa clé d'hôte « a changé ».
    let decoy = PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
    let known_hosts = avash::ssh::chemin_known_hosts().unwrap();
    russh::keys::known_hosts::learn_known_hosts_path(
        "127.0.0.1",
        jump_port,
        decoy.public_key(),
        &known_hosts,
    )
    .unwrap();

    let hop = avash::ssh::Hop {
        addr: "127.0.0.1".into(),
        port: jump_port,
        auth: test_auth(),
    };
    let err = avash::ssh::AvashSession::connect_via(&[hop], "127.0.0.1", jump_port, &test_auth())
        .await
        .err()
        .expect("une clé de rebond changée doit être refusée");

    let _ = avash::ssh::forget_host_key_at("127.0.0.1", jump_port, &known_hosts);

    let alterne = format!("{err:#}");
    assert!(
        alterne.contains("Rebond"),
        "le contexte du rebond est attendu : {alterne}"
    );
    assert!(
        alterne.contains(avash::ssh::HOST_KEY_CHANGED),
        "le marqueur doit survivre au format alterné : {alterne}"
    );
    assert!(
        !err.to_string().contains(avash::ssh::HOST_KEY_CHANGED),
        "to_string() n'affiche que le contexte externe et perd le marqueur : {err}"
    );
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

/// Trouvé par l'audit du 7 septembre 2026 : un commentaire en tête du fichier
/// décalait la numérotation. russh ne compte pas les lignes `# …` (son
/// `continue` saute l'incrément), mais l'oubli les comptait via `enumerate()` :
/// oublier `hôteB` retirait en réalité la clé de `hôteA`, dont le TOFU repartait
/// alors de zéro, et laissait celle de `hôteB` en place.
#[tokio::test]
async fn forget_host_key_ne_touche_pas_a_un_autre_hote_apres_un_commentaire() {
    let path = std::env::temp_dir().join(format!(
        "avash-kh-cmt-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_file(&path);
    let cle_a =
        russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
    let cle_b =
        russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
    // Un commentaire, puis deux hôtes distincts. La forme d'une entrée
    // known_hosts est « hôte type base64 » : c'est ce qu'écrit `to_openssh`.
    let ligne = |hote: &str, k: &russh::keys::PrivateKey| {
        format!("{hote} {}\n", k.public_key().to_openssh().unwrap())
    };
    let contenu = format!(
        "# mes serveurs\n{}{}",
        ligne("hoteA", &cle_a),
        ligne("hoteB", &cle_b)
    );
    std::fs::write(&path, &contenu).unwrap();

    let retirees = avash::ssh::forget_host_key_at("hoteB", 22, &path).unwrap();
    assert_eq!(retirees, 1, "une seule clé retirée");
    let reste = std::fs::read_to_string(&path).unwrap();
    assert!(
        reste.contains("hoteA "),
        "la clé de hoteA doit rester :\n{reste}"
    );
    assert!(
        !reste.contains("hoteB "),
        "la clé de hoteB doit partir :\n{reste}"
    );
    assert!(
        reste.contains("# mes serveurs"),
        "le commentaire doit rester"
    );
    // hoteA reste effectivement reconnu par russh après l'opération.
    let a = russh::keys::known_hosts::known_host_keys_path("hoteA", 22, &path).unwrap();
    assert_eq!(a.len(), 1, "hoteA toujours connu");
    let _ = std::fs::remove_file(&path);
}

// ---------- Transferts : dossiers, reprise, bandes montantes, annulation, relais ----------

/// Un dossier temporaire à ce test, vide.
fn dossier_temp(nom: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("avash-transferts-{}-{nom}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Un contenu où chaque octet dépend de sa position : un réassemblage faux se voit.
fn motif(n: usize, graine: u32) -> Vec<u8> {
    (0..n)
        .map(|i| ((i as u32).wrapping_mul(31).wrapping_add(graine) % 251) as u8)
        .collect()
}

async fn sftp_de_test() -> avash::sftp::SftpHandle {
    let port = spawn_test_sshd().await;
    let session = connect_for_tunnel(port).await;
    avash::sftp::SftpHandle::open(session).await.unwrap()
}

/// Un dossier distant entier arrive avec ses sous-dossiers, ses fichiers
/// (dont un assez gros pour les bandes) et ses dossiers vides.
#[tokio::test]
async fn un_dossier_distant_se_telecharge_entier() {
    let gros = motif(300 * 1024, 1);
    fs_poser("/fs/dl/a.txt", b"alpha");
    fs_poser("/fs/dl/sous/b.bin", &gros);
    fs_dossiers()
        .as_mut()
        .unwrap()
        .insert("/fs/dl/vide".to_owned());
    let sftp = sftp_de_test().await;
    let local = dossier_temp("dl");
    let mut dernier = avash::sftp::Avancement::default();
    let n = sftp
        .download_dir_with("/fs/dl", &local, None, |a| {
            dernier = a;
        })
        .await
        .unwrap();
    assert_eq!(n as usize, 5 + gros.len());
    assert_eq!(std::fs::read(local.join("a.txt")).unwrap(), b"alpha");
    assert_eq!(
        std::fs::read(local.join("sous").join("b.bin")).unwrap(),
        gros
    );
    assert!(local.join("vide").is_dir(), "le dossier vide est recréé");
    assert_eq!((dernier.termines, dernier.nombre, dernier.fait), (2, 2, n));
    let _ = std::fs::remove_dir_all(&local);
    sftp.close().await.unwrap();
}

/// Un dossier local entier part avec son arborescence ; le gros fichier
/// arrive à l'octet près.
#[tokio::test]
async fn un_dossier_local_se_televerse_entier() {
    let gros = motif(300 * 1024 + 11, 2);
    let local = dossier_temp("up");
    std::fs::create_dir_all(local.join("sous").join("creux")).unwrap();
    std::fs::write(local.join("a.txt"), b"alpha").unwrap();
    std::fs::write(local.join("sous").join("b.bin"), &gros).unwrap();
    let sftp = sftp_de_test().await;
    let mut dernier = avash::sftp::Avancement::default();
    let n = sftp
        .upload_dir_with(&local, "/fs/up/arbre", None, |a| {
            dernier = a;
        })
        .await
        .unwrap();
    assert_eq!(n as usize, 5 + gros.len());
    assert_eq!(fs_lire("/fs/up/arbre/a.txt").unwrap(), b"alpha");
    assert_eq!(fs_lire("/fs/up/arbre/sous/b.bin").unwrap(), gros);
    assert!(fs_est_dossier("/fs/up/arbre/sous/creux"));
    assert_eq!((dernier.termines, dernier.nombre), (2, 2));
    let _ = std::fs::remove_dir_all(&local);
    sftp.close().await.unwrap();
}

/// Annulé en route, un téléchargement garde son `.part` et sa carte ; relancé,
/// il rend le fichier entier et ne laisse aucune trace.
///
/// Huit mégaoctets : huit bandes de seize blocs. Avec 400 Kio (deux blocs par
/// bande), les bandes finissaient leurs lectures avant que la boucle de
/// progression, qui pose le drapeau, ne les ait toutes vues : sur l'exécuteur
/// macOS de la chaîne, le transfert rendait Ok(409600) au lieu de
/// « annulé » (régression vue en CI, 04/09/2026). Le drapeau est vérifié par
/// chaque bande avant chaque bloc : avec seize blocs par bande, il est vu.
#[tokio::test]
async fn un_telechargement_annule_reprend_et_rend_le_fichier_entier() {
    let gros = motif(8 * 1024 * 1024, 3);
    fs_poser("/fs/reprise/gros.bin", &gros);
    let sftp = sftp_de_test().await;
    let local = dossier_temp("reprise").join("gros.bin");
    let annulation: avash::sftp::Annulation = std::sync::Arc::default();
    let a = annulation.clone();
    let issue = sftp
        .download_reprise(
            "/fs/reprise/gros.bin",
            &local,
            Some(&annulation),
            move |fait, _| {
                if fait >= 1024 * 1024 {
                    a.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            },
        )
        .await;
    let e = issue.expect_err("l'annulation doit se voir");
    assert!(e.to_string().contains(avash::sftp::ANNULE), "{e:#}");
    let partiel = local.with_file_name("gros.bin.part");
    let carte = local.with_file_name("gros.bin.part.reprise");
    // La carte n'existe que si une bande a fini avant l'annulation, ce que
    // huit bandes menées de front ne garantissent pas ; le .part, lui, reste.
    assert!(partiel.exists(), "le .part reste pour la reprise");
    let n = sftp
        .download_reprise("/fs/reprise/gros.bin", &local, None, |_, _| {})
        .await
        .unwrap();
    assert_eq!(n as usize, gros.len());
    assert_eq!(
        std::fs::read(&local).unwrap(),
        gros,
        "le fichier repris est faux"
    );
    assert!(
        !partiel.exists() && !carte.exists(),
        "plus de trace après la reprise"
    );
    let _ = std::fs::remove_dir_all(local.parent().unwrap());
    sftp.close().await.unwrap();
}

/// La reprise ne redemande pas les bandes que la carte dit complètes : avec
/// une carte posée à la main (première et dernière bandes faites, leurs
/// octets déjà dans le `.part`), aucune lecture ne tombe dans ces bandes, et
/// le fichier rendu est entier. Déterministe, là où l'annulation en cours de
/// route ne garantit pas qu'une bande ait fini.
#[tokio::test]
async fn la_reprise_ne_relit_pas_les_bandes_que_la_carte_dit_faites() {
    let gros = motif(2 * 1024 * 1024, 5);
    fs_poser("/fs/reprise2/gros.bin", &gros);
    let sftp = sftp_de_test().await;
    let local = dossier_temp("reprise2").join("gros.bin");
    let partiel = local.with_file_name("gros.bin.part");
    let carte = local.with_file_name("gros.bin.part.reprise");
    // Huit bandes de 256 Kio : la première et la dernière sont « faites ».
    let bande = 256 * 1024u64;
    let faites = vec![(0, bande), (7 * bande, 8 * bande)];
    let mut part = vec![0u8; gros.len()];
    for (d, f) in &faites {
        let (d, f) = (*d as usize, *f as usize);
        part[d..f].copy_from_slice(&gros[d..f]);
    }
    std::fs::write(&partiel, &part).unwrap();
    // Même taille et même date que le serveur de test : la carte vaut.
    std::fs::write(
        &carte,
        serde_json::json!({ "taille": gros.len(), "mtime": 1_700_000_000u64, "faites": faites })
            .to_string(),
    )
    .unwrap();

    fs_oublier_lectures();
    let n = sftp
        .download_reprise("/fs/reprise2/gros.bin", &local, None, |_, _| {})
        .await
        .unwrap();
    assert_eq!(n as usize, gros.len());
    assert_eq!(
        std::fs::read(&local).unwrap(),
        gros,
        "le fichier repris est faux"
    );
    assert!(
        !partiel.exists() && !carte.exists(),
        "plus de trace après la reprise"
    );
    let lectures = fs_lectures_de("/fs/reprise2/gros.bin");
    assert!(!lectures.is_empty(), "les six bandes manquantes se lisent");
    for o in lectures {
        assert!(
            !faites.iter().any(|(d, f)| *d <= o && o < *f),
            "la reprise a relu le décalage {o}, dans une bande déjà faite {faites:?}"
        );
    }
    let _ = std::fs::remove_dir_all(local.parent().unwrap());
    sftp.close().await.unwrap();
}

/// Une carte disant une bande « faite » alors que le `.part` est plus court
/// que cette bande ne doit pas promouvoir un fichier faux.
///
/// Trouvé par l'audit du 7 septembre 2026 : la carte `.part.reprise` est écrite
/// avec un `fsync` (`ecrire_atomiquement`), mais les octets d'une bande
/// n'avaient qu'un `flush` (pas de `fsync`) au moment de l'annonce « bande
/// faite ». Après une coupure de courant, la carte durable pouvait promettre
/// une bande que le `.part` n'avait pas encore reçue, plus courte sur le disque.
/// À la reprise, la bande dite faite n'était pas relue, `fait == total` comptait
/// `deja_fait` sans regarder le fichier, et le `rename` promouvait un fichier
/// troué (zéros là où l'écriture d'une bande suivante avait étendu le `.part`
/// en sparse). Ici, la carte annonce `(0, bande)` faite mais le `.part` s'arrête
/// à `bande / 2` : la reprise doit refuser cette carte et repartir de zéro, donc
/// relire la bande 0, et rendre le fichier entier.
#[tokio::test]
async fn une_carte_de_reprise_survit_a_un_part_tronque() {
    let gros = motif(2 * 1024 * 1024, 6);
    fs_poser("/fs/reprise3/gros.bin", &gros);
    let sftp = sftp_de_test().await;
    let local = dossier_temp("reprise3").join("gros.bin");
    let partiel = local.with_file_name("gros.bin.part");
    let carte = local.with_file_name("gros.bin.part.reprise");
    // Huit bandes de 256 Kio : la carte prétend la première faite, mais le
    // `.part` s'arrête au milieu de cette bande (crash entre la donnée non
    // durable et la carte durable).
    let bande = 256 * 1024u64;
    let faites = vec![(0u64, bande)];
    let tronque = (bande / 2) as usize;
    std::fs::write(&partiel, &gros[..tronque]).unwrap();
    std::fs::write(
        &carte,
        serde_json::json!({ "taille": gros.len(), "mtime": 1_700_000_000u64, "faites": faites })
            .to_string(),
    )
    .unwrap();

    let n = sftp
        .download_reprise("/fs/reprise3/gros.bin", &local, None, |_, _| {})
        .await
        .unwrap();
    assert_eq!(n as usize, gros.len());
    assert_eq!(
        std::fs::read(&local).unwrap(),
        gros,
        "un .part tronqué sous une carte optimiste ne doit pas être promu tel quel"
    );
    assert!(
        !partiel.exists() && !carte.exists(),
        "plus de trace après la reprise"
    );
    let _ = std::fs::remove_dir_all(local.parent().unwrap());
    sftp.close().await.unwrap();
}

/// Même chose en montée : la carte `.envoi.reprise` note ce qui est
/// sûrement écrit, la reprise repart de là et le fichier distant finit entier.
#[tokio::test]
async fn un_envoi_annule_reprend_sans_renvoyer_les_bandes_faites() {
    // Douze mégaoctets : au moins deux points de contrôle (tous les 4 Mio)
    // avant l'annulation, pour que la reprise ait quelque chose à reprendre.
    let gros = motif(12 * 1024 * 1024 + 5, 4);
    let local = dossier_temp("envoi").join("gros.bin");
    std::fs::write(&local, &gros).unwrap();
    let sftp = sftp_de_test().await;
    let annulation: avash::sftp::Annulation = std::sync::Arc::default();
    let a = annulation.clone();
    let issue = sftp
        .upload_reprise(
            &local,
            "/fs/envoi/gros.bin",
            true,
            Some(&annulation),
            move |fait, _| {
                if fait >= 9 * 1024 * 1024 {
                    a.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            },
        )
        .await;
    assert!(issue.is_err());
    let carte = local.with_file_name("gros.bin.envoi.reprise");
    assert!(carte.exists(), "la carte d'envoi reste");
    let mut premiere = None;
    let n = sftp
        .upload_reprise(&local, "/fs/envoi/gros.bin", true, None, |fait, _| {
            premiere.get_or_insert(fait);
        })
        .await
        .unwrap();
    assert_eq!(n as usize, gros.len());
    assert!(
        premiere.unwrap_or(0) > 0,
        "la reprise repart de ce qui était fait"
    );
    assert_eq!(fs_lire("/fs/envoi/gros.bin").unwrap(), gros);
    assert!(!carte.exists());
    let _ = std::fs::remove_dir_all(local.parent().unwrap());
    sftp.close().await.unwrap();
}

/// Trouvé par l'audit du 7 septembre 2026 : un envoi de fichier unitaire faisait
/// `create(remote)`, qui tronque une cible du même nom sans un mot. Avec
/// `refuser_ecrasement`, une cible distante déjà présente fait refuser l'envoi et
/// reste intacte ; vers un chemin libre, le même envoi passe.
#[tokio::test]
async fn un_envoi_unitaire_ne_remplace_pas_une_cible_existante() {
    let sftp = sftp_de_test().await;
    let local = dossier_temp("envoi_ecrase").join("note.txt");
    std::fs::create_dir_all(local.parent().unwrap()).unwrap();
    std::fs::write(&local, b"nouveau contenu").unwrap();
    fs_poser("/fs/ecrase/note.txt", b"precieux, a garder");

    let issue = sftp
        .upload_reprise(&local, "/fs/ecrase/note.txt", true, None, |_, _| {})
        .await;
    assert!(
        issue.is_err(),
        "un envoi ne doit pas écraser une cible existante"
    );
    assert_eq!(
        fs_lire("/fs/ecrase/note.txt").as_deref(),
        Some(&b"precieux, a garder"[..]),
        "la cible existante doit rester intacte"
    );

    // Vers un chemin libre, le même envoi passe.
    let n = sftp
        .upload_reprise(&local, "/fs/ecrase/neuf.txt", true, None, |_, _| {})
        .await
        .unwrap();
    assert_eq!(n, b"nouveau contenu".len() as u64);
    assert_eq!(fs_lire("/fs/ecrase/neuf.txt").unwrap(), b"nouveau contenu");

    let _ = std::fs::remove_dir_all(local.parent().unwrap());
    sftp.close().await.unwrap();
}

/// Trouvé par l'audit du 7 septembre 2026 : `relayer_vers` lisait la taille par
/// `metadata(...).map_or(0, ...)`. Une source illisible (lien symbolique vers un
/// fichier supprimé) donnait `total = 0`, la cible était alors créée vide et la
/// copie annoncée « réussie » ; une cible préexistante était tronquée avant que
/// l'échec ne survienne. Une source illisible doit échouer SANS créer ni
/// tronquer la cible.
#[tokio::test]
async fn relayer_vers_echoue_sur_une_source_illisible_sans_toucher_la_cible() {
    let source = sftp_de_test().await;
    let cible = sftp_de_test().await;
    // Cible préexistante non vide : elle ne doit être ni tronquée ni écrasée.
    fs_poser("/fs/relaisko/garde.txt", b"contenu-precieux");

    let issue = source
        .relayer_vers(
            "/fs/relaisko/absente.log",
            &cible,
            "/fs/relaisko/garde.txt",
            false,
            None,
            |_, _| {},
        )
        .await;
    assert!(issue.is_err(), "une source illisible doit échouer");
    assert_eq!(
        fs_lire("/fs/relaisko/garde.txt").as_deref(),
        Some(&b"contenu-precieux"[..]),
        "la cible existante doit rester intacte"
    );

    // Vers une cible neuve : aucun fichier vide ne doit être créé.
    let issue2 = source
        .relayer_vers(
            "/fs/relaisko/absente2.log",
            &cible,
            "/fs/relaisko/neuf.bin",
            false,
            None,
            |_, _| {},
        )
        .await;
    assert!(issue2.is_err(), "une source illisible doit échouer");
    assert!(
        fs_lire("/fs/relaisko/neuf.bin").is_none(),
        "aucune cible vide ne doit être créée"
    );
    source.close().await.unwrap();
    cible.close().await.unwrap();
}

/// Trouvé par l'audit du 7 septembre 2026 : `relayer_vers` faisait
/// `create(remote_cible)`, qui tronque un fichier du même nom chez la cible — une
/// copie de fichier vers un autre hôte écrasait sans un mot. Avec
/// `refuser_ecrasement`, une cible existante fait refuser la copie et reste
/// intacte (le serveur en mémoire honore `OpenFlags::TRUNCATE`, la troncature
/// serait donc visible sinon).
#[tokio::test]
async fn relayer_vers_ne_remplace_pas_une_cible_existante() {
    let source = sftp_de_test().await;
    let cible = sftp_de_test().await;
    fs_poser("/fs/copie/src.bin", &motif(200 * 1024, 7));
    fs_poser("/fs/copie/deja.bin", b"a garder chez la cible");

    let issue = source
        .relayer_vers(
            "/fs/copie/src.bin",
            &cible,
            "/fs/copie/deja.bin",
            true,
            None,
            |_, _| {},
        )
        .await;
    assert!(
        issue.is_err(),
        "la copie ne doit pas écraser une cible existante"
    );
    assert_eq!(
        fs_lire("/fs/copie/deja.bin").as_deref(),
        Some(&b"a garder chez la cible"[..]),
        "la cible existante doit rester intacte"
    );
    source.close().await.unwrap();
    cible.close().await.unwrap();
}

/// Un dossier passe d'un serveur à un autre par le poste sans rien y écrire :
/// deux serveurs, deux sessions, et les octets identiques à l'arrivée.
#[tokio::test]
async fn un_dossier_se_relaie_d_un_serveur_a_l_autre() {
    let gros = motif(300 * 1024 + 3, 5);
    fs_poser("/fs/relais/src/x.bin", &gros);
    fs_poser("/fs/relais/src/sous/y.txt", b"y");
    let source = sftp_de_test().await;
    let cible = sftp_de_test().await;
    let temp = std::env::temp_dir();
    let mut dernier = avash::sftp::Avancement::default();
    // Trouvé par l'audit du 8 septembre 2026 : on ne vérifiait l'absence de
    // `.part` local qu'APRÈS le transfert — un relais qui passerait par le
    // disque puis nettoierait resterait vert. On OBSERVE donc pendant le
    // transfert : à chaque progression, aucun `*.part` du relais ne doit
    // apparaître dans le répertoire temporaire (le fichier fait 300 Kio, donc
    // plusieurs bandes et plusieurs rappels).
    let mut vu_part_en_vol = false;
    let n = source
        .relayer_dir_vers("/fs/relais/src", &cible, "/fs/relais/dst", None, |a| {
            dernier = a;
            if let Ok(entrees) = std::fs::read_dir(&temp) {
                if entrees.flatten().any(|e| {
                    let nom = e.file_name();
                    let nom = nom.to_string_lossy();
                    nom.contains("x.bin.part") || nom.contains("y.txt.part")
                }) {
                    vu_part_en_vol = true;
                }
            }
        })
        .await
        .unwrap();
    assert_eq!(n as usize, gros.len() + 1);
    assert_eq!(fs_lire("/fs/relais/dst/x.bin").unwrap(), gros);
    assert_eq!(fs_lire("/fs/relais/dst/sous/y.txt").unwrap(), b"y");
    assert_eq!((dernier.termines, dernier.nombre), (2, 2));
    assert!(
        !vu_part_en_vol,
        "le relais a écrit un .part sur le disque du poste pendant le transfert"
    );
    // Et rien ne subsiste après coup non plus.
    assert!(
        !std::fs::read_dir(&temp)
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().contains("x.bin.part")),
        "le relais a laissé un .part sur le disque du poste"
    );
    source.close().await.unwrap();
    cible.close().await.unwrap();
}

/// Trouvé par l'audit du 7 septembre 2026 : un dossier distant qui contient un
/// lien symbolique (ici cassé, comme un `current -> releases/1.2.3` dont la
/// cible a disparu) faisait échouer TOUT le téléchargement. `parcourir` rangeait
/// le lien en fichier (son `file_type` n'est ni dossier ni fichier régulier),
/// puis `download_reprise` tentait d'`open`/`read` dessus, l'échec remontait par
/// `?` et les fichiers listés après le lien n'étaient jamais reçus. Le lien doit
/// désormais être ignoré (comme `scp -r` sans `-L`) et le reste du dossier arriver.
#[tokio::test]
async fn un_lien_symbolique_dans_un_dossier_ne_casse_pas_le_telechargement() {
    fs_poser("/fs/liens/a.txt", b"alpha");
    fs_poser("/fs/liens/b.txt", b"bravo");
    // Lien cassé « courant » : listé, mais ni ouvrable ni statable.
    fs_poser_lien("/fs/liens/courant", 14);
    let sftp = sftp_de_test().await;
    let local = dossier_temp("liens");
    let mut dernier = avash::sftp::Avancement::default();
    let n = sftp
        .download_dir_with("/fs/liens", &local, None, |a| {
            dernier = a;
        })
        .await
        .expect("le lien doit être ignoré, pas faire échouer le dossier");
    assert_eq!(n, b"alpha".len() as u64 + b"bravo".len() as u64);
    assert_eq!(std::fs::read(local.join("a.txt")).unwrap(), b"alpha");
    assert_eq!(std::fs::read(local.join("b.txt")).unwrap(), b"bravo");
    assert!(
        !local.join("courant").exists(),
        "le lien n'a pas à être matérialisé en local"
    );
    // Seuls les deux fichiers réguliers comptent, la progression ne compte pas le lien.
    assert_eq!((dernier.termines, dernier.nombre), (2, 2));
    let _ = std::fs::remove_dir_all(&local);
    sftp.close().await.unwrap();
}

/// Même défaut sur le relais serveur à serveur : `relayer_dir_vers` réutilise
/// `parcourir`, donc un lien cassé dans la source faisait tout échouer et, pire,
/// `relayer_vers` créait un fichier VIDE à la place du lien (total nul → `Ok`).
/// Le lien doit être ignoré et rien de faux créé chez la cible.
#[tokio::test]
async fn un_lien_symbolique_dans_un_dossier_ne_casse_pas_le_relais() {
    fs_poser("/fs/lienrelais/src/x.bin", b"donnees");
    fs_poser_lien("/fs/lienrelais/src/courant", 14);
    let source = sftp_de_test().await;
    let cible = sftp_de_test().await;
    let mut dernier = avash::sftp::Avancement::default();
    let n = source
        .relayer_dir_vers(
            "/fs/lienrelais/src",
            &cible,
            "/fs/lienrelais/dst",
            None,
            |a| {
                dernier = a;
            },
        )
        .await
        .expect("le lien doit être ignoré, pas faire échouer le relais");
    assert_eq!(n, b"donnees".len() as u64);
    assert_eq!(fs_lire("/fs/lienrelais/dst/x.bin").unwrap(), b"donnees");
    assert!(
        fs_lire("/fs/lienrelais/dst/courant").is_none(),
        "aucun fichier vide ne doit être créé à la place du lien"
    );
    assert_eq!((dernier.termines, dernier.nombre), (1, 1));
    source.close().await.unwrap();
    cible.close().await.unwrap();
}

/// Une annulation posée avant le départ arrête tout, en le disant.
#[tokio::test]
async fn un_transfert_annule_avant_de_partir_le_dit() {
    fs_poser("/fs/annule/f.txt", b"abc");
    let sftp = sftp_de_test().await;
    let annulation: avash::sftp::Annulation =
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let local = dossier_temp("annule");
    let e = sftp
        .download_dir_with("/fs/annule", &local, Some(&annulation), |_| {})
        .await
        .expect_err("annulé");
    assert!(e.to_string().contains(avash::sftp::ANNULE), "{e:#}");
    let _ = std::fs::remove_dir_all(&local);
    sftp.close().await.unwrap();
}

/// Trouvé par la revue de sécurité du commit : un serveur qui nomme une
/// entrée « ../evasion.txt » ne doit rien faire écrire hors du dossier local.
#[tokio::test]
async fn un_serveur_qui_nomme_une_entree_hors_du_dossier_est_refuse() {
    fs_dossiers()
        .as_mut()
        .unwrap()
        .insert("/fs/hostile".to_owned());
    let sftp = sftp_de_test().await;
    let local = dossier_temp("hostile").join("recu");
    let e = sftp
        .download_dir_with("/fs/hostile", &local, None, |_| {})
        .await
        .expect_err("le nom doit être refusé");
    assert!(e.to_string().contains("nom interdit"), "{e:#}");
    assert!(
        !local.parent().unwrap().join("evasion.txt").exists(),
        "un fichier a été écrit hors du dossier de réception"
    );
    let _ = std::fs::remove_dir_all(local.parent().unwrap());
    sftp.close().await.unwrap();
}

/// Sous `AVASH_HOME`, les téléchargements restent sous ce toit : le vrai dossier
/// Téléchargements de l'utilisateur ne doit pas recevoir les fichiers d'un bac à
/// sable (cinquième passage Windows de la suite bout en bout, 05/09/2026, où
/// `dirs::download_dir()` ignorait la variable). Le sous-dossier des
/// téléchargements est pris dès qu'il existe.
#[test]
fn sous_avash_home_les_telechargements_restent_sous_le_foyer() {
    std::sync::LazyLock::force(&HOME_POSE);
    let foyer = std::path::PathBuf::from(std::env::var_os("AVASH_HOME").unwrap());
    assert!(
        avash::sftp::default_local_dir().starts_with(&foyer),
        "{:?} hors de {foyer:?}",
        avash::sftp::default_local_dir()
    );
    let telechargements = foyer.join("Téléchargements");
    std::fs::create_dir_all(&telechargements).unwrap();
    assert_eq!(avash::sftp::default_local_dir(), telechargements);
}

/// Le plafond de sortie d'une commande est d'un mébioctet, pas de deux
/// kibioctets : trois kilo-octets reviennent entiers, par `run` comme par
/// `run_avec_agent`, qui rend aussi la sortie et le code. Mutants survivants
/// du premier passage de cargo-mutants (05/09/2026) : `run_avec_agent`
/// remplacé par `Ok((String::new(), 0))`, `1024 * 1024` par `1024 + 1024`,
/// `>=` par `<` sur le plafond.
#[tokio::test]
async fn les_deux_executions_rendent_toute_une_sortie_de_trois_kilo_octets() {
    let port = spawn_test_sshd().await;
    let auth = test_auth();
    let mut session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion");
    let long = "x".repeat(3000);
    let (out, code) = session.run(&format!("echo {long}")).await.unwrap();
    assert_eq!(code, 0);
    assert!(
        out.contains(&long) && !out.contains("tronquée"),
        "sortie coupée : {} octets",
        out.len()
    );
    let (out, code) = session
        .run_avec_agent(&format!("echo {long} ; exit 7"), None)
        .await
        .unwrap();
    assert_eq!(code, 7, "le code de sortie de run_avec_agent");
    assert!(
        out.contains("CMD:echo") && out.contains(&long) && !out.contains("tronquée"),
        "sortie de run_avec_agent : {} octets",
        out.len()
    );
    session.disconnect().await.unwrap();
}

/// Trouvé par l'audit du 7 septembre 2026 : `run_avec_agent` initialisait le
/// code de sortie à 0 et ne le changeait que sur `ExitStatus`. Une copie
/// directe (scp) interrompue — canal fermé sans statut — rendait donc 0 et se
/// voyait « réussie ». Sans statut de sortie, la commande doit échouer.
#[tokio::test]
async fn run_avec_agent_echoue_quand_le_canal_ferme_sans_statut() {
    let port = spawn_test_sshd().await;
    let auth = test_auth();
    let session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion");
    // Le serveur de test ferme le canal sans exit-status sur ce marqueur.
    let err = session
        .run_avec_agent("echo SANS_STATUT", None)
        .await
        .expect_err("une fermeture sans statut doit échouer");
    assert!(
        err.to_string().contains("sans code de sortie"),
        "message inattendu : {err}"
    );
    // Une commande normale, elle, rend toujours son code.
    let (_out, code) = session
        .run_avec_agent("echo ok ; exit 3", None)
        .await
        .unwrap();
    assert_eq!(code, 3);
}

/// Trouvé par l'audit du 7 septembre 2026 : `run_avec_agent` faisait
/// `agent_forward(true)` sans consommer le verdict du serveur, et la boucle exec
/// ignorait `ChannelMsg::Failure` (`_ => {}`). Contre un serveur durci
/// `AllowAgentForwarding no` (`CHANNEL_FAILURE`), le refus était avalé :
/// Avash
/// lançait quand même `scp … autre:`, qui échouait « Permission denied » en
/// accusant la clé, jamais le refus de redirection. Le verdict est désormais
/// consommé AVANT `exec` : un refus rend une erreur qui nomme la vraie cause,
/// une acceptation laisse la commande se dérouler normalement.
///
/// Le simulacre émet un vrai `CHANNEL_SUCCESS`/`CHANNEL_FAILURE` dans
/// `agent_request`
/// (« sans-agent » = refus) : le défaut de russh 0.63 répondrait par un message
/// GLOBAL invisible du client, jamais par un verdict de canal (cf. le
/// commentaire du handler).
#[tokio::test]
async fn run_avec_agent_signale_le_refus_de_redirection_d_agent() {
    let port = spawn_test_sshd().await;
    // Compte refusé pour la redirection : le serveur répond CHANNEL_FAILURE.
    let auth_refuse = avash::ssh::ClientAuth {
        user: "sans-agent".into(),
        key_path: Some(temp_key_path()),
        password: None,
    };
    let session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth_refuse)
        .await
        .expect("connexion");
    let err = session
        .run_avec_agent("true", None)
        .await
        .expect_err("un refus de redirection d'agent doit échouer");
    assert!(
        err.to_string().contains("refuse la redirection d'agent"),
        "le message doit nommer le refus de redirection, pas la clé : {err}"
    );

    // Le chemin passant, lui, se déroule : le serveur accepte la redirection
    // (CHANNEL_SUCCESS) et la commande rend sa sortie et son code.
    let auth_ok = test_auth();
    let session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth_ok)
        .await
        .expect("connexion");
    let (out, code) = session
        .run_avec_agent("echo ok ; exit 5", None)
        .await
        .expect("une redirection acceptée laisse la commande aboutir");
    assert_eq!(code, 5);
    assert!(out.contains("CMD:echo"), "sortie inattendue : {out}");
}

/// Se déconnecter ferme la session, et `is_closed` le dit.
#[tokio::test]
async fn la_deconnexion_ferme_la_session() {
    let port = spawn_test_sshd().await;
    let auth = test_auth();
    let session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion");
    assert!(!session.is_closed(), "ouverte tant qu'on ne l'a pas fermée");
    session.disconnect().await.unwrap();
    let echeance = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    while !session.is_closed() {
        assert!(
            tokio::time::Instant::now() < echeance,
            "la session ne se ferme pas après disconnect"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// Un marqueur `@revoked` sur l'hôte dans `known_hosts` bloque la connexion
/// avant toute clé, par la vraie lecture du fichier et pas seulement par la
/// fonction pure (mutant survivant : `marqueur_bloquant` remplacé par `None`).
/// L'hôte visé est « localhost », résolu vers le serveur de test : les autres
/// tests parlent à 127.0.0.1 et ne voient pas la ligne ; russh, lui, lit
/// « @revoked » comme un hôte qui ne correspond à rien.
#[tokio::test]
async fn un_hote_marque_revoked_dans_known_hosts_est_refuse() {
    std::sync::LazyLock::force(&HOME_POSE);
    let port = spawn_test_sshd().await;
    let chemin = avash::ssh::chemin_known_hosts().unwrap();
    std::fs::create_dir_all(chemin.parent().unwrap()).unwrap();
    let cle =
        russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
    let ligne = format!(
        "@revoked localhost {}\n",
        cle.public_key().to_openssh().unwrap()
    );
    {
        use std::io::Write as _;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&chemin)
            .unwrap()
            .write_all(ligne.as_bytes())
            .unwrap();
    }
    let auth = test_auth();
    let issue = avash::ssh::AvashSession::connect("localhost", port, &auth).await;
    let Err(e) = issue else {
        panic!("un hôte révoqué a été accepté");
    };
    assert!(format!("{e:#}").contains("@revoked"), "{e:#}");
}

/// Une redirection distante demandée sur le port 0 prend le port que le
/// serveur choisit (40 000 chez le serveur de test), et c'est celui-là qui est
/// rendu et qui relaie (mutants survivants sur `port == 0 && bound != 0`).
#[tokio::test]
async fn une_redirection_distante_en_port_zero_prend_le_port_du_serveur() {
    let local = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let local_port = local.local_addr().unwrap().port();
    // Réponse marquée par le port local : le statique REMOTE_REPLY est partagé
    // avec `tunnel_distant_relaie_vers_un_service_local` (audit du 7 septembre
    // 2026), qui poussait le même « HELLO » ; la moitié « relaie » de ce test
    // était alors satisfaite par la réponse de l'AUTRE test, sans jamais lire
    // les compteurs de son propre tunnel.
    let attendu = format!("HELLO-{local_port}");
    let reponse = attendu.clone();
    tokio::spawn(async move {
        let (mut s, _) = local.accept().await.unwrap();
        let mut buf = [0u8; 16];
        let _ = s.read(&mut buf).await.unwrap();
        s.write_all(reponse.as_bytes()).await.unwrap();
    });
    let port = spawn_test_sshd().await;
    let session = connect_for_tunnel(port).await;
    // Directement par la session : la définition d'un tunnel refuse le port 0
    // (validation_refuse_un_port_d_ecoute_a_zero), c'est la couche SSH qui
    // sait laisser le serveur choisir.
    // Compteurs de CE tunnel : c'est le vrai garde-fou. Sous un mutant qui
    // garderait le calcul de `bound` mais supprimerait la ré-indexation
    // `f.insert(bound, dest)` (ssh.rs), la table reste clée sur 0, le canal
    // forwarded-tcpip sur 40 000 est refusé et ces compteurs restent à zéro,
    // quel que soit l'état de REMOTE_REPLY.
    let compteurs = Arc::new(avash::ssh::ForwardCounters::default());
    let bound = session
        .remote_forward("localhost", 0, "127.0.0.1", local_port, compteurs.clone())
        .await
        .expect("redirection distante en port 0");
    assert_eq!(bound, 40_000, "le port choisi par le serveur");
    // Le relais est asynchrone : attente bornée sur l'Arc<ForwardCounters>
    // (attendre_compteurs ne prend qu'un &Tunnel). `bytes_up` = « hello » du
    // serveur vers le service local (5), `bytes_down` = notre réponse marquée
    // vers le serveur ; cf. ForwardCounters::relay(a = canal serveur, b = local).
    let attendu_len = attendu.len() as u64;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        use std::sync::atomic::Ordering::Relaxed;
        let compteurs_ok = compteurs.total.load(Relaxed) == 1
            && compteurs.bytes_up.load(Relaxed) == 5
            && compteurs.bytes_down.load(Relaxed) == attendu_len;
        let reply_ok = REMOTE_REPLY
            .lock()
            .await
            .iter()
            .any(|r| r == attendu.as_bytes());
        if compteurs_ok && reply_ok {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "le relais en port 0 n'a pas atteint MON service : total={}, up={}, down={}",
            compteurs.total.load(Relaxed),
            compteurs.bytes_up.load(Relaxed),
            compteurs.bytes_down.load(Relaxed),
        );
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
}

/// Trouvé par l'audit du 7 septembre 2026 : la garde
/// `server_channel_open_agent_forward` — qui empêche un serveur déjà accepté
/// par TOFU d'emprunter l'agent du poste hors d'une commande explicitement
/// lancée avec redirection — n'avait AUCUN test sur son chemin de refus. Un
/// mutant qui inversait le drapeau (`!load` -> `load`), l'initialisait à
/// `true`, ou vidait le `Drop` de `GardeAgent`, survivait à toute la suite.
///
/// Le serveur de test ouvre un canal d'agent hors commande (`run`) puis pendant
/// une commande à redirection (`run_avec_agent`) et rapporte le verdict rendu
/// par le client. Les DEUX moitiés sont nécessaires : le `Drop` du handle de
/// russh rejette déjà en `AdministrativelyProhibited` quand la réponse est
/// simplement lâchée, donc « refusé hors commande » seul ne prouverait pas que
/// le drapeau commande le prêt. C'est le passage à `ConnectFailed` sous
/// `run_avec_agent` (drapeau levé, la garde atteint l'agent, absent ici) qui le
/// prouve. La troisième sonde vérifie la retombée : le `Drop` de `GardeAgent`
/// remet le drapeau à false, et l'agent n'est plus prêté ensuite.
#[cfg(unix)]
#[tokio::test]
async fn un_canal_d_agent_hors_commande_est_refuse_mais_prete_le_temps_d_une_commande() {
    // `HOME_POSE` a posé `SSH_AUTH_SOCK` sur un chemin inexistant : aucun agent
    // n'est joignable, la garde refuse en `ConnectFailed` quand le drapeau est
    // levé, sans dépendre de l'agent réel du poste ni le muter.
    std::sync::LazyLock::force(&HOME_POSE);
    let port = spawn_test_sshd().await;
    let auth = test_auth();
    let mut session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion");

    // 1) Hors commande : le serveur ne doit pas obtenir l'agent du poste.
    let (out, _) = session.run("sonde-agent").await.unwrap();
    assert!(
        out.contains("AdministrativelyProhibited"),
        "hors commande, le canal d'agent doit être administrativement refusé : {out}"
    );

    // 2) Pendant `run_avec_agent` : le drapeau est levé, la garde laisse passer
    // et échoue plus loin faute d'agent joignable — verdict distinct, preuve que
    // le drapeau a bien changé avec la commande.
    let (out, _) = session.run_avec_agent("sonde-agent", None).await.unwrap();
    assert!(
        out.contains("ConnectFailed"),
        "pendant une commande à redirection, la garde doit atteindre l'agent (absent ici) : {out}"
    );

    // 3) Retombée : après la commande, le `Drop` de `GardeAgent` a remis le
    // drapeau à false ; un canal d'agent est de nouveau refusé.
    let (out, _) = session.run("sonde-agent").await.unwrap();
    assert!(
        out.contains("AdministrativelyProhibited"),
        "après la commande, l'agent ne doit plus être prêté : {out}"
    );

    session.disconnect().await.unwrap();
}

/// Jumeau du précédent pour `server_channel_open_forwarded_tcpip` : le serveur
/// ouvre un canal `forwarded-tcpip` sur un port JAMAIS enregistré par
/// `remote_forward`, et le client doit refuser cette destination locale
/// arbitraire. Trouvé par l'audit du 7 septembre 2026 : seul le chemin
/// acceptant (port enregistré, cf. `une_redirection_distante_en_port_zero...`)
/// était testé, le chemin de refus ne l'était pas.
///
/// Le refus se lit `ConnectFailed` (et non `AdministrativelyProhibited`) : c'est
/// le code de rejet que pose `server_channel_open_forwarded_tcpip` pour une
/// destination inconnue (ssh.rs). Ce qui compte est que le canal soit REFUSÉ
/// (jamais `SONDE:ok`, qui signifierait un relais accordé) : un mutant qui
/// accepterait le canal produirait `SONDE:ok` et ferait rougir l'assertion.
#[tokio::test]
async fn un_forwarded_tcpip_sur_un_port_non_enregistre_est_refuse() {
    let port = spawn_test_sshd().await;
    let auth = test_auth();
    let mut session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion");
    let (out, _) = session.run("sonde-forward").await.unwrap();
    assert!(
        out.contains("ChannelOpenFailure(ConnectFailed)"),
        "un forwarded-tcpip sur un port non enregistré doit être refusé : {out}"
    );
    session.disconnect().await.unwrap();
}
