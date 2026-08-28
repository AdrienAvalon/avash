//! Avash — SFTP v0.3 : list/download/upload via russh-sftp.
//! Ouverture de canal SFTP sur une session existante.

use anyhow::{anyhow, Context, Result};
use russh_sftp::client::SftpSession;
use serde::Serialize;
use std::path::{Path, PathBuf};

use super::ssh::AvashSession;

#[derive(Debug, Clone, Serialize)]
pub struct SftpEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<u64>,
}

/// Handle SFTP vivant — le front liste/téléverse via ces commandes.
pub struct SftpHandle {
    pub sftp: SftpSession,
}

impl SftpHandle {
    /// Ouvre le sous-système SFTP sur la session.
    pub async fn open(session: &mut AvashSession) -> Result<Self> {
        let channel = session.open_sftp_channel().await?;
        let sftp = SftpSession::new(channel.into_stream())
            .await
            .context("Ouverture sous-système SFTP")?;
        Ok(Self { sftp })
    }

    /// Liste un répertoire distant.
    pub async fn list(&self, path: &str) -> Result<Vec<SftpEntry>> {
        let entries = self
            .sftp
            .read_dir(path)
            .await
            .with_context(|| format!("Lecture répertoire distant {path}"))?;
        Ok(entries
            .into_iter()
            .map(|e| {
                let m = e.metadata();
                SftpEntry {
                    name: e.file_name(),
                    is_dir: e.file_type().is_dir(),
                    size: m.len(),
                    modified: m.mtime.map(|t| t as u64),
                }
            })
            .collect())
    }

    /// Télécharge un fichier distant → local.
    pub async fn download(&self, remote: &str, local: &Path) -> Result<u64> {
        let mut remote_file = self
            .sftp
            .open(remote)
            .await
            .with_context(|| format!("Ouverture distant {remote}"))?;
        let mut local_file = tokio::fs::File::create(local)
            .await
            .with_context(|| format!("Création local {}", local.display()))?;
        let n = tokio::io::copy(&mut remote_file, &mut local_file)
            .await
            .context("Copie download")?;
        Ok(n)
    }

    /// Téléverse un fichier local → distant.
    pub async fn upload(&self, local: &Path, remote: &str) -> Result<u64> {
        let mut local_file = tokio::fs::File::open(local)
            .await
            .with_context(|| format!("Ouverture local {}", local.display()))?;
        let mut remote_file = self
            .sftp
            .create(remote)
            .await
            .with_context(|| format!("Création distant {remote}"))?;
        let n = tokio::io::copy(&mut local_file, &mut remote_file)
            .await
            .context("Copie upload")?;
        Ok(n)
    }

    pub async fn close(self) -> Result<()> {
        self.sftp.close().await.map_err(|e| anyhow!(e))
    }
}

/// Résolveur de chemin de téléchargement local par défaut.
pub fn default_local_dir() -> PathBuf {
    dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}