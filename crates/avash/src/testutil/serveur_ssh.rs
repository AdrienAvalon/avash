//! Serveur SSH+SFTP en mémoire (russh server, russh-sftp server), sur un port
//! éphémère de 127.0.0.1, pour éprouver le vrai client d'Avash de bout en bout.
//!
//! Il vivait dans `tests/integration.rs`, donc inaccessible aux tests
//! d'`avash-ui` : leurs commandes SFTP, tunnels et dépôt de clé n'avaient aucun
//! test. Extrait tel quel par l'audit du 12 septembre 2026 (C-couv-4), derrière
//! la fonctionnalité `outils-de-test` que seules les `[dev-dependencies]`
//! posent : il n'entre jamais dans un binaire publié.

// Code de test compilé dans la bibliothèque : ses aides déroulent leur décor
// par `unwrap` (un décor qui échoue doit faire échouer le test sur place), ce
// que `allow-unwrap-in-tests` ne couvre pas hors d'une fonction `#[test]`. Et
// ses fonctions publiques ne sont pas une API : `must_use` n'y apporte rien.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::must_use_candidate,
    clippy::missing_panics_doc
)]

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
pub static REMOTE_REPLY: Mutex<Vec<Vec<u8>>> = Mutex::const_new(Vec::new());

/// État partagé des tests d'inondation (marqueurs « INONDE » / « GOUTTE »).
///
/// Le serveur émet des données sans fin sur le canal exec ; ces compteurs, lus
/// côté test, prouvent que le client borne bien le FLUX et pas seulement
/// l'attente : au plafond (`run`) comme à l'échéance (`run_borne`), il ferme le
/// canal, ce que `channel_close` constate, et le flux se tarit alors.
#[derive(Clone, Default)]
pub struct EtatInondation {
    /// Octets remis à `handle.data` par la tâche d'inondation.
    pub octets: Arc<std::sync::atomic::AtomicU64>,
    /// Levé par `channel_close` : preuve que le client a envoyé `CHANNEL_CLOSE`,
    /// et signal d'arrêt pour la tâche d'inondation.
    pub close_recu: Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Clone, Default)]
pub struct TestSshServer {
    /// Connexions TCP acceptées par CETTE instance : de quoi prouver qu'une
    /// opération n'a pas rouvert de session derrière le dos du test.
    pub connexions: Arc<std::sync::atomic::AtomicUsize>,
    /// Partagé avec chaque session ouverte, pour les tests de bornage du flux.
    pub inondation: EtatInondation,
    /// Dernier nom d'utilisateur reçu par CETTE instance (auth par mot de passe
    /// ou clavier). Porté par serveur — non plus par un global partagé — pour
    /// qu'un test ne lise pas le nom posé par le serveur d'un test parallèle.
    /// Trouvé par l'audit du 8 septembre 2026.
    pub dernier_utilisateur: Arc<std::sync::Mutex<Option<String>>>,
    /// Octets reçus du client sur les canaux de session : les frappes d'un PTY.
    pub frappes: Arc<std::sync::Mutex<Vec<u8>>>,
    /// Levé quand la rafale d'un PTY ouvert en terminal « rafale » est émise.
    pub rafale_finie: Arc<std::sync::atomic::AtomicBool>,
    /// Canaux que le client a terminés (`CHANNEL_EOF` ou `CHANNEL_CLOSE`, compté
    /// une fois par canal), tous types confondus. Un canal SFTP refermé par
    /// le client n'envoie que `EOF` : le serveur répond alors `CLOSE` lui-même,
    /// et le `CLOSE` du client qui suit ne lui parvient plus comme tel.
    pub fermetures: Arc<std::sync::atomic::AtomicUsize>,
    /// Sous-systèmes SFTP ouverts par le client.
    pub sous_systemes_sftp: Arc<std::sync::atomic::AtomicUsize>,
    /// Commandes de dépôt de clé (`authorized_keys`) déjà reçues : la même,
    /// reçue une seconde fois, répond « déjà présente ».
    pub depots_de_cle: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
}

impl russh::server::Server for TestSshServer {
    type Handler = TestSshSession;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self::Handler {
        self.connexions
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        TestSshSession {
            inondation: self.inondation.clone(),
            dernier_utilisateur: self.dernier_utilisateur.clone(),
            frappes: self.frappes.clone(),
            rafale_finie: self.rafale_finie.clone(),
            fermetures: self.fermetures.clone(),
            sous_systemes_sftp: self.sous_systemes_sftp.clone(),
            depots_de_cle: self.depots_de_cle.clone(),
            ..Default::default()
        }
    }
}

#[derive(Default)]
pub struct TestSshSession {
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
    frappes: Arc<std::sync::Mutex<Vec<u8>>>,
    rafale_finie: Arc<std::sync::atomic::AtomicBool>,
    fermetures: Arc<std::sync::atomic::AtomicUsize>,
    sous_systemes_sftp: Arc<std::sync::atomic::AtomicUsize>,
    depots_de_cle: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    /// Canaux de CETTE connexion déjà comptés dans `fermetures`.
    canaux_finis: std::collections::HashSet<ChannelId>,
}

impl TestSshSession {
    /// Compte la fin d'un canal par le client, une fois par canal.
    fn noter_fin(&mut self, canal: ChannelId) {
        if self.canaux_finis.insert(canal) {
            self.fermetures
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// Dépôt de clé (`keys::deploy_command`) : le serveur répond le SEUL
    /// marqueur, sans écho de la commande. L'écho générique (`CMD:<commande>`)
    /// contient les deux littéraux `AVASH_AJOUTEE` et `AVASH_DEJA_PRESENTE` de
    /// la commande, et `interpret_deploy` aurait conclu « installée » pour la
    /// mauvaise raison (piège relevé par l'audit du 12 septembre 2026,
    /// C-couv-3). Première fois : ajoutée ; même commande ensuite : déjà
    /// présente ; compte « sans-droit » : refus du shell distant, code 3.
    fn repondre_depot_de_cle(&self, cmd: &str, channel_id: ChannelId, session: &mut Session) {
        let (sortie, code) = if self.user.as_deref() == Some("sans-droit") {
            (
                "mkdir: cannot create directory '.ssh': Permission denied\r\n".to_owned(),
                3,
            )
        } else if self.depots_de_cle.lock().unwrap().insert(cmd.to_owned()) {
            ("AVASH_AJOUTEE\r\n".to_owned(), 0)
        } else {
            ("AVASH_DEJA_PRESENTE\r\n".to_owned(), 0)
        };
        let _ = session.data(channel_id, bytes::Bytes::from(sortie.into_bytes()));
        let _ = session.eof(channel_id);
        let _ = session.exit_status_request(channel_id, code);
        let _ = session.close(channel_id);
    }
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
        if cmd.contains("authorized_keys") {
            self.repondre_depot_de_cle(&cmd, channel_id, session);
            let _ = channel;
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
        // Marqueur de test « SIGNAL_SANS_CLOTURE » : le serveur annonce la mort
        // du processus par `exit-signal` (RFC 4254 §6.10) puis GARDE le canal
        // ouvert et continue d'émettre, comme un serveur dont le scp a été tué
        // alors que le canal exec, lui, n'est pas refermé. Sert à vérifier que
        // le client ferme le canal sur CE chemin de sortie aussi, et pas
        // seulement au plafond ou à l'annulation. Audit du 9 septembre 2026.
        if cmd.contains("SIGNAL_SANS_CLOTURE") {
            let _ = session.exit_signal_request(channel_id, russh::Sig::KILL, false, "", "");
            let handle = session.handle();
            let etat = self.inondation.clone();
            tokio::spawn(async move {
                use std::sync::atomic::Ordering;
                let bloc = bytes::Bytes::from(vec![b'Z'; 256]);
                while !etat.close_recu.load(Ordering::SeqCst) {
                    if handle.data(channel_id, bloc.clone()).await.is_err() {
                        break; // session terminée
                    }
                    etat.octets.fetch_add(bloc.len() as u64, Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                // Tant qu'on émet, le canal reste vivant : le lâcher enverrait
                // CLOSE au client et masquerait ce qu'on veut observer.
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
        // Marqueur de test : simule une commande tuée par un signal côté
        // distant (OOM-killer, `kill -9`). RFC 4254 §6.10 : le serveur envoie
        // alors `exit-signal` et JAMAIS `exit-status`. Sert à vérifier que
        // `run`/`run_borne` ne prennent pas ce cas pour un succès.
        if cmd.contains("TUE_PAR_SIGNAL") {
            let _ = session.exit_signal_request(channel_id, russh::Sig::KILL, false, "", "");
        }
        // Marqueur de test : simule une commande interrompue — canal fermé SANS
        // exit-status (lien coupé, processus tué). Sert à vérifier que
        // `run_avec_agent` ne prend pas ce silence pour un succès.
        else if !cmd.contains("SANS_STATUT") {
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
        // Terminal « rafale » : `RAFALE_PTY` petits blocs d'un coup, pour
        // saturer le canal de sortie du client (contrat K6 de l'audit du
        // 12 septembre 2026). Émis depuis une tâche : le gestionnaire tourne
        // dans la boucle de la session serveur, qu'il ne faut pas bloquer.
        if term == "rafale" {
            let handle = session.handle();
            let finie = self.rafale_finie.clone();
            tokio::spawn(async move {
                for i in 0..RAFALE_PTY {
                    let bloc = bytes::Bytes::from(format!("r{i:04}\r\n").into_bytes());
                    if handle.data(channel_id, bloc).await.is_err() {
                        return;
                    }
                }
                finie.store(true, std::sync::atomic::Ordering::SeqCst);
            });
        }
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
        self.frappes.lock().unwrap().extend_from_slice(data);
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
        self.noter_fin(channel_id);
        self.channels.lock().await.remove(&channel_id);
        let _ = session.close(channel_id);
        Ok(())
    }

    /// Le client ferme un canal. On le note pour les tests de bornage du flux :
    /// c'est la preuve que le client a envoyé `CHANNEL_CLOSE` (et pas seulement
    /// lâché son canal), et le signal d'arrêt de la tâche d'inondation.
    async fn channel_close(
        &mut self,
        channel_id: ChannelId,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.inondation
            .close_recu
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.noter_fin(channel_id);
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
            self.sous_systemes_sftp
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
pub static SFTP_OPS: Mutex<Vec<String>> = Mutex::const_new(Vec::new());
/// Fichiers retirés du système de fichiers en mémoire (`remove` sous `/fs`) :
/// permet à un test d'affirmer qu'une cible n'a JAMAIS disparu, pas seulement
/// qu'elle est là à la fin.
pub static FS_SUPPRESSIONS: Mutex<Vec<String>> = Mutex::const_new(Vec::new());

pub fn ok_status(id: u32) -> Status {
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
pub struct TestSftpSession {
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
pub static FS_FICHIERS: std::sync::Mutex<Option<std::collections::HashMap<String, Vec<u8>>>> =
    std::sync::Mutex::new(None);
/// Dossiers en mémoire : chemins absolus.
pub static FS_DOSSIERS: std::sync::Mutex<Option<std::collections::HashSet<String>>> =
    std::sync::Mutex::new(None);
/// Liens symboliques en mémoire : chemin absolu du lien → longueur de la cible
/// (la « taille » qu'un `lstat` rend pour un lien). Ces entrées apparaissent
/// dans `readdir` avec des permissions `0o120xxx` (`S_IFLNK`) mais n'ont ni octets
/// dans `FS_FICHIERS` ni dossier dans `FS_DOSSIERS` : `stat` et `open` échouent
/// dessus (lien cassé), ce qui reproduit le cas de l'audit du 7 septembre 2026.
pub static FS_LIENS: std::sync::Mutex<Option<std::collections::HashMap<String, u64>>> =
    std::sync::Mutex::new(None);
/// Requêtes STAT servies sous /fs/ (chemin) : de quoi compter les allers-retours
/// qu'un transfert coûte par fichier (audit du 12 septembre 2026, C-perf-3).
pub static FS_STATS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
/// Ouvertures (vrai) et fermetures (faux) de descripteurs de fichiers sous
/// /fs/, dans l'ordre où le serveur les a servies : de quoi mesurer combien de
/// fichiers un transfert de dossier tient ouverts à la fois (C-perf-3).
pub static FS_DESCRIPTEURS: std::sync::Mutex<Vec<(String, bool)>> =
    std::sync::Mutex::new(Vec::new());

/// Nombre de STAT servis pour `chemin`.
pub fn fs_stats_de(chemin: &str) -> usize {
    FS_STATS
        .lock()
        .unwrap()
        .iter()
        .filter(|c| *c == chemin)
        .count()
}

/// Le plus grand nombre de descripteurs ouverts à la fois sous `prefixe`.
pub fn fs_max_ouverts_sous(prefixe: &str) -> usize {
    let mut ouverts = 0usize;
    let mut max = 0usize;
    for (_, ouvre) in FS_DESCRIPTEURS
        .lock()
        .unwrap()
        .iter()
        .filter(|(c, _)| c.starts_with(prefixe))
    {
        if *ouvre {
            ouverts += 1;
            max = max.max(ouverts);
        } else {
            ouverts = ouverts.saturating_sub(1);
        }
    }
    max
}

/// Lectures servies sous /fs/ : (chemin, décalage).
pub static FS_LECTURES: std::sync::Mutex<Vec<(String, u64)>> = std::sync::Mutex::new(Vec::new());

pub fn fs_fichiers(
) -> std::sync::MutexGuard<'static, Option<std::collections::HashMap<String, Vec<u8>>>> {
    let mut g = FS_FICHIERS.lock().unwrap();
    if g.is_none() {
        *g = Some(std::collections::HashMap::new());
    }
    g
}
pub fn fs_dossiers() -> std::sync::MutexGuard<'static, Option<std::collections::HashSet<String>>> {
    let mut g = FS_DOSSIERS.lock().unwrap();
    if g.is_none() {
        let mut s = std::collections::HashSet::new();
        s.insert("/fs".to_owned());
        *g = Some(s);
    }
    g
}
/// Écrit un fichier en mémoire (et ses dossiers parents), depuis un test.
pub fn fs_poser(chemin: &str, contenu: &[u8]) {
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
pub fn fs_liens() -> std::sync::MutexGuard<'static, Option<std::collections::HashMap<String, u64>>>
{
    let mut g = FS_LIENS.lock().unwrap();
    if g.is_none() {
        *g = Some(std::collections::HashMap::new());
    }
    g
}
/// Pose un lien symbolique cassé (sans cible servable) dans un dossier, depuis
/// un test : il sera listé mais ni ouvrable ni statable.
pub fn fs_poser_lien(chemin: &str, taille_cible: u64) {
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
pub fn fs_lire(chemin: &str) -> Option<Vec<u8>> {
    fs_fichiers().as_ref().unwrap().get(chemin).cloned()
}
pub fn fs_est_dossier(chemin: &str) -> bool {
    fs_dossiers().as_ref().unwrap().contains(chemin)
}
pub fn fs_lectures_de(chemin: &str) -> Vec<u64> {
    FS_LECTURES
        .lock()
        .unwrap()
        .iter()
        .filter(|(c, _)| c == chemin)
        .map(|(_, o)| *o)
        .collect()
}
pub fn fs_oublier_lectures() {
    FS_LECTURES.lock().unwrap().clear();
}
pub fn sous_fs(chemin: &str) -> bool {
    chemin == "/fs" || chemin.starts_with("/fs/")
}
pub fn parent_de(chemin: &str) -> &str {
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
                // Un fichier au nom « refus » ne s'ouvre pas (droits) : de quoi
                // éprouver l'arrêt d'un transfert de dossier sur une erreur.
                if filename.contains("refus") {
                    return Err(StatusCode::PermissionDenied);
                }
                let existe = fs_lire(&filename).is_some();
                if !existe && !pflags.contains(OpenFlags::CREATE) {
                    return Err(StatusCode::NoSuchFile);
                }
                if !existe || pflags.contains(OpenFlags::TRUNCATE) {
                    fs_poser(&filename, b"");
                }
                FS_DESCRIPTEURS
                    .lock()
                    .unwrap()
                    .push((filename.clone(), true));
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
        handle: String,
    ) -> impl Future<Output = Result<Status, Self::Error>> + Send {
        async move {
            if let Some(chemin) = handle.strip_prefix("fs:") {
                FS_DESCRIPTEURS
                    .lock()
                    .unwrap()
                    .push((chemin.to_owned(), false));
            }
            Ok(ok_status(id))
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
                FS_SUPPRESSIONS.lock().await.push(filename.clone());
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
                // Relecture de l'audit du 9 septembre 2026 : ce serveur renommait
                // à la POSIX, donc il remplaçait une cible existante sans un mot.
                // OpenSSH refuse `SSH_FXP_RENAME` dans ce cas, et c'est ce refus
                // qui fait passer la promotion d'un `.part` par sa seconde
                // branche, celle qu'empruntent les vrais serveurs, et qu'aucun
                // test n'éprouvait tant que ce serveur-ci était complaisant.
                // Sous un segment « posix », ce serveur remplace la cible comme
                // un `rename(2)` : c'est l'autre branche de la promotion du
                // partiel, celle du renommage direct, qu'il faut aussi éprouver.
                if newpath.contains("/posix/") {
                    g.as_mut().unwrap().remove(&newpath);
                } else if g.as_ref().unwrap().contains_key(&newpath) {
                    return Err(StatusCode::Failure);
                }
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
                FS_STATS.lock().unwrap().push(path.clone());
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
pub static GROS_FICHIER: std::sync::LazyLock<Vec<u8>> =
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
pub static CLE_HOTE: std::sync::LazyLock<PrivateKey> = std::sync::LazyLock::new(|| {
    PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap()
});

/// Démarre le serveur SSH de test sur un port libre, retourne le port.
pub async fn spawn_test_sshd() -> u16 {
    spawn_test_sshd_compte().await.0
}

/// Blocs émis d'un coup par un PTY ouvert en terminal « rafale ». Le client
/// tient 256 blocs dans le canal de sortie d'un PTY, en garde un en attente,
/// et russh en tamponne 100 par canal : 300 sature le canal de sortie sans
/// remplir le tampon de russh, donc sans bloquer la session. C'est l'état où
/// une frappe doit encore passer (contrat K6 de l'audit du 12 septembre 2026).
pub const RAFALE_PTY: usize = 300;

/// Un serveur de test lancé : son port, et de quoi observer ce qu'il a vu.
pub struct ServeurDeTest {
    pub port: u16,
    pub connexions: Arc<std::sync::atomic::AtomicUsize>,
    pub inondation: EtatInondation,
    pub dernier_utilisateur: Arc<std::sync::Mutex<Option<String>>>,
    pub frappes: Arc<std::sync::Mutex<Vec<u8>>>,
    pub rafale_finie: Arc<std::sync::atomic::AtomicBool>,
    pub fermetures: Arc<std::sync::atomic::AtomicUsize>,
    pub sous_systemes_sftp: Arc<std::sync::atomic::AtomicUsize>,
}

/// Démarre un serveur de test sur un port libre de 127.0.0.1.
pub async fn lancer_serveur() -> ServeurDeTest {
    let config = Arc::new(russh::server::Config {
        keys: vec![CLE_HOTE.clone()],
        ..Default::default()
    });
    let mut server = TestSshServer::default();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let observe = ServeurDeTest {
        port: listener.local_addr().unwrap().port(),
        connexions: server.connexions.clone(),
        inondation: server.inondation.clone(),
        dernier_utilisateur: server.dernier_utilisateur.clone(),
        frappes: server.frappes.clone(),
        rafale_finie: server.rafale_finie.clone(),
        fermetures: server.fermetures.clone(),
        sous_systemes_sftp: server.sous_systemes_sftp.clone(),
    };
    tokio::spawn(async move {
        let _ = server.run_on_socket(config, &listener).await;
    });
    observe
}

/// Comme `spawn_test_sshd`, avec le compteur de connexions de ce serveur.
pub async fn spawn_test_sshd_compte() -> (u16, Arc<std::sync::atomic::AtomicUsize>) {
    let s = lancer_serveur().await;
    (s.port, s.connexions)
}

/// Comme `spawn_test_sshd`, avec le handle « dernier utilisateur reçu » de CE
/// serveur : un test qui l'observe ne lit plus le nom posé par un autre serveur
/// de test tournant en parallèle.
pub async fn spawn_test_sshd_observe() -> (u16, Arc<std::sync::Mutex<Option<String>>>) {
    let s = lancer_serveur().await;
    (s.port, s.dernier_utilisateur)
}

/// Comme `spawn_test_sshd`, avec l'état d'inondation partagé de ce serveur,
/// pour les tests de bornage du flux (marqueurs « INONDE » / « GOUTTE »).
pub async fn spawn_test_sshd_inondation() -> (u16, EtatInondation) {
    let s = lancer_serveur().await;
    (s.port, s.inondation)
}

/// Répertoire personnel virtuel, pour ne pas toucher au `known_hosts` réel.
///
/// `/tmp` était codé en dur, et `HOME` seul n'isole rien sous Windows — où
/// `dirs::home_dir()` interroge le dossier de profil du système. On passe par
/// le répertoire temporaire de la plateforme et l'on pose aussi `AVASH_HOME`,
/// que le cœur honore partout.
pub fn virtual_home() -> std::path::PathBuf {
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
pub static CLE_TEST: std::sync::LazyLock<std::path::PathBuf> = std::sync::LazyLock::new(|| {
    let path = virtual_home().join("id_ed25519");
    let key = PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
    let mut buf = Vec::new();
    russh::keys::encode_pkcs8_pem(&key, &mut buf).unwrap();
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, buf).unwrap();
    std::fs::rename(&tmp, &path).unwrap();
    path
});

pub fn temp_key_path() -> std::path::PathBuf {
    CLE_TEST.clone()
}
