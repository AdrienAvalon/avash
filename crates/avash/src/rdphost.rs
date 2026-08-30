//! Connexions RDP enregistrees, dans `~/.config/avash/rdp.yaml`.
//!
//! `~/.ssh/config` est propre au SSH (OpenSSH ne connait pas le RDP) : les
//! bureaux distants ont donc leur propre fichier. Le mot de passe, lui, va
//! dans le trousseau systeme (comme pour SSH), sous un identifiant prefixe
//! `rdp:` pour ne pas entrer en collision avec un compte SSH du meme hote.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RdpHost {
    pub id: String,
    /// Libelle affiche ; a defaut, `user@host`.
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub width: u16,
    pub height: u16,
    /// Dossier de rangement Avash (ex. « prod/web »), vide = racine.
    #[serde(default)]
    pub folder: String,
}

impl RdpHost {
    #[must_use]
    pub fn new(name: &str, host: &str, port: u16, user: &str, width: u16, height: u16) -> Self {
        let host = host.trim().to_string();
        let user = user.trim().to_string();
        let name = {
            let n = name.trim();
            if n.is_empty() {
                format!("{user}@{host}")
            } else {
                n.to_string()
            }
        };
        Self {
            id: format!("r-{:016x}", rand::random::<u64>()),
            name,
            host,
            port,
            user,
            width,
            height,
            folder: String::new(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.host.trim().is_empty() {
            bail!("L'adresse du serveur RDP est vide.");
        }
        if self.user.trim().is_empty() {
            bail!("L'utilisateur RDP est vide.");
        }
        Ok(())
    }
}

/// Identifiant trousseau d'un compte RDP (distinct des comptes SSH).
#[must_use]
pub fn keyring_account(user: &str, host: &str, port: u16) -> String {
    format!("rdp:{}@{}:{}", user.trim(), host.trim(), port)
}

/// `~/.config/avash/rdp.yaml`
#[must_use]
pub fn hosts_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("avash")
        .join("rdp.yaml")
}

pub fn load_hosts() -> Result<Vec<RdpHost>> {
    load_hosts_from(&hosts_path())
}

/// Un fichier absent n'est pas une erreur : c'est l'etat initial.
pub fn load_hosts_from(path: &Path) -> Result<Vec<RdpHost>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("Lecture de {}", path.display())),
    };
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_yaml::from_str(&text).with_context(|| format!("{} est illisible", path.display()))
}

/// Ecriture atomique : un plantage en cours d'ecriture ne tronque pas le fichier.
pub fn save_hosts_to(path: &Path, hosts: &[RdpHost]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("yaml.tmp");
    std::fs::write(&tmp, serde_yaml::to_string(hosts)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn upsert_host_in(path: &Path, host: RdpHost) -> Result<Vec<RdpHost>> {
    host.validate()?;
    let mut all = load_hosts_from(path)?;
    match all.iter_mut().find(|h| h.id == host.id) {
        Some(slot) => *slot = host,
        None => all.push(host),
    }
    save_hosts_to(path, &all)?;
    Ok(all)
}

pub fn remove_host_in(path: &Path, id: &str) -> Result<Vec<RdpHost>> {
    let mut all = load_hosts_from(path)?;
    all.retain(|h| h.id != id);
    save_hosts_to(path, &all)?;
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        std::env::temp_dir().join(format!(
            "avash-rdp-{}-{:?}.yaml",
            std::process::id(),
            std::thread::current().id()
        ))
    }

    #[test]
    fn new_derive_le_nom_et_un_id_unique() {
        let a = RdpHost::new("", "10.0.0.1", 3389, "admin", 1280, 800);
        assert_eq!(a.name, "admin@10.0.0.1");
        let b = RdpHost::new("Prod", " 10.0.0.1 ", 3389, "admin", 1280, 800);
        assert_eq!(b.name, "Prod");
        assert_eq!(b.host, "10.0.0.1", "hote rogne");
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn keyring_account_prefixe_rdp() {
        assert_eq!(
            keyring_account("adm", "10.0.0.1", 3389),
            "rdp:adm@10.0.0.1:3389"
        );
    }

    #[test]
    fn validate_refuse_hote_ou_user_vide() {
        assert!(RdpHost::new("x", " ", 3389, "u", 1, 1).validate().is_err());
        assert!(RdpHost::new("x", "h", 3389, " ", 1, 1).validate().is_err());
        assert!(RdpHost::new("x", "h", 3389, "u", 1, 1).validate().is_ok());
    }

    #[test]
    fn persistance_aller_retour() {
        let p = temp();
        let _ = std::fs::remove_file(&p);
        assert!(load_hosts_from(&p).unwrap().is_empty());
        let a = RdpHost::new("A", "10.0.0.1", 3389, "u", 1280, 800);
        let mut b = RdpHost::new("B", "10.0.0.2", 3390, "v", 1920, 1080);
        upsert_host_in(&p, a.clone()).unwrap();
        let all = upsert_host_in(&p, b.clone()).unwrap();
        assert_eq!(all, vec![a.clone(), b.clone()]);
        assert_eq!(load_hosts_from(&p).unwrap(), all, "relecture identique");
        // Remplacement par id.
        b.width = 2560;
        let all = upsert_host_in(&p, b.clone()).unwrap();
        assert_eq!(all, vec![a.clone(), b.clone()]);
        let all = remove_host_in(&p, &a.id).unwrap();
        assert_eq!(all, vec![b]);
        let _ = std::fs::remove_file(&p);
    }
}
