//! Clés SSH : lister, générer, déployer.

use super::Target;
use avash::ssh::AvashSession;
use avash::SshHost;

/// Liste les clés de `~/.ssh` utilisables pour un déploiement.
#[tauri::command]
pub fn keys_list() -> Result<Vec<avash::keys::KeyEntry>, String> {
    avash::keys::list_keys().map_err(|e| format!("{e:#}"))
}

/// Génère une paire ed25519 dans `~/.ssh`.
#[tauri::command]
pub fn key_generate(
    name: String,
    comment: Option<String>,
) -> Result<avash::keys::KeyEntry, String> {
    let comment = comment.unwrap_or_else(|| {
        // Un commentaire par defaut identifie la machine d'origine dans
        // authorized_keys — utile quand on revoque des annees plus tard.
        format!(
            "{}@{}",
            avash::ssh::current_username(),
            whoami::hostname().unwrap_or_else(|_| "avash".into())
        )
    });
    avash::keys::generate(&name, &comment).map_err(|e| format!("{e:#}"))
}

/// Installe une clé publique dans l'`authorized_keys` d'un serveur.
///
/// C'est l'équivalent de `ssh-copy-id` : on se connecte une dernière fois par
/// mot de passe, on ajoute la clé, et les connexions suivantes s'en passent.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn key_deploy(
    addr: String,
    port: Option<u16>,
    user: String,
    password: String,
    public_line: String,
) -> Result<String, String> {
    let cmd = avash::keys::deploy_command(&public_line).map_err(|e| format!("{e:#}"))?;
    // Le deploiement se fait forcement par mot de passe : si la cle etait
    // deja acceptee, l'operation n'aurait pas lieu d'etre.
    let target = Target::manual(addr, port, user, Some(password), None)?;
    let mut session = AvashSession::connect(&target.addr, target.port, &target.auth())
        .await
        .map_err(|e| format!("{e:#}"))?;
    let (stdout, code) = session.run(&cmd).await.map_err(|e| format!("{e:#}"))?;
    let _ = session.disconnect().await;
    if code != 0 {
        return Err(format!(
            "Le serveur a refusé l'installation (code {code}) : {}",
            stdout.trim()
        ));
    }
    avash::keys::interpret_deploy(&stdout)
        .map(std::string::ToString::to_string)
        .map_err(|e| format!("{e:#}"))
}

/// Enregistre une connexion manuelle dans `~/.ssh/config`.
///
/// L'hôte devient alors utilisable avec `ssh`, `scp`, `rsync` — pas seulement
/// dans Avash. Le mot de passe n'est jamais écrit : ce fichier est en clair.
#[tauri::command]
pub fn host_save(
    alias: String,
    addr: String,
    port: Option<u16>,
    user: String,
    key_path: Option<String>,
    proxy_jump: Option<String>,
    tags: Option<String>,
) -> Result<SshHost, String> {
    let host = SshHost {
        alias: alias.trim().to_string(),
        hostname: Some(addr.trim().to_string()),
        user: Some(user.trim().to_string()).filter(|u| !u.is_empty()),
        port,
        identity_file: key_path
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty()),
        proxy_jump: proxy_jump
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty()),
        tags: tags
            .unwrap_or_default()
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect(),
        folder: String::new(),
    };
    avash::append_host(&host).map_err(|e| format!("{e:#}"))?;
    Ok(host)
}
