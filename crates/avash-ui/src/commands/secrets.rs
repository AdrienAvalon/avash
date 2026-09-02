//! Mots de passe mémorisés, hôtes déclarés, verrous clavier, ouverture externe.

use super::{find_host, Target};
use avash::SshHost;

/// Mémorise un mot de passe dans le trousseau du système.
///
/// Jamais dans `~/.ssh/config` : ce fichier est en clair. Le trousseau
/// (`KWallet`, GNOME Keyring, Credential Manager, Trousseau macOS) gère le
/// chiffrement, le déverrouillage et la révocation.
/// Utilisateur effectif d'un hote pour la cle du trousseau.
///
/// Doit correspondre EXACTEMENT a ce que `Target::from_alias` utilise pour
/// *relire* le mot de passe : un hote sans directive `User` retombe sur
/// l'utilisateur courant. Sans cette resolution commune, un mot de passe
/// enregistre sous une cle et relu sous une autre ne serait jamais retrouve
/// (bug : « mémoriser » cassé pour tout hote sans `User`).
pub(crate) fn effective_user(user: Option<String>) -> String {
    user.map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
        .unwrap_or_else(avash::ssh::current_username)
}

#[tauri::command]
pub fn password_save(
    addr: String,
    port: Option<u16>,
    user: Option<String>,
    password: String,
) -> Result<(), String> {
    let id = avash::secrets::account_id(&effective_user(user), addr.trim(), port.unwrap_or(22));
    avash::secrets::save(&id, &password).map_err(|e| format!("{e:#}"))
}

/// Oublie la clé d'hôte mémorisée (`known_hosts`) après un changement légitime.
/// Le prochain contact réapprend la nouvelle clé (TOFU).
#[tauri::command]
pub fn known_hosts_forget(addr: String, port: Option<u16>) -> Result<usize, String> {
    avash::ssh::forget_host_key(addr.trim(), port.unwrap_or(22)).map_err(|e| format!("{e:#}"))
}

/// Oublie un mot de passe mémorisé.
#[tauri::command]
pub fn password_forget(
    addr: String,
    port: Option<u16>,
    user: Option<String>,
) -> Result<(), String> {
    let id = avash::secrets::account_id(&effective_user(user), addr.trim(), port.unwrap_or(22));
    avash::secrets::forget(&id).map_err(|e| format!("{e:#}"))
}

/// Un mot de passe est-il déjà mémorisé pour cet hôte ?
#[tauri::command]
#[must_use]
pub fn password_known(addr: String, port: Option<u16>, user: Option<String>) -> bool {
    let id = avash::secrets::account_id(&effective_user(user), addr.trim(), port.unwrap_or(22));
    avash::secrets::load(&id).is_some()
}

/// Ouvre une URL dans le navigateur du système, jamais dans la webview.
///
/// Un lien cliquable du terminal ne doit pas naviguer dans la fenêtre Avash :
/// celle-ci a accès à `invoke`. On délègue au système, et on n'ouvre que des
/// schémas sûrs.
#[tauri::command]
pub fn open_external(url: String) -> Result<(), String> {
    let url = url.trim();
    // Whitelist stricte : ni file://, ni javascript:, ni schéma inconnu.
    let ok = ["http://", "https://", "mailto:", "ftp://"]
        .iter()
        .any(|p| url.starts_with(p));
    if !ok {
        return Err(format!("Schéma d'URL non autorisé : {url}"));
    }
    open::that(url).map_err(|e| format!("Ouverture impossible : {e}"))
}

/// Supprime un hôte de `~/.ssh/config` et oublie son mot de passe mémorisé.
#[tauri::command]
pub fn host_delete(alias: String) -> Result<(), String> {
    // On résout la cible AVANT de supprimer (après, l'hôte n'existe plus et on
    // ne saurait plus quel identifiant du trousseau oublier)... mais on n'oublie
    // le secret qu'APRÈS le succès de la suppression. Dans l'autre ordre, un
    // hôte déclaré via `Include` — que remove_host ne sait pas retirer — faisait
    // perdre le mot de passe alors que l'hôte restait en place.
    let identifiant = Target::from_alias(&alias)
        .ok()
        .map(|t| avash::secrets::account_id(&t.user, &t.addr, t.port));
    avash::remove_host(&alias).map_err(|e| format!("{e:#}"))?;
    if let Some(id) = identifiant {
        let _ = avash::secrets::forget(&id);
    }
    Ok(())
}

/// Renvoie les champs d'un hôte pour pré-remplir le formulaire d'édition.
#[tauri::command]
pub fn host_get(alias: String) -> Result<SshHost, String> {
    find_host(&alias)
}

/// Modifie un hôte enregistré. Si l'alias change, le mot de passe mémorisé
/// est déplacé vers le nouvel identifiant.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn host_update(
    old_alias: String,
    alias: String,
    addr: String,
    port: Option<u16>,
    user: Option<String>,
    key_path: Option<String>,
    proxy_jump: Option<String>,
    tags: Option<String>,
    folder: Option<String>,
) -> Result<(), String> {
    let host = SshHost {
        alias: alias.trim().to_string(),
        hostname: Some(addr.trim().to_string()).filter(|a| !a.is_empty()),
        user: user.map(|u| u.trim().to_string()).filter(|u| !u.is_empty()),
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
        folder: avash::folders::normalize(&folder.unwrap_or_default()),
    };
    // Identifiant du trousseau AVANT modification : il dérive de user@addr:port,
    // pas de l'alias. Changer l'adresse ou l'utilisateur d'un hôte laissait donc
    // le secret sous l'ancien identifiant — redemandé à chaque connexion, sans
    // explication, l'ancienne entrée restant orpheline dans le trousseau.
    let ancien = Target::from_alias(old_alias.trim())
        .ok()
        .map(|t| avash::secrets::account_id(&t.user, &t.addr, t.port));

    avash::update_host(old_alias.trim(), &host).map_err(|e| format!("{e:#}"))?;

    // Après le succès seulement : on déplace le secret vers le nouvel identifiant.
    if let Some(ancien) = ancien {
        if let Some(nouveau) = Target::from_alias(host.alias.trim())
            .ok()
            .map(|t| avash::secrets::account_id(&t.user, &t.addr, t.port))
        {
            if nouveau != ancien {
                if let Some(secret) = avash::secrets::load(&ancien) {
                    // L'oubli n'a lieu qu'après une écriture réussie. Sinon —
                    // trousseau verrouillé, D-Bus absent — la nouvelle entrée
                    // n'existait pas, l'ancienne était quand même effacée, et
                    // `host_update` renvoyait Ok : le mot de passe était perdu
                    // sans un mot, pour un simple changement de port.
                    avash::secrets::save(&nouveau, &secret).map_err(|e| {
                        format!("Le mot de passe mémorisé n'a pas pu être déplacé : {e:#}")
                    })?;
                    let _ = avash::secrets::forget(&ancien);
                }
            }
        }
    }
    Ok(())
}
