//! Avash — parseur ~/.ssh/config v0.1, avec serialisation pour le front.

pub mod keys;
pub mod sftp;
pub mod ssh;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SshHost {
    pub alias: String,
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
    pub proxy_jump: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

pub fn ssh_config_path() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".ssh/config")
}

pub fn parse_ssh_config() -> anyhow::Result<Vec<SshHost>> {
    let path = ssh_config_path();
    let content = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("Impossible de lire {}: {e}", path.display()))?;
    Ok(parse_config_str(&content))
}

pub fn parse_config_str(content: &str) -> Vec<SshHost> {
    let mut hosts: Vec<SshHost> = Vec::new();
    let mut current: Option<SshHost> = None;

    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = match line.split_once(char::is_whitespace) {
            Some((k, v)) => (k.to_lowercase(), v.trim().to_string()),
            None => (line.to_lowercase(), String::new()),
        };

        match key.as_str() {
            "host" => {
                if let Some(h) = current.take() {
                    hosts.push(h);
                }
                current = Some(SshHost {
                    alias: value.to_string(),
                    ..Default::default()
                });
            }
            _ => {
                if let Some(h) = current.as_mut() {
                    match key.as_str() {
                        "hostname" => h.hostname = Some(value),
                        "user" => h.user = Some(value),
                        "port" => h.port = value.parse().ok(),
                        "identityfile" => h.identity_file = Some(value),
                        "proxyjump" => h.proxy_jump = Some(value),
                        _ => {}
                    }
                }
            }
        }
    }
    if let Some(h) = current.take() {
        hosts.push(h);
    }

    let mut expanded = Vec::new();
    for h in hosts {
        for alias in h.alias.split_whitespace() {
            expanded.push(SshHost {
                alias: alias.to_string(),
                ..h.clone()
            });
        }
    }
    expanded.retain(|h| !h.alias.contains('*') && !h.alias.starts_with('!'));
    expanded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_config() {
        let cfg = r#"
# commentaire
Host web
    HostName 10.0.0.5
    User adrien
    Port 2222
    IdentityFile ~/.ssh/id_ed25519

Host db bastion
    HostName 10.0.0.9
    User root
"#;
        let hosts = parse_config_str(cfg);
        assert_eq!(hosts.len(), 3);
        assert_eq!(hosts[0].alias, "web");
        assert_eq!(hosts[0].port, Some(2222));
        assert_eq!(hosts[1].alias, "db");
        assert_eq!(hosts[2].alias, "bastion");
        assert_eq!(hosts[2].user, Some("root".into()));
    }

    #[test]
    fn skips_wildcards() {
        let cfg = "Host db*\n  User admin\nHost prod-1\n  User root";
        let hosts = parse_config_str(cfg);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "prod-1");
    }
}
