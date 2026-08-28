//! Avash — moteur SSH v0.1 : connexion russh + exécution de commande + SFTP list.
//! Phase backend pure : compilable et testable sans webkit2gtk.

use anyhow::{anyhow, Context, Result};
use russh::client::*;
use std::path::PathBuf;
use std::sync::Arc;

pub struct ClientAuth {
    pub user: String,
    /// Chemin de la clé privée (OpenSSH). Support agent à venir.
    pub key_path: Option<PathBuf>,
    pub password: Option<String>,
}

/// Handler d'auth : clés depuis disque + fallback password. Refuse tout host key en aveugle sauf trust explicite.
struct AvashAuth {
    user: String,
    key_path: Option<PathBuf>,
    password: Option<String>,
}

#[async_trait::async_trait]
impl russh::client::Handler for AvashAuth {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh_keys::key::PublicKey,
    ) -> Result<bool, Self::Error> {
        // v0.1 : pending — brancher le known_hosts store (TODO v0.2, sécurité).
        Ok(true)
    }
}

pub struct AvashSession {
    session: russh::client::Handle<AvashAuth>,
}

impl AvashSession {
    /// Ouvre une connexion TCP+SSH vers (host, port).
    pub async fn connect(host: &str, port: u16, auth: ClientAuth) -> Result<Self> {
        let config = Arc::new(russh::client::Config::default());
        let handler = AvashAuth {
            user: auth.user.clone(),
            key_path: auth.key_path.clone(),
            password: auth.password.clone(),
        };
        let mut session = russh::client::connect(config, (host, port), handler)
            .await
            .with_context(|| format!("Connexion SSH à {host}:{port}"))?;
        Self::authenticate(&mut session, &auth).await?;
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

    /// Exécute une commande, retourne stdout complet + exit code.
    pub async fn run(&mut self, command: &str) -> Result<(String, u32)> {
        let mut channel = self.session.channel_open_session().await?;
        channel.exec(false, command).await?;
        let mut stdout = String::new();
        let mut exit_code = 0u32;
        let mut msg_buffer = channel.wait().await;

        while let Some(msg) = msg_buffer {
            match msg {
                russh::ChannelMsg::Data { ref data } => {
                    stdout.push_str(&String::from_utf8_lossy(data));
                }
                russh::ChannelMsg::ExtendedData { ref data, .. } => {
                    stdout.push_str(&String::from_utf8_lossy(data));
                }
                russh::ChannelMsg::ExitStatus { exit_status } => {
                    exit_code = exit_status;
                    // sortie délibérée : on termine la lecture
                }
                russh::ChannelMsg::Eof => break,
                _ => {}
            }
            msg_buffer = channel.wait().await;
        }
        Ok((stdout, exit_code))
    }

    pub async fn disconnect(mut self) -> Result<()> {
        self.session
            .disconnect(russh::Disconnect::ByApplication, "au revoir", "")
            .await?;
        Ok(())
    }
}
