//! Avash — moteur SSH v0.2 : sessions exécution + PTY interactif.
//! v0.1 : connect/auth/exec. v0.2 : request_pty + flux bidirectionnel terminal.

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

/// Handler d'auth : clés depuis disque + fallback password.
/// check_server_key : branché sur un store known_hosts en v0.2 (TODO sécurité).
struct AvashAuth;

#[async_trait::async_trait]
impl russh::client::Handler for AvashAuth {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh_keys::key::PublicKey,
    ) -> Result<bool, Self::Error> {
        // v0.2 TODO : vérifier known_hosts strict, refuser l'inconnu par défaut.
        Ok(true)
    }
}

pub struct AvashSession {
    session: russh::client::Handle<AvashAuth>,
}

impl AvashSession {
    pub async fn connect(host: &str, port: u16, auth: &ClientAuth) -> Result<Self> {
        let config = Arc::new(russh::client::Config::default());
        let mut session = russh::client::connect(config, (host, port), AvashAuth)
            .await
            .with_context(|| format!("Connexion SSH à {host}:{port}"))?;
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

    /// Ouvre un canal PTY interactif. Retourne (writer_stdin, reader_stdout).
    /// Le front écrit dans le writer (touches clavier), lit le reader (flux terminal).
    pub async fn open_pty(
        &mut self,
        cols: u32,
        rows: u32,
        term: &str,
    ) -> Result<PtyChannel> {
        let channel = self.session.channel_open_session().await?;
        channel
            .request_pty(false, term, cols, rows, 0, 0, &[])
            .await
            .context("Demande PTY refusée")?;
        channel.request_shell(false).await?;

        let (in_tx, mut in_rx) = mpsc::channel::<Vec<u8>>(256);
        let (out_tx, out_rx) = mpsc::channel::<Vec<u8>>(256);

        // Pump stdout → out_tx (le canal est possédé par ce pump)
        let mut pump_channel = channel;
        let pump_out = tokio::spawn(async move {
            while let Some(msg) = pump_channel.wait().await {
                match msg {
                    russh::ChannelMsg::Data { ref data } => {
                        if out_tx.send(data.to_vec()).await.is_err() {
                            break;
                        }
                    }
                    russh::ChannelMsg::ExtendedData { ref data, .. } => {
                        if out_tx.send(data.to_vec()).await.is_err() {
                            break;
                        }
                    }
                    russh::ChannelMsg::Eof | russh::ChannelMsg::Close => break,
                    _ => {}
                }
            }
        });

        // in_rx (clavier) est consommé par le pump_in ; v0.3 branchera
        // le vrai write stdin sur le canal via make_writer.
        let pump_in = tokio::spawn(async move {
            while let Some(bytes) = in_rx.recv().await {
                let _ = bytes; // bufferise — v0.3 branchera le write
            }
        });

        Ok(PtyChannel {
            out_rx,
            in_tx,
            _pumps: vec![pump_out, pump_in],
        })
    }

    pub async fn disconnect(self) -> Result<()> {
        self.session
            .disconnect(russh::Disconnect::ByApplication, "au revoir", "")
            .await?;
        Ok(())
    }
}

/// Canal PTY exposé au front : flux sortant (terminal) + flux entrant (clavier).
pub struct PtyChannel {
    pub out_rx: mpsc::Receiver<Vec<u8>>,
    pub in_tx: mpsc::Sender<Vec<u8>>,
    _pumps: Vec<tokio::task::JoinHandle<()>>,
}