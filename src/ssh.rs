//! Avash — moteur SSH v0.3 : sessions exécution + PTY interactif complet.
//! v0.1 : connect/auth/exec. v0.2 : request_pty.
//! v0.3 : write stdin réel, window_change (resize), known_hosts strict.

use anyhow::{anyhow, Context, Result};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct ClientAuth {
    pub user: String,
    /// Chemin de la clé privée (OpenSSH). Support agent à venir.
    pub key_path: Option<PathBuf>,
    pub password: Option<String>,
}

/// Raison d'un refus de cle d'hote, partagee entre le handler et l'appelant.
///
/// `check_server_key` ne peut renvoyer qu'un `russh::Error` sans message : sans
/// ce canal, l'utilisateur ne verrait qu'un "Unknown key" opaque alors que
/// c'est l'avertissement le plus important de l'application.
type HostKeyVerdict = Arc<std::sync::Mutex<Option<String>>>;

/// Handler d'auth + vérification known_hosts.
struct AvashAuth {
    host: String,
    port: u16,
    verdict: HostKeyVerdict,
}

#[async_trait::async_trait]
impl russh::client::Handler for AvashAuth {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh_keys::key::PublicKey,
    ) -> Result<bool, Self::Error> {
        // TOFU (Trust On First Use), avec la distinction que fait OpenSSH :
        // hôte inconnu  -> on apprend la clé (premier contact) ;
        // clé CHANGÉE   -> on refuse, sans jamais réapprendre en silence.
        match russh_keys::check_known_hosts(&self.host, self.port, server_public_key) {
            // Hôte connu, clé identique.
            Ok(true) => Ok(true),

            // Hôte inconnu : premier contact, on mémorise.
            Ok(false) => {
                russh_keys::learn_known_hosts(&self.host, self.port, server_public_key)
                    .map_err(|_| russh::Error::UnknownKey)?;
                Ok(true)
            }

            // La clé d'hôte a changé : réinstallation du serveur, ou interception.
            // Dans le doute on refuse — c'est à l'utilisateur de trancher.
            Err(russh_keys::Error::KeyChanged { line }) => {
                *self.verdict.lock().unwrap() = Some(format!(
                    "LA CLÉ D'HÔTE A CHANGÉ pour {}:{}.\n\n\
                     Soit le serveur a été réinstallé, soit quelqu'un intercepte \
                     la connexion. Avash refuse de se connecter.\n\n\
                     Si tu es certain que le serveur a changé légitimement, \
                     supprime la ligne {} de ~/.ssh/known_hosts.",
                    self.host, self.port, line
                ));
                Err(russh::Error::UnknownKey)
            }

            // known_hosts illisible ou autre erreur : on refuse aussi.
            Err(e) => {
                *self.verdict.lock().unwrap() =
                    Some(format!("Vérification de la clé d'hôte impossible : {e}"));
                Err(russh::Error::UnknownKey)
            }
        }
    }
}

pub struct AvashSession {
    session: russh::client::Handle<AvashAuth>,
}

impl AvashSession {
    pub async fn connect(host: &str, port: u16, auth: &ClientAuth) -> Result<Self> {
        let config = Arc::new(russh::client::Config::default());
        let verdict: HostKeyVerdict = Arc::new(std::sync::Mutex::new(None));
        let handler = AvashAuth {
            host: host.to_string(),
            port,
            verdict: verdict.clone(),
        };
        let mut session = match russh::client::connect(config, (host, port), handler).await {
            Ok(s) => s,
            Err(e) => {
                // Un refus de cle d'hote porte un message explicite : on le
                // remonte tel quel plutot que le "Unknown key" de russh.
                if let Some(reason) = verdict.lock().unwrap().take() {
                    return Err(anyhow!(reason));
                }
                return Err(e).with_context(|| format!("Connexion SSH à {host}:{port}"));
            }
        };
        Self::authenticate(&mut session, auth).await?;
        Ok(Self { session })
    }

    async fn authenticate(
        session: &mut russh::client::Handle<AvashAuth>,
        auth: &ClientAuth,
    ) -> Result<()> {
        if let Some(key_path) = &auth.key_path {
            let key_pair = russh_keys::load_secret_key(key_path, None)
                .with_context(|| format!("Chargement clé {}", key_path.display()))?;
            let auth_res = session
                .authenticate_publickey(&auth.user, Arc::new(key_pair))
                .await?;
            if auth_res {
                return Ok(());
            }
        }
        if let Some(password) = &auth.password {
            let auth_res = session.authenticate_password(&auth.user, password).await?;
            if auth_res {
                return Ok(());
            }
        }
        Err(anyhow!("Authentification échouée pour {}", auth.user))
    }

    /// Exécution one-shot : stdout + exit code.
    pub async fn run(&mut self, command: &str) -> Result<(String, u32)> {
        let mut channel = self.session.channel_open_session().await?;
        channel.exec(false, command).await?;
        let mut stdout = String::new();
        let mut exit_code = 0u32;

        while let Some(msg) = channel.wait().await {
            match msg {
                russh::ChannelMsg::Data { ref data } => {
                    stdout.push_str(&String::from_utf8_lossy(data));
                }
                russh::ChannelMsg::ExtendedData { ref data, .. } => {
                    stdout.push_str(&String::from_utf8_lossy(data));
                }
                russh::ChannelMsg::ExitStatus { exit_status } => exit_code = exit_status,
                russh::ChannelMsg::Eof => break,
                _ => {}
            }
        }
        Ok((stdout, exit_code))
    }

    /// Ouvre un canal PTY interactif.
    /// Le front écrit dans in_tx (touches clavier), lit out_rx (flux terminal),
    /// et appelle resize_tx pour window_change.
    pub async fn open_pty(&mut self, cols: u32, rows: u32, term: &str) -> Result<PtyChannel> {
        let channel = self.session.channel_open_session().await?;
        channel
            .request_pty(false, term, cols, rows, 0, 0, &[])
            .await
            .context("Demande PTY refusée")?;
        channel.request_shell(false).await?;

        // Le canal est partagé entre le pump de sortie et le writer d'entrée :
        // russh::Channel est clonable via son sender interne ? Non — mais on
        // dédouble : le stream into_stream() possède le canal. On garde donc
        // une approche à deux moitiés : wait() pour la sortie, data() pour l'entrée
        // n'est pas possible sur le même objet possédé. Solution russh idiomatique :
        // cloner le canal (russh 0.45 : Channel implémente Clone ? Non).
        // → On utilise une seule tâche qui possède le canal et traite via select.
        let (in_tx, mut in_rx) = mpsc::channel::<Vec<u8>>(256);
        let (out_tx, out_rx) = mpsc::channel::<Vec<u8>>(256);
        let (resize_tx, mut resize_rx) = mpsc::channel::<(u32, u32)>(16);

        let mut pump_channel = channel;
        let pump = tokio::spawn(async move {
            // Le resize est optionnel : sa fermeture ne doit pas tuer la session,
            // mais son bras select! doit etre desactive (voir plus bas).
            let mut resize_closed = false;
            loop {
                tokio::select! {
                    // Sortie du serveur → front
                    msg = pump_channel.wait() => {
                        match msg {
                            Some(russh::ChannelMsg::Data { ref data }) => {
                                if out_tx.send(data.to_vec()).await.is_err() { break; }
                            }
                            Some(russh::ChannelMsg::ExtendedData { ref data, .. }) => {
                                if out_tx.send(data.to_vec()).await.is_err() { break; }
                            }
                            Some(russh::ChannelMsg::Eof | russh::ChannelMsg::Close) | None => break,
                            Some(_) => {}
                        }
                    }
                    // Clavier du front → stdin serveur
                    maybe = in_rx.recv() => {
                        match maybe {
                            Some(bytes) => {
                                let mut cursor = std::io::Cursor::new(bytes);
                                if pump_channel.data(&mut cursor).await.is_err() { break; }
                            }
                            None => break,
                        }
                    }
                    // Resize du front → window_change.
                    // La garde `if !resize_closed` est indispensable : un canal
                    // ferme rend Ready(None) immediatement et sans fin, et ce
                    // bras ferait tourner la boucle a vide a 100 % de CPU.
                    // On desactive donc le bras plutot que d'ignorer le None.
                    maybe = resize_rx.recv(), if !resize_closed => {
                        match maybe {
                            Some((c, r)) => {
                                if pump_channel.window_change(c, r, 0, 0).await.is_err() { break; }
                            }
                            None => resize_closed = true,
                        }
                    }
                }
            }
            let _ = pump_channel.close().await;
        });

        Ok(PtyChannel {
            out_rx,
            in_tx,
            resize_tx,
            _pump: pump,
        })
    }

    pub async fn disconnect(self) -> Result<()> {
        self.session
            .disconnect(russh::Disconnect::ByApplication, "au revoir", "")
            .await?;
        Ok(())
    }

    /// Ouvre un canal dédié au sous-système SFTP (canal indépendant, session intacte).
    pub async fn open_sftp_channel(&mut self) -> Result<russh::Channel<russh::client::Msg>> {
        let channel = self.session.channel_open_session().await?;
        channel.request_subsystem(true, "sftp").await?;
        Ok(channel)
    }
}

/// Canal PTY exposé au front : sortie terminal + entrée clavier + resize.
pub struct PtyChannel {
    pub out_rx: mpsc::Receiver<Vec<u8>>,
    pub in_tx: mpsc::Sender<Vec<u8>>,
    pub resize_tx: mpsc::Sender<(u32, u32)>,
    _pump: tokio::task::JoinHandle<()>,
}
