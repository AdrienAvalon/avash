//! Avash — parseur ~/.ssh/config v0.1, avec serialisation pour le front.

#[cfg(test)]
pub(crate) mod testutil;

pub mod enregistrement;
pub mod folders;
pub mod import;
pub mod keys;
pub mod osinfo;
pub mod rdphost;
pub mod sante;
pub mod secrets;
pub mod serie;
pub mod sftp;
pub mod snippet;
pub mod ssh;
pub mod tunnel;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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
    /// Dossier de rangement Avash (ex. « prod/web »), vide = racine.
    #[serde(default)]
    pub folder: String,
}

#[must_use]
pub fn ssh_config_path() -> std::path::PathBuf {
    repertoire_personnel()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".ssh/config")
}

pub fn parse_ssh_config() -> anyhow::Result<Vec<SshHost>> {
    let path = ssh_config_path();
    let content = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("Impossible de lire {}: {e}", path.display()))?;
    let base = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    Ok(parse_config_str(&resolve_includes(&content, &base, 0)))
}

/// Profondeur maximale de resolution des `Include`.
///
/// OpenSSH s'arrete a 16 ; on fait de meme. Sans borne, deux fichiers qui
/// s'incluent mutuellement boucleraient indefiniment.
const MAX_INCLUDE_DEPTH: usize = 16;

/// Resout les directives `Include` et rend le contenu aplati.
///
/// Les chemins relatifs sont resolus depuis `~/.ssh`, comme le fait OpenSSH.
/// `~` est developpe. Les motifs (`config.d/*`) sont etendus par ordre
/// alphabetique. Un fichier illisible est ignore en silence : OpenSSH se
/// comporte ainsi, et une configuration partielle vaut mieux qu'aucune.
fn resolve_includes(content: &str, base: &Path, depth: usize) -> String {
    if depth >= MAX_INCLUDE_DEPTH {
        return content.to_string();
    }
    let mut out = String::with_capacity(content.len());
    for raw in content.lines() {
        let line = raw.trim();
        let is_include = line
            .split_once(char::is_whitespace)
            .is_some_and(|(k, _)| k.eq_ignore_ascii_case("include"));
        if !is_include {
            out.push_str(raw);
            out.push('\n');
            continue;
        }
        let Some((_, patterns)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        for pattern in patterns.split_whitespace() {
            for path in expand_include(pattern, base) {
                if let Ok(inner) = std::fs::read_to_string(&path) {
                    let parent = path.parent().unwrap_or(base).to_path_buf();
                    out.push_str(&resolve_includes(&inner, &parent, depth + 1));
                    out.push('\n');
                }
            }
        }
    }
    out
}

/// Developpe un motif d'`Include` en liste de fichiers existants.
fn expand_include(pattern: &str, base: &Path) -> Vec<PathBuf> {
    let expanded = if let Some(rest) = pattern.strip_prefix("~/") {
        repertoire_personnel()
            .unwrap_or_else(|| base.to_path_buf())
            .join(rest)
    } else if pattern.starts_with('/') {
        PathBuf::from(pattern)
    } else {
        base.join(pattern)
    };

    let s = expanded.to_string_lossy().into_owned();
    if !s.contains(['*', '?']) {
        return if expanded.is_file() {
            vec![expanded]
        } else {
            vec![]
        };
    }
    // Motif : on liste le repertoire parent et on filtre a la main plutot
    // que d'ajouter une dependance de glob pour ce seul usage.
    let (dir, pat) = match expanded.parent().zip(expanded.file_name()) {
        Some((d, f)) => (d.to_path_buf(), f.to_string_lossy().into_owned()),
        None => return vec![],
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return vec![];
    };
    let mut found: Vec<PathBuf> = entries
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| glob_match(&pat, &n.to_string_lossy()))
        })
        .collect();
    // Ordre stable : OpenSSH lit dans l'ordre lexicographique.
    found.sort();
    found
}

/// Correspondance de motif minimale : `*` et `?`, sans classes.
fn glob_match(pattern: &str, name: &str) -> bool {
    fn inner(p: &[u8], n: &[u8]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (Some(b'*'), _) => inner(&p[1..], n) || (!n.is_empty() && inner(p, &n[1..])),
            (Some(b'?'), Some(_)) => inner(&p[1..], &n[1..]),
            (Some(a), Some(b)) if a == b => inner(&p[1..], &n[1..]),
            _ => false,
        }
    }
    inner(pattern.as_bytes(), name.as_bytes())
}

pub fn parse_config_str(content: &str) -> Vec<SshHost> {
    let mut hosts: Vec<SshHost> = Vec::new();
    let mut current: Option<SshHost> = None;

    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        // Convention Avash : un commentaire `#Tags: a, b` DANS un bloc Host
        // etiquette l'hote. Reste un commentaire pour OpenSSH (ignore).
        if let Some(rest) = line.strip_prefix('#') {
            if let Some(list) = rest
                .trim_start()
                .strip_prefix("Tags:")
                .or_else(|| rest.trim_start().strip_prefix("tags:"))
            {
                if let Some(h) = current.as_mut() {
                    h.tags = list
                        .split(',')
                        .map(|t| t.trim().to_string())
                        .filter(|t| !t.is_empty())
                        .collect();
                }
            } else if let Some(path) = rest
                .trim_start()
                .strip_prefix("Folder:")
                .or_else(|| rest.trim_start().strip_prefix("folder:"))
            {
                if let Some(h) = current.as_mut() {
                    h.folder = path.trim().trim_matches('/').to_string();
                }
            }
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
                    alias: value.clone(),
                    ..Default::default()
                });
            }
            // Un bloc `Match` ferme le bloc `Host` courant. Sans cela ses
            // directives etaient attribuees au dernier hote rencontre : un
            // `Match exec ...` jamais satisfait pouvait ainsi changer
            // silencieusement l'utilisateur et le port d'un hote reel.
            //
            // Avash n'evalue pas les conditions de `Match` (elles dependent de
            // l'hote cible, de l'utilisateur courant, voire d'une commande) :
            // on ferme le bloc et on ignore ce qu'il contient, plutot que de
            // l'appliquer a tort.
            "match" => {
                if let Some(h) = current.take() {
                    hosts.push(h);
                }
                current = None;
            }
            _ => {
                if let Some(h) = current.as_mut() {
                    match key.as_str() {
                        "hostname" => h.hostname = Some(value),
                        "user" => h.user = Some(value),
                        // OpenSSH refuse « Port 0 » (« Bad port ») : le lire
                        // comme un port menait à une connexion vouée à l'échec
                        // sur un message opaque. Trouvé par le fuzzing.
                        "port" => h.port = value.parse::<u16>().ok().filter(|p| *p != 0),
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
    fn tags_lus_et_reecrits() {
        let cfg = "Host prod\n  HostName 10.0.0.1\n  #Tags: prod, web\n";
        let h = &parse_config_str(cfg)[0];
        assert_eq!(h.tags, vec!["prod", "web"]);
        // Round-trip : render puis relit les memes tags.
        let rendered = render_host_block(h);
        assert!(rendered.contains("#Tags: prod, web"), "{rendered}");
        assert_eq!(parse_config_str(&rendered)[0].tags, vec!["prod", "web"]);
    }

    #[test]
    fn tags_hors_bloc_host_ignores() {
        // Un #Tags avant tout Host ne s'attache a rien.
        let h = parse_config_str("#Tags: orphelin\nHost a\n  HostName x\n");
        assert!(h[0].tags.is_empty());
    }

    #[test]
    fn split_proxy_jump_decoupe_une_chaine() {
        let v = split_proxy_jump("bastion, deploy@10.0.0.1:2222");
        assert_eq!(v.len(), 2);
        assert_eq!(
            v[0],
            HopSpec {
                user: None,
                host: "bastion".into(),
                port: None
            }
        );
        assert_eq!(
            v[1],
            HopSpec {
                user: Some("deploy".into()),
                host: "10.0.0.1".into(),
                port: Some(2222)
            }
        );
    }

    #[test]
    fn split_proxy_jump_gere_none_et_vide() {
        assert!(split_proxy_jump("none").is_empty());
        assert!(split_proxy_jump("").is_empty());
        assert!(split_proxy_jump("  ,  ").is_empty());
    }

    #[test]
    fn parses_basic_config() {
        let cfg = r"
# commentaire
Host web
    HostName 10.0.0.5
    User adrien
    Port 2222
    IdentityFile ~/.ssh/id_ed25519

Host db bastion
    HostName 10.0.0.9
    User root
";
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

/// Ajoute un hote a `~/.ssh/config`.
///
/// On ecrit dans le fichier standard plutot que dans un format propre a
/// Avash : l'hote enregistre devient utilisable avec `ssh`, `scp`, `rsync`
/// et tout l'ecosysteme, pas seulement ici.
///
/// ⚠️ Aucun mot de passe n'est enregistre — ce fichier est en clair. Pour se
/// passer de saisie, la voie propre est de deployer une cle.
/// Un maillon d'une chaine `ProxyJump`, tel qu'ecrit dans `~/.ssh/config`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HopSpec {
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
}

/// Decoupe une valeur `ProxyJump` (`a,b`, `user@host:port`, un alias…) en
/// maillons, dans l'ordre. Ne resout rien : la resolution (alias -> hote)
/// se fait ensuite avec la config.
#[must_use]
pub fn split_proxy_jump(spec: &str) -> Vec<HopSpec> {
    spec.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty() && !t.eq_ignore_ascii_case("none"))
        .map(|token| {
            // Chaque morceau est rogné : `user @hote :port` n'est pas une
            // syntaxe valide, mais un fichier édité à la main peut la
            // contenir, et un espace collé au nom d'hôte rendait un rebond
            // introuvable sans que le message ne le laisse voir (trouvé par
            // le test de mutation).
            let (user, rest) = match token.split_once('@') {
                Some((u, r)) => (Some(u.trim()).filter(|u| !u.is_empty()), r.trim()),
                None => (None, token),
            };
            // `host:port` — on ne coupe que si la partie apres `:` est un port
            // (evite de casser une adresse IPv6 nue, rare en ProxyJump). Zéro
            // n'en est pas un : le morceau reste entier, et la résolution le
            // dira introuvable plutôt que de viser le port 0.
            let (host, port) = match rest.rsplit_once(':') {
                Some((h, p))
                    if !h.trim().is_empty() && p.trim().parse::<u16>().is_ok_and(|p| p != 0) =>
                {
                    (h.trim().to_string(), p.trim().parse::<u16>().ok())
                }
                _ => (rest.to_string(), None),
            };
            HopSpec {
                user: user.map(str::to_string),
                host,
                port,
            }
        })
        .filter(|h| !h.host.is_empty())
        .collect()
}

pub fn append_host(host: &SshHost) -> anyhow::Result<()> {
    use std::io::Write as _;

    let alias = host.alias.trim();
    validate_host(host)?;

    let path = ssh_config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("Création de {} : {e}", parent.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }

    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    // Unicité vérifiée sur la configuration COMPLÈTE, Include résolus : sinon on
    // ajoutait un second bloc pour un alias déjà déclaré dans un fichier inclus.
    // OpenSSH retenant la première occurrence, les modifications ultérieures
    // semblaient sans effet, et deux entrées identiques apparaissaient dans la
    // liste. On retombe sur le fichier principal si la résolution échoue.
    let deja_declare = parse_ssh_config().map_or_else(
        |_| {
            parse_config_str(&existing)
                .iter()
                .any(|h| h.alias.eq_ignore_ascii_case(alias))
        },
        |hotes| hotes.iter().any(|h| h.alias.eq_ignore_ascii_case(alias)),
    );
    if deja_declare {
        return Err(anyhow::anyhow!(
            "Un hôte « {alias} » est déjà déclaré dans votre configuration SSH."
        ));
    }

    let mut block = String::new();
    // Une ligne vide avant le bloc, sauf si le fichier est vide ou en finit
    // deja par une : sinon le `Host` se colle a la directive precedente et
    // en devient une sous-directive.
    if !existing.is_empty() && !existing.ends_with("\n\n") {
        if !existing.ends_with('\n') {
            block.push('\n');
        }
        block.push('\n');
    }
    block.push_str(&render_host_block(host));

    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| anyhow::anyhow!("Ouverture de {} : {e}", path.display()))?;
    f.write_all(block.as_bytes())
        .map_err(|e| anyhow::anyhow!("Écriture dans {} : {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Supprime un hôte de `~/.ssh/config`, en préservant tout le reste.
///
/// On retire le bloc `Host <alias>` et ses directives indentées, jusqu'au
/// prochain `Host`/`Match` ou la fin du fichier. Les commentaires et les
/// autres hôtes de l'utilisateur restent intacts.
pub fn remove_host(alias: &str) -> anyhow::Result<()> {
    let alias = alias.trim();
    if alias.is_empty() {
        return Err(anyhow::anyhow!("Nom d'hôte vide."));
    }
    let path = ssh_config_path();
    let content = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("Lecture de {} : {e}", path.display()))?;

    let mut out = String::with_capacity(content.len());
    let mut skipping = false;
    let mut removed = false;
    for line in content.lines() {
        let trimmed = line.trim_start();
        let (key, value) = trimmed
            .split_once(char::is_whitespace)
            .map_or((trimmed, ""), |(k, v)| (k, v.trim()));
        let key_lower = key.to_lowercase();

        if key_lower == "host" {
            // Un bloc Host commence : on saute celui qui matche exactement.
            // Les alias multiples (`Host a b`) : on ne retire que si l'alias
            // vise est le seul du bloc — sinon on toucherait aux autres.
            skipping = value.split_whitespace().eq(std::iter::once(alias));
            if skipping {
                removed = true;
                continue;
            }
        } else if key_lower == "match" {
            skipping = false;
        }
        if !skipping {
            out.push_str(line);
            out.push('\n');
        }
    }

    if !removed {
        return Err(anyhow::anyhow!(
            "Hôte « {alias} » introuvable dans {}.",
            path.display()
        ));
    }
    // Compacter les lignes vides en trop laissees par la suppression.
    while out.contains("\n\n\n") {
        out = out.replace("\n\n\n", "\n\n");
    }
    ecrire_atomiquement(&path, out.trim_start_matches('\n').as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Modifie un hôte de `~/.ssh/config` : remplace son bloc, en préservant sa
/// position et tout le reste du fichier.
///
/// Si l'alias change, l'ancien bloc est retiré et le nouveau écrit à la même
/// place. On refuse de renommer vers un alias déjà pris (hors l'hôte modifié
/// lui-même). Les blocs à alias multiples ne sont pas modifiables ici — même
/// raison que pour la suppression.
pub fn update_host(old_alias: &str, host: &SshHost) -> anyhow::Result<()> {
    let old_alias = old_alias.trim();
    validate_host(host)?;

    let path = ssh_config_path();
    let content = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("Lecture de {} : {e}", path.display()))?;

    // Renommage vers un alias existant (autre que celui qu'on modifie) : refus.
    if !host.alias.eq_ignore_ascii_case(old_alias)
        && parse_config_str(&content)
            .iter()
            .any(|h| h.alias.eq_ignore_ascii_case(host.alias.trim()))
    {
        return Err(anyhow::anyhow!(
            "Un hôte « {} » existe déjà.",
            host.alias.trim()
        ));
    }

    let mut out = String::with_capacity(content.len());
    let mut skipping = false;
    let mut replaced = false;
    for line in content.lines() {
        let trimmed = line.trim_start();
        let (key, value) = trimmed
            .split_once(char::is_whitespace)
            .map_or((trimmed, ""), |(k, v)| (k, v.trim()));
        let key_lower = key.to_lowercase();

        if key_lower == "host" {
            let is_target = value.split_whitespace().eq(std::iter::once(old_alias));
            if is_target {
                // Bloc cible : on ecrit le nouveau contenu a sa place.
                skipping = true;
                replaced = true;
                out.push_str(render_host_block(host).trim_end());
                out.push('\n');
                continue;
            }
            skipping = false;
        } else if key_lower == "match" {
            skipping = false;
        }
        if !skipping {
            out.push_str(line);
            out.push('\n');
        }
    }

    if !replaced {
        return Err(anyhow::anyhow!(
            "Hôte « {old_alias} » introuvable dans {}.",
            path.display()
        ));
    }
    while out.contains("\n\n\n") {
        out = out.replace("\n\n\n", "\n\n");
    }
    ecrire_atomiquement(&path, out.trim_start_matches('\n').as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Rend un bloc `Host` au format OpenSSH.
/// Vrai si la ligne est un commentaire `#Folder:` d'Avash.
fn is_folder_comment(line: &str) -> bool {
    line.trim_start().strip_prefix('#').is_some_and(|r| {
        let r = r.trim_start();
        r.strip_prefix("Folder:")
            .or_else(|| r.strip_prefix("folder:"))
            .is_some()
    })
}

/// Émet un bloc accumulé ; pour le bloc cible, retire l'ancienne ligne
/// `#Folder:` et insère la nouvelle après la dernière directive (avant les
/// éventuelles lignes vides de fin de bloc). Le reste est préservé tel quel.
fn flush_folder_block(out: &mut String, block: &mut Vec<String>, is_target: bool, folder: &str) {
    if is_target {
        block.retain(|l| !is_folder_comment(l));
        if !folder.is_empty() {
            let last = block.iter().rposition(|l| !l.trim().is_empty());
            let pos = last.map_or(block.len(), |i| i + 1);
            block.insert(pos, format!("    #Folder: {folder}"));
        }
    }
    for l in block.drain(..) {
        out.push_str(&l);
        out.push('\n');
    }
}

/// Range un hôte dans un dossier (commentaire `#Folder:`), en **place** :
/// seule la ligne `#Folder:` du bloc est ajoutée/remplacée/retirée, toutes les
/// autres directives sont préservées (contrairement à `update_host`).
///
/// # Errors
/// Si le fichier est illisible/inscriptible, ou l'alias introuvable.
pub fn set_host_folder(alias: &str, folder: &str) -> anyhow::Result<()> {
    set_host_folder_at(&ssh_config_path(), alias, folder)
}

/// Comme [`set_host_folder`], sur un chemin explicite (testable).
///
/// # Errors
/// Si le fichier est illisible/inscriptible, ou l'alias introuvable.
pub fn set_host_folder_at(path: &std::path::Path, alias: &str, folder: &str) -> anyhow::Result<()> {
    let alias = alias.trim();
    let folder = folder.trim().trim_matches('/');
    validate_config_value("Folder", folder)?;
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("Lecture de {} : {e}", path.display()))?;

    let mut out = String::with_capacity(content.len() + 32);
    let mut block: Vec<String> = Vec::new();
    let mut in_target = false;
    let mut found = false;

    for line in content.lines() {
        let trimmed = line.trim_start();
        let (key, value) = trimmed
            .split_once(char::is_whitespace)
            .map_or((trimmed, ""), |(k, v)| (k, v.trim()));
        let key_lower = key.to_lowercase();
        if key_lower == "host" || key_lower == "match" {
            flush_folder_block(&mut out, &mut block, in_target, folder);
            in_target = key_lower == "host" && value.split_whitespace().eq(std::iter::once(alias));
            if in_target {
                found = true;
            }
        }
        block.push(line.to_string());
    }
    flush_folder_block(&mut out, &mut block, in_target, folder);

    if !found {
        return Err(anyhow::anyhow!(
            "Hôte « {alias} » introuvable dans {}.",
            path.display()
        ));
    }
    ecrire_atomiquement(path, out.as_bytes())?;
    Ok(())
}

#[must_use]
pub fn render_host_block(host: &SshHost) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    let _ = writeln!(out, "Host {}", host.alias.trim());
    if let Some(v) = host.hostname.as_deref().filter(|v| !v.trim().is_empty()) {
        let _ = writeln!(out, "    HostName {}", v.trim());
    }
    if let Some(v) = host.user.as_deref().filter(|v| !v.trim().is_empty()) {
        let _ = writeln!(out, "    User {}", v.trim());
    }
    if let Some(p) = host.port.filter(|p| *p != 22) {
        let _ = writeln!(out, "    Port {p}");
    }
    if let Some(v) = host
        .identity_file
        .as_deref()
        .filter(|v| !v.trim().is_empty())
    {
        let _ = writeln!(out, "    IdentityFile {}", v.trim());
    }
    if let Some(v) = host.proxy_jump.as_deref().filter(|v| !v.trim().is_empty()) {
        let _ = writeln!(out, "    ProxyJump {}", v.trim());
    }
    let clean: Vec<&str> = host
        .tags
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .collect();
    if !clean.is_empty() {
        let _ = writeln!(out, "    #Tags: {}", clean.join(", "));
    }
    let folder = host.folder.trim().trim_matches('/');
    if !folder.is_empty() {
        let _ = writeln!(out, "    #Folder: {folder}");
    }
    out
}

/// Répertoire personnel de l'utilisateur, avec un point d'entrée unique.
///
/// Deux raisons de ne pas appeler `repertoire_personnel()` directement partout.
///
/// **La cohérence avec russh.** Sous Windows, `repertoire_personnel()` interroge
/// `SHGetKnownFolderPath(FOLDERID_Profile)` alors que `std::env::home_dir()` —
/// celui dont russh se sert pour `known_hosts` — consulte d'abord `USERPROFILE`.
/// Les deux peuvent différer : nous vérifiions alors un fichier pendant que
/// russh en lisait un autre, ce qui vide de sens la vérification de clé d'hôte.
/// Tout passe désormais par ici, et les chemins `known_hosts` sont donnés
/// explicitement à russh plutôt que laissés à sa propre résolution.
///
/// **L'isolation des tests.** Elle reposait sur le remplacement de `HOME`, que
/// Windows ignore : les tests y travaillaient sur le vrai profil, tous en
/// parallèle sur les mêmes fichiers. `AVASH_HOME` sert de dérogation explicite,
/// honorée sur toutes les plateformes. Ce n'est pas une porte dérobée : qui
/// peut poser une variable d'environnement dans le processus peut déjà
/// beaucoup plus.
#[must_use]
pub fn repertoire_personnel() -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os("AVASH_HOME") {
        let p = std::path::PathBuf::from(p);
        if !p.as_os_str().is_empty() {
            return Some(p);
        }
    }
    dirs::home_dir()
}

/// Répertoire de configuration d'Avash (`~/.config/avash` ou son équivalent).
///
/// Suit `AVASH_HOME` quand il est posé, pour que les tests isolent aussi les
/// fichiers d'état (dossiers, bureaux RDP, snippets, tunnels).
#[must_use]
pub fn repertoire_configuration() -> Option<std::path::PathBuf> {
    if std::env::var_os("AVASH_HOME").is_some() {
        return repertoire_personnel().map(|h| h.join(".config"));
    }
    dirs::config_dir()
}

/// Écrit un fichier de configuration **atomiquement** et sans fenêtre lisible.
///
/// Deux défauts corrigés d'un coup :
///
/// 1. `std::fs::write` tronque le fichier **puis** écrit. Une coupure entre les
///    deux — disque plein, arrêt brutal — laissait un fichier vide. Pour
///    `~/.ssh/config` c'est toute la configuration SSH de l'utilisateur, pas
///    seulement celle d'Avash, qui disparaissait sur un simple renommage de
///    dossier (une réécriture complète par hôte déplacé). Pour
///    `rdp_known_hosts` c'était pire qu'une perte : sans empreintes, chaque
///    serveur redevient un « premier contact » et tout certificat est réaccepté.
/// 2. Le temporaire naissait avec l'umask, souvent 0644, et n'était resserré
///    qu'après le renommage : `snippets.yaml` — qui contient des commandes
///    d'administration, parfois avec un jeton dedans — était brièvement lisible
///    par les autres comptes de la machine.
///
/// Le temporaire est créé dans le **même répertoire** que la cible, sans quoi
/// `rename` franchirait un point de montage et échouerait.
///
/// # Errors
/// Si le répertoire, l'écriture, la synchronisation ou le renommage échouent.
pub fn ecrire_atomiquement(path: &std::path::Path, contenu: &[u8]) -> anyhow::Result<()> {
    use std::io::Write as _;
    // Le temporaire doit être unique par APPEL, pas seulement par processus :
    // `folders::rename_core` réécrit ~/.ssh/config une fois par hôte, et une
    // autre commande peut y toucher au même moment. Deux appels ouvrant le même
    // `.tmp` en troncature produisaient un fichier mêlant les deux contenus —
    // exactement la perte que cette fonction doit empêcher.
    static SUITE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            // Seul un répertoire que CET appel crée est resserré à 0700. On
            // resserrait aussi celui qui existait déjà : la suite de tests
            // lancée en root (2026-09-03), dont plusieurs cas écrivent un
            // fichier directement sous /tmp, a passé /tmp en 0700 — plus aucun
            // utilisateur du poste ne pouvait y entrer. Un compte ordinaire
            // subissait la même chose, en silence, sur tout répertoire à lui
            // où Avash déposait un fichier : un export dans ~/Documents rendait
            // ~/Documents privé.
            #[cfg(unix)]
            let existait = dir.exists();
            std::fs::create_dir_all(dir)
                .map_err(|e| anyhow::anyhow!("Création de {} : {e}", dir.display()))?;
            #[cfg(unix)]
            if !existait {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
            }
        }
    }
    // Un renommage remplace le **lien** symbolique, là où `std::fs::write` le
    // suivait et écrivait dans sa cible. Une configuration de dotfiles —
    // `~/.ssh/config` pointant vers un dépôt versionné, cas très courant —
    // aurait vu son lien transformé en fichier ordinaire au premier
    // déplacement d'hôte : le dépôt devenait silencieusement orphelin, sans
    // que `git status` n'ait rien à dire. On écrit donc dans la cible réelle.
    // `canonicalize` échoue si le fichier n'existe pas encore : c'est alors le
    // chemin demandé qui convient.
    let resolu = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let path = resolu.as_path();

    // Écrire par renommage remplace la cible même si **elle** est en lecture
    // seule : seul le droit d'écriture du répertoire compte. Un utilisateur qui
    // a délibérément passé son ~/.ssh/config en 0400 ne s'attend pas à le voir
    // réécrit ; on refuse plutôt que de passer outre.
    #[cfg(unix)]
    if let Ok(meta) = std::fs::metadata(path) {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o200 == 0 {
            return Err(anyhow::anyhow!("{} est en lecture seule.", path.display()));
        }
    }
    let tmp = path.with_extension(format!(
        "{}tmp{}.{}",
        path.extension().map_or("", |_| "."),
        std::process::id(),
        SUITE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let ecrire = || -> std::io::Result<()> {
        let mut f = options.open(&tmp)?;
        f.write_all(contenu)?;
        // Sans cette synchronisation, le renommage peut être visible avant le
        // contenu : on retrouverait un fichier de la bonne taille, rempli de
        // zéros, après une coupure de courant.
        f.sync_all()
    };
    if let Err(e) = ecrire() {
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow::anyhow!("Écriture de {} : {e}", tmp.display()));
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        anyhow::anyhow!("Renommage vers {} : {e}", path.display())
    })?;
    restreindre_au_proprietaire(path);
    Ok(())
}

/// Un alias finit dans un fichier de configuration lu par OpenSSH.
/// Refuse un saut de ligne (ou un octet nul) dans une valeur destinee a
/// `~/.ssh/config`. Sans ce controle, `HostName`, `User` ou `IdentityFile`
/// pourraient contenir un `\n` suivi d'une directive arbitraire — dont
/// `ProxyCommand`, qu'OpenSSH executerait a la connexion (exec de commande).
/// Seul l'alias etait protege ; ce trou concernait les trois autres champs.
/// Restreint un fichier de configuration à son seul propriétaire.
///
/// Ces fichiers ne contiennent pas de mot de passe — ceux-ci vivent dans le
/// trousseau — mais bien l'inventaire de l'infrastructure : bureaux RDP,
/// tunnels, dossiers, snippets, donc utilisateurs, hôtes internes, ports et
/// commandes d'administration. Ils héritaient de l'umask (souvent lisible par
/// tous), alors que `~/.ssh/config` est déjà resserré depuis longtemps. Sans
/// effet sous Windows, où les droits viennent des ACL du profil.
pub fn restreindre_au_proprietaire(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        // Le répertoire, lui, n'est resserré que s'il est à Avash. On
        // resserrait le parent de TOUT fichier écrit : un export déposé dans
        // ~/Documents rendait ~/Documents privé sans un mot, et la suite de
        // tests lancée en root sur le poste du mainteneur (2026-09-03) a passé
        // /tmp en 0700 par les cas qui y écrivent directement — plus aucun
        // utilisateur ne pouvait y entrer.
        if let Some(parent) = path.parent() {
            if est_un_repertoire_d_avash(parent) {
                let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
            }
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// `~/.ssh` et `~/.config/avash` (avec ses sous-répertoires, dont les
/// enregistrements) : les seuls répertoires qu'Avash s'autorise à resserrer,
/// parce qu'il les tient pour siens. Comparés une fois résolus, pour qu'un
/// `~/.ssh` en lien symbolique vers un dépôt de dotfiles compte aussi.
#[cfg(unix)]
fn est_un_repertoire_d_avash(dir: &std::path::Path) -> bool {
    let reel = |p: std::path::PathBuf| std::fs::canonicalize(&p).unwrap_or(p);
    let dir = reel(dir.to_path_buf());
    let ssh = repertoire_personnel().map(|h| reel(h.join(".ssh")));
    let config = repertoire_configuration().map(|c| reel(c.join("avash")));
    ssh.is_some_and(|s| s == dir) || config.is_some_and(|c| dir.starts_with(c))
}

fn validate_config_value(label: &str, value: &str) -> anyhow::Result<()> {
    if value.contains(['\n', '\r', '\0']) {
        return Err(anyhow::anyhow!(
            "{label} contient un caractère interdit (saut de ligne)."
        ));
    }
    Ok(())
}

/// Valide tous les champs d'un hote avant ecriture.
fn validate_host(host: &SshHost) -> anyhow::Result<()> {
    validate_alias(host.alias.trim())?;
    if let Some(v) = &host.hostname {
        validate_config_value("HostName", v)?;
    }
    if let Some(v) = &host.user {
        validate_config_value("User", v)?;
    }
    if let Some(v) = &host.identity_file {
        validate_config_value("IdentityFile", v)?;
    }
    if let Some(v) = &host.proxy_jump {
        validate_config_value("ProxyJump", v)?;
    }
    for t in &host.tags {
        validate_config_value("Tags", t)?;
    }
    validate_config_value("Folder", &host.folder)?;
    Ok(())
}

fn validate_alias(alias: &str) -> anyhow::Result<()> {
    if alias.is_empty() {
        return Err(anyhow::anyhow!("Le nom de l'hôte est vide."));
    }
    // Un saut de ligne permettrait d'injecter n'importe quelle directive
    // dans la configuration SSH — y compris ProxyCommand.
    if alias.contains(['\n', '\r', '\0']) {
        return Err(anyhow::anyhow!("Nom d'hôte invalide : caractère interdit."));
    }
    if alias.contains(char::is_whitespace) {
        return Err(anyhow::anyhow!(
            "Le nom d'hôte ne doit pas contenir d'espace : « {alias} »"
        ));
    }
    // `Host *` s'appliquerait a toutes les connexions.
    if alias.contains(['*', '?', '!']) {
        return Err(anyhow::anyhow!(
            "Le nom d'hôte ne doit pas contenir de joker (* ? !)."
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests_ecriture_atomique {
    use super::ecrire_atomiquement;
    use crate::testutil::temp_home;

    /// Le contenu doit être intégralement lisible, et le fichier ne doit jamais
    /// avoir été lisible par un autre compte — le temporaire naissait avec
    /// l'umask et n'était resserré qu'après le renommage.
    #[test]
    fn le_fichier_ecrit_est_complet_et_prive() {
        let home = temp_home();
        let cible = home.dir().join("secrets.yaml");
        ecrire_atomiquement(&cible, b"contenu complet\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(&cible).unwrap(),
            "contenu complet\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&cible).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "lisible par d'autres comptes : {mode:o}");
        }
    }

    /// Réécrire remplace le contenu sans laisser d'intermédiaire : aucun
    /// résidu `.tmp` ne doit subsister dans le répertoire.
    #[test]
    fn la_reecriture_ne_laisse_aucun_residu() {
        let home = temp_home();
        let cible = home.dir().join("liste.yaml");
        ecrire_atomiquement(&cible, b"premier").unwrap();
        ecrire_atomiquement(&cible, b"second").unwrap();
        assert_eq!(std::fs::read_to_string(&cible).unwrap(), "second");
        let restants: Vec<_> = std::fs::read_dir(home.dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            restants,
            vec!["liste.yaml".to_owned()],
            "résidu : {restants:?}"
        );
    }

    /// Le répertoire manquant est créé, et en 0700 : `~/.config/avash` naissait
    /// lui aussi avec l'umask.
    #[test]
    fn le_repertoire_absent_est_cree_et_prive() {
        let home = temp_home();
        let cible = home.dir().join("neuf/sous/fichier.yaml");
        ecrire_atomiquement(&cible, b"x").unwrap();
        assert!(cible.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(cible.parent().unwrap())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "répertoire ouvert : {mode:o}");
        }
    }

    /// Un répertoire qui existait déjà garde ses droits : on le resserrait à
    /// 0700 comme s'il venait d'être créé. Vu le 2026-09-03 quand la suite,
    /// lancée en root, a passé /tmp en 0700 par les cas qui y écrivent
    /// directement ; un compte ordinaire subissait la même chose sur ses
    /// propres répertoires.
    #[test]
    #[cfg(unix)]
    fn un_repertoire_existant_garde_ses_droits() {
        use std::os::unix::fs::PermissionsExt;
        let home = temp_home();
        let partage = home.dir().join("partage");
        std::fs::create_dir(&partage).unwrap();
        std::fs::set_permissions(&partage, std::fs::Permissions::from_mode(0o755)).unwrap();
        ecrire_atomiquement(&partage.join("export.yaml"), b"x").unwrap();
        let mode = std::fs::metadata(&partage).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "répertoire existant resserré : {mode:o}");
    }

    /// Les répertoires d'Avash, eux, sont resserrés même s'ils existaient
    /// déjà : `~/.config/avash` et `~/.ssh` naissaient avec l'umask, souvent
    /// lisibles par tous, et c'est ce que le correctif précédent ne doit pas
    /// défaire.
    #[test]
    #[cfg(unix)]
    fn les_repertoires_d_avash_sont_resserres_meme_existants() {
        use std::os::unix::fs::PermissionsExt;
        let home = temp_home();
        let ouverts = std::fs::Permissions::from_mode(0o755);
        let config = crate::repertoire_configuration().unwrap().join("avash");
        let ssh = home.dir().join(".ssh");
        for (dir, fichier) in [(&config, "folders.yaml"), (&ssh, "config")] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::set_permissions(dir, ouverts.clone()).unwrap();
            ecrire_atomiquement(&dir.join(fichier), b"x").unwrap();
            let mode = std::fs::metadata(dir).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "{} non resserré : {mode:o}", dir.display());
        }
    }

    /// Un chemin impossible doit remonter une erreur, pas laisser un temporaire
    /// derrière lui.
    #[test]
    fn un_echec_ne_laisse_pas_de_temporaire() {
        let home = temp_home();
        let obstacle = home.dir().join("obstacle");
        std::fs::write(&obstacle, b"je suis un fichier").unwrap();
        // « obstacle » est un fichier : on ne peut pas en faire un répertoire.
        let cible = obstacle.join("dedans.yaml");
        assert!(ecrire_atomiquement(&cible, b"x").is_err());
    }
}

#[cfg(test)]
mod save_tests {
    use super::*;

    #[test]
    fn set_host_folder_preserve_les_autres_directives() {
        let dir = std::env::temp_dir().join(format!("avash-sf-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config");
        std::fs::write(
            &path,
            "Host prod
    HostName 10.0.0.1
    ForwardAgent yes

Host autre
    HostName 10.0.0.2
",
        )
        .unwrap();
        // Ranger « prod » dans prod/web : la directive custom reste, le folder est posé.
        set_host_folder_at(&path, "prod", "prod/web").unwrap();
        let t = std::fs::read_to_string(&path).unwrap();
        assert!(t.contains("ForwardAgent yes"), "directive perdue : {t}");
        assert!(t.contains("#Folder: prod/web"), "folder absent : {t}");
        // Le bloc « autre » n'est pas touché.
        assert!(
            !t.contains(
                "Host autre
    HostName 10.0.0.2
    #Folder"
            ),
            "{t}"
        );
        // Re-déplacer remplace (pas de doublon), et vider retire la ligne.
        set_host_folder_at(&path, "prod", "").unwrap();
        let t2 = std::fs::read_to_string(&path).unwrap();
        assert!(!t2.contains("#Folder"), "folder non retiré : {t2}");
        assert!(t2.contains("ForwardAgent yes"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_host_folder_refuse_une_injection_par_saut_de_ligne() {
        let dir = std::env::temp_dir().join(format!("avash-inj-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config");
        std::fs::write(&path, "Host prod\n    HostName 10.0.0.1\n").unwrap();
        // Un dossier contenant un saut de ligne tenterait d'injecter une directive.
        let r = set_host_folder_at(&path, "prod", "web\n    ProxyCommand nc evil 22");
        assert!(r.is_err(), "l'injection aurait dû être refusée");
        let t = std::fs::read_to_string(&path).unwrap();
        assert!(!t.contains("ProxyCommand"), "directive injectée : {t}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn host(alias: &str) -> SshHost {
        SshHost {
            alias: alias.into(),
            hostname: Some("10.0.0.7".into()),
            user: Some("adrien".into()),
            port: Some(2222),
            identity_file: Some("/home/a/.ssh/id_ed25519".into()),
            ..Default::default()
        }
    }

    #[test]
    fn le_bloc_rendu_est_relu_a_l_identique() {
        // Boucle complete : ce qu'on ecrit doit etre relisible par le parseur.
        let h = host("prod");
        let relu = parse_config_str(&render_host_block(&h));
        assert_eq!(relu.len(), 1);
        assert_eq!(relu[0].alias, "prod");
        assert_eq!(relu[0].hostname.as_deref(), Some("10.0.0.7"));
        assert_eq!(relu[0].user.as_deref(), Some("adrien"));
        assert_eq!(relu[0].port, Some(2222));
        assert_eq!(
            relu[0].identity_file.as_deref(),
            Some("/home/a/.ssh/id_ed25519")
        );
    }

    #[test]
    fn le_port_par_defaut_n_est_pas_ecrit() {
        // Ecrire « Port 22 » partout alourdit le fichier pour rien.
        let mut h = host("simple");
        h.port = Some(22);
        assert!(
            !render_host_block(&h).contains("Port"),
            "{}",
            render_host_block(&h)
        );
    }

    #[test]
    fn les_champs_vides_sont_omis() {
        let h = SshHost {
            alias: "minimal".into(),
            hostname: Some("  ".into()),
            user: None,
            ..Default::default()
        };
        let bloc = render_host_block(&h);
        assert_eq!(bloc.trim(), "Host minimal", "bloc : {bloc:?}");
    }

    #[test]
    fn un_alias_avec_saut_de_ligne_est_refuse() {
        // Sans ce garde-fou on injecte n'importe quelle directive dans la
        // configuration SSH — ProxyCommand comprise.
        for mechant in [
            "prod\n    ProxyCommand nc evil.example 22",
            "prod\rHost *",
            "prod\0",
        ] {
            assert!(
                validate_alias(mechant).is_err(),
                "devrait etre refuse : {mechant:?}"
            );
        }
    }

    #[test]
    fn append_host_refuse_une_injection_de_directive_dans_les_champs() {
        // Regression securite : un saut de ligne dans HostName/User/
        // IdentityFile injecterait une directive arbitraire (ex. ProxyCommand,
        // execute par ssh a la connexion). Seul l'alias etait protege.
        let _g = crate::testutil::temp_home();
        for bad in [
            SshHost {
                alias: "srv".into(),
                hostname: Some("1.2.3.4\n    ProxyCommand evil".into()),
                ..Default::default()
            },
            SshHost {
                alias: "srv".into(),
                user: Some("root\nProxyCommand evil".into()),
                ..Default::default()
            },
            SshHost {
                alias: "srv".into(),
                identity_file: Some("/k\r  ProxyCommand evil".into()),
                ..Default::default()
            },
        ] {
            assert!(append_host(&bad).is_err(), "doit refuser : {bad:?}");
        }
        // Un hote propre passe toujours.
        assert!(append_host(&SshHost {
            alias: "ok".into(),
            hostname: Some("10.0.0.1".into()),
            ..Default::default()
        })
        .is_ok());
    }

    #[test]
    fn un_alias_joker_est_refuse() {
        // « Host * » s'appliquerait a TOUTES les connexions de la machine.
        for mechant in ["*", "prod*", "?", "!prod"] {
            assert!(
                validate_alias(mechant).is_err(),
                "devrait etre refuse : {mechant}"
            );
        }
    }

    #[test]
    fn un_alias_avec_espace_ou_vide_est_refuse() {
        assert!(validate_alias("").is_err());
        assert!(validate_alias("mon serveur").is_err());
    }

    #[test]
    fn un_alias_normal_passe() {
        for bon in ["prod", "prod-web", "serveur_1", "10.0.0.5"] {
            assert!(validate_alias(bon).is_ok(), "devrait passer : {bon}");
        }
    }
    // ---------- Ecriture reelle dans ~/.ssh/config ----------

    use crate::testutil::temp_home;

    #[test]
    fn append_host_cree_le_fichier_et_le_relit() {
        let _h = temp_home();
        append_host(&host("neuf")).unwrap();
        let relu = parse_ssh_config().unwrap();
        assert_eq!(relu.len(), 1);
        assert_eq!(relu[0].alias, "neuf");
    }

    #[test]
    fn append_host_preserve_le_contenu_existant() {
        // Le risque majeur : abimer une configuration que l'utilisateur a
        // ecrite a la main, commentaires compris.
        let _h = temp_home();
        let path = ssh_config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let avant = "# Ma config perso\nHost ancien\n    HostName 1.2.3.4\n";
        std::fs::write(&path, avant).unwrap();

        append_host(&host("nouveau")).unwrap();

        let apres = std::fs::read_to_string(&path).unwrap();
        assert!(
            apres.starts_with(avant),
            "le contenu d'origine doit rester intact :\n{apres}"
        );
        assert!(apres.contains("# Ma config perso"), "commentaire perdu");

        let relu = parse_ssh_config().unwrap();
        let noms: Vec<_> = relu.iter().map(|h| h.alias.as_str()).collect();
        assert_eq!(noms, vec!["ancien", "nouveau"]);
    }

    #[test]
    fn append_host_separe_les_blocs_par_une_ligne_vide() {
        // Sans separation, `Host` se colle a la directive precedente et en
        // devient une sous-directive : le nouvel hote serait invisible.
        let _h = temp_home();
        let path = ssh_config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "Host a\n    HostName 1.1.1.1").unwrap(); // sans \n final
        append_host(&host("b")).unwrap();
        assert_eq!(parse_ssh_config().unwrap().len(), 2);
    }

    /// Un alias déclaré dans un fichier inclus doit être refusé lui aussi.
    ///
    /// Sans cela on ajoutait un second bloc pour le même alias : OpenSSH retenant
    /// la première occurrence, l'hôte semblait ne plus répondre aux
    /// modifications, et la liste affichait deux entrées identiques.
    #[test]
    fn append_host_voit_les_alias_declares_dans_un_include() {
        let _h = temp_home();
        let ssh = repertoire_personnel().unwrap().join(".ssh");
        std::fs::create_dir_all(ssh.join("config.d")).unwrap();
        std::fs::write(
            ssh.join("config.d").join("10-prod"),
            "Host venu-d-un-include\n    HostName 10.0.0.9\n",
        )
        .unwrap();
        std::fs::write(ssh.join("config"), "Include config.d/*\n").unwrap();

        let e = append_host(&host("venu-d-un-include"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("déjà déclaré"), "{e}");
    }

    #[test]
    fn append_host_refuse_un_alias_deja_present() {
        let _h = temp_home();
        append_host(&host("double")).unwrap();
        let e = append_host(&host("double")).unwrap_err().to_string();
        assert!(e.contains("déjà déclaré"), "{e}");
        // Insensible a la casse : OpenSSH l'est aussi.
        let mut autre = host("DOUBLE");
        autre.alias = "DOUBLE".into();
        assert!(append_host(&autre).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn append_host_pose_les_droits_attendus() {
        use std::os::unix::fs::PermissionsExt;
        let _h = temp_home();
        append_host(&host("droits")).unwrap();
        let path = ssh_config_path();
        let m = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(m, 0o600, "config SSH lisible par d'autres");
        let d = std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(d, 0o700, "~/.ssh trop ouvert");
    }

    // ---------- Match ----------

    #[test]
    fn un_bloc_match_ne_contamine_pas_l_hote_precedent() {
        // Regression : `Match` n'etait pas reconnu comme delimiteur, donc ses
        // directives etaient appliquees au dernier Host. Un `Match exec`
        // jamais satisfait pouvait ainsi changer l'utilisateur et le port
        // d'un hote reel — sans le moindre avertissement.
        let cfg = "Host prod\n  HostName 10.0.0.1\n  User root\n\n                   Match exec \"test -f /tmp/jamais\"\n  User compromis\n  Port 9999\n";
        let hosts = parse_config_str(cfg);
        assert_eq!(hosts.len(), 1, "seul `prod` est un hote : {hosts:?}");
        assert_eq!(
            hosts[0].user.as_deref(),
            Some("root"),
            "utilisateur contamine"
        );
        assert_eq!(hosts[0].port, None, "port contamine");
    }

    #[test]
    fn un_host_apres_un_match_est_bien_lu() {
        let cfg = "Match user root\n  ForwardAgent yes\n\nHost apres\n  HostName 1.2.3.4\n";
        let hosts = parse_config_str(cfg);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "apres");
        assert_eq!(hosts[0].hostname.as_deref(), Some("1.2.3.4"));
    }

    // ---------- Include ----------

    #[test]
    fn include_absolu_est_resolu() {
        let _h = crate::testutil::temp_home();
        let dir = ssh_config_path().parent().unwrap().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        let inc = dir.join("perso");
        std::fs::write(&inc, "Host inclus\n  HostName 5.5.5.5\n").unwrap();
        std::fs::write(
            ssh_config_path(),
            format!(
                "Host principal\n  HostName 1.1.1.1\n\nInclude {}\n",
                inc.display()
            ),
        )
        .unwrap();

        let noms: Vec<_> = parse_ssh_config()
            .unwrap()
            .into_iter()
            .map(|h| h.alias)
            .collect();
        assert!(noms.contains(&"principal".to_string()), "{noms:?}");
        assert!(
            noms.contains(&"inclus".to_string()),
            "l'hote inclus doit apparaitre : {noms:?}"
        );
    }

    #[test]
    fn include_relatif_part_de_ssh() {
        // OpenSSH resout les chemins relatifs depuis ~/.ssh.
        let _h = crate::testutil::temp_home();
        let dir = ssh_config_path().parent().unwrap().to_path_buf();
        std::fs::create_dir_all(dir.join("config.d")).unwrap();
        std::fs::write(
            dir.join("config.d/dix"),
            "Host relatif\n  HostName 9.9.9.9\n",
        )
        .unwrap();
        std::fs::write(ssh_config_path(), "Include config.d/dix\n").unwrap();

        let noms: Vec<_> = parse_ssh_config()
            .unwrap()
            .into_iter()
            .map(|h| h.alias)
            .collect();
        assert_eq!(noms, vec!["relatif"], "{noms:?}");
    }

    #[test]
    fn include_avec_motif_prend_tous_les_fichiers_en_ordre() {
        let _h = crate::testutil::temp_home();
        let dir = ssh_config_path().parent().unwrap().to_path_buf();
        std::fs::create_dir_all(dir.join("config.d")).unwrap();
        std::fs::write(dir.join("config.d/10-a"), "Host aaa\n  HostName 1.1.1.1\n").unwrap();
        std::fs::write(dir.join("config.d/20-b"), "Host bbb\n  HostName 2.2.2.2\n").unwrap();
        std::fs::write(ssh_config_path(), "Include config.d/*\n").unwrap();

        let noms: Vec<_> = parse_ssh_config()
            .unwrap()
            .into_iter()
            .map(|h| h.alias)
            .collect();
        assert_eq!(
            noms,
            vec!["aaa", "bbb"],
            "ordre lexicographique attendu : {noms:?}"
        );
    }

    #[test]
    fn include_manquant_est_ignore_sans_planter() {
        // OpenSSH tolere un Include qui ne correspond a rien ; une config
        // partielle vaut mieux qu'aucune.
        let _h = crate::testutil::temp_home();
        let dir = ssh_config_path().parent().unwrap().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            ssh_config_path(),
            "Include /rien/du/tout\nHost seul\n  HostName 1.1.1.1\n",
        )
        .unwrap();
        let noms: Vec<_> = parse_ssh_config()
            .unwrap()
            .into_iter()
            .map(|h| h.alias)
            .collect();
        assert_eq!(noms, vec!["seul"], "{noms:?}");
    }

    #[test]
    fn include_circulaire_ne_boucle_pas() {
        // Deux fichiers qui s'incluent mutuellement : borne a 16 niveaux,
        // comme OpenSSH. Sans borne, le parseur ne rendrait jamais la main.
        let _h = crate::testutil::temp_home();
        let dir = ssh_config_path().parent().unwrap().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("boucle"),
            "Include config\nHost cycle\n  HostName 3.3.3.3\n",
        )
        .unwrap();
        std::fs::write(ssh_config_path(), "Include boucle\n").unwrap();
        let hosts = parse_ssh_config().unwrap();
        assert!(hosts.iter().any(|h| h.alias == "cycle"), "{hosts:?}");
    }

    #[test]
    fn glob_match_gere_etoile_et_point_interrogation() {
        assert!(glob_match("*", "quoi-que-ce-soit"));
        assert!(glob_match("10-*", "10-web"));
        assert!(glob_match("*.conf", "prod.conf"));
        assert!(glob_match("config?", "config1"));
        assert!(!glob_match("config?", "config12"));
        assert!(!glob_match("10-*", "20-web"));
        assert!(glob_match("a*b*c", "axxbyyc"));
    }

    // ---------- remove_host ----------

    #[test]
    fn remove_host_supprime_le_bon_bloc_et_garde_le_reste() {
        let _h = crate::testutil::temp_home();
        let path = ssh_config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "# entete perso\nHost prod\n  HostName 1.1.1.1\n\nHost staging\n  HostName 2.2.2.2\n",
        )
        .unwrap();

        remove_host("prod").unwrap();

        let apres = std::fs::read_to_string(&path).unwrap();
        assert!(
            apres.contains("# entete perso"),
            "commentaire perdu : {apres}"
        );
        assert!(
            !apres.contains("prod"),
            "prod aurait du disparaitre : {apres}"
        );
        assert!(apres.contains("staging"), "staging efface a tort : {apres}");
        let noms: Vec<_> = parse_ssh_config()
            .unwrap()
            .into_iter()
            .map(|h| h.alias)
            .collect();
        assert_eq!(noms, vec!["staging"]);
    }

    #[test]
    fn remove_host_ajoute_puis_retire_revient_a_l_etat_initial() {
        let _h = crate::testutil::temp_home();
        append_host(&host("temporaire")).unwrap();
        assert_eq!(parse_ssh_config().unwrap().len(), 1);
        remove_host("temporaire").unwrap();
        assert_eq!(parse_ssh_config().unwrap().len(), 0);
    }

    #[test]
    fn remove_host_signale_un_alias_absent() {
        let _h = crate::testutil::temp_home();
        append_host(&host("existe")).unwrap();
        let e = remove_host("absent").unwrap_err().to_string();
        assert!(e.contains("introuvable"), "{e}");
    }

    #[test]
    fn remove_host_ne_touche_pas_un_bloc_a_alias_multiples() {
        // `Host prod backup` partage des directives : retirer « prod » ne doit
        // pas casser « backup ». On laisse le bloc entier plutot que d'abimer
        // l'autre alias.
        let _h = crate::testutil::temp_home();
        let path = ssh_config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "Host prod backup\n  User root\n").unwrap();
        let e = remove_host("prod").unwrap_err().to_string();
        assert!(e.contains("introuvable"), "{e}");
    }

    // ---------- update_host ----------

    #[test]
    fn update_host_remplace_le_bloc_sur_place() {
        let _h = crate::testutil::temp_home();
        let path = ssh_config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "Host un\n  HostName 1.1.1.1\n\nHost prod\n  HostName 2.2.2.2\n  User old\n\nHost deux\n  HostName 3.3.3.3\n",
        )
        .unwrap();

        let mut modifie = host("prod");
        modifie.hostname = Some("9.9.9.9".into());
        modifie.user = Some("nouveau".into());
        update_host("prod", &modifie).unwrap();

        let hosts = parse_ssh_config().unwrap();
        // Ordre preserve : un, prod, deux.
        let noms: Vec<_> = hosts.iter().map(|h| h.alias.as_str()).collect();
        assert_eq!(noms, vec!["un", "prod", "deux"], "ordre casse");
        let p = hosts.iter().find(|h| h.alias == "prod").unwrap();
        assert_eq!(p.hostname.as_deref(), Some("9.9.9.9"));
        assert_eq!(p.user.as_deref(), Some("nouveau"));
    }

    #[test]
    fn update_host_gere_le_renommage() {
        let _h = crate::testutil::temp_home();
        append_host(&host("ancien")).unwrap();
        let mut renomme = host("ancien");
        renomme.alias = "nouveau".into();
        update_host("ancien", &renomme).unwrap();
        let noms: Vec<_> = parse_ssh_config()
            .unwrap()
            .into_iter()
            .map(|h| h.alias)
            .collect();
        assert_eq!(noms, vec!["nouveau"]);
    }

    #[test]
    fn update_host_refuse_de_renommer_vers_un_alias_existant() {
        let _h = crate::testutil::temp_home();
        append_host(&host("a")).unwrap();
        append_host(&host("b")).unwrap();
        let mut collision = host("a");
        collision.alias = "b".into();
        let e = update_host("a", &collision).unwrap_err().to_string();
        assert!(e.contains("existe déjà"), "{e}");
    }

    #[test]
    fn update_host_meme_alias_ne_declenche_pas_la_collision() {
        // Modifier sans renommer ne doit pas se heurter a « existe deja ».
        let _h = crate::testutil::temp_home();
        append_host(&host("stable")).unwrap();
        let mut m = host("stable");
        m.user = Some("change".into());
        assert!(update_host("stable", &m).is_ok());
        assert_eq!(
            parse_ssh_config().unwrap()[0].user.as_deref(),
            Some("change")
        );
    }
}

/// Fuzzing par mutation du parseur `~/.ssh/config`.
///
/// C'est la surface d'entrée la plus exposée du cœur : un fichier que
/// l'utilisateur édite à la main, qu'un outil tiers réécrit, ou qu'un dépôt de
/// dotfiles fournit. Le même principe que pour le processus RDP : muter un
/// contenu authentique atteint des chemins que des octets aléatoires ne
/// touchent jamais, parce que le tout premier `Host` filtre déjà tout ce qui ne
/// ressemble pas à une configuration.
#[cfg(test)]
mod tests_mutation {
    use super::{parse_config_str, split_proxy_jump};

    /// Générateur déterministe (LCG) : une suite rejouable, aucun crate de plus.
    struct Graine(u64);
    impl Graine {
        fn suivant(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 33
        }
        fn entre(&mut self, borne: usize) -> usize {
            (self.suivant() as usize) % borne.max(1)
        }
    }

    const SOUCHE: &str = "\
# Configuration réaliste, avec ce que le parseur sait lire.
Include ~/.ssh/conf.d/*.conf
Host prod
  HostName 10.0.0.7
  User adrien
  Port 2222
  IdentityFile ~/.ssh/id_ed25519
  ProxyJump bastion, relais:2200
  #Tags: prod, web
  #Folder: clients/acme
Host bastion
  HostName bastion.exemple.net
  User rebond
Match host *.interne
  ProxyJump none
Host *
  ServerAliveInterval 30
";

    /// Fragments qui visent les chemins du parseur : mots-clés, séparateurs,
    /// commentaires porteurs de sens, valeurs vides, octets hostiles.
    const FRAGMENTS: &[&str] = &[
        "Host ",
        "Host\t",
        "Match ",
        "Include ",
        "ProxyJump ",
        "#Tags: ",
        "#Folder: ",
        "Port ",
        "Port 99999",
        "Port 0",
        ":0",
        "\n",
        "\r\n",
        "\0",
        "  ",
        "=",
        "\"",
        "*",
        "?",
        ",",
        "../",
        "é",
        "\u{feff}",
        "\u{2028}",
        "none",
    ];

    fn muter(graine: &mut Graine, base: &str) -> String {
        let mut octets: Vec<u8> = base.as_bytes().to_vec();
        for _ in 0..=graine.entre(6) {
            match graine.entre(6) {
                0 if !octets.is_empty() => {
                    let i = graine.entre(octets.len());
                    octets[i] = graine.suivant() as u8;
                }
                1 if !octets.is_empty() => {
                    let i = graine.entre(octets.len());
                    octets.truncate(i);
                }
                2 => {
                    let i = graine.entre(octets.len() + 1);
                    let f = FRAGMENTS[graine.entre(FRAGMENTS.len())];
                    octets.splice(i..i, f.bytes());
                }
                3 if octets.len() > 2 => {
                    let a = graine.entre(octets.len());
                    let b = a + graine.entre(octets.len() - a);
                    let bloc: Vec<u8> = octets[a..b].to_vec();
                    let i = graine.entre(octets.len());
                    octets.splice(i..i, bloc);
                }
                4 if octets.len() > 2 => {
                    let a = graine.entre(octets.len());
                    let b = a + graine.entre(octets.len() - a);
                    octets.drain(a..b);
                }
                _ => {
                    let i = graine.entre(octets.len() + 1);
                    let n = 1 + graine.entre(64);
                    octets.splice(i..i, std::iter::repeat_n(b'A', n));
                }
            }
        }
        // Le parseur reçoit une `&str` : ce que `read_to_string` aurait rendu.
        String::from_utf8_lossy(&octets).into_owned()
    }

    /// Aucune mutation ne fait paniquer le parseur, et ce qu'il rend reste
    /// cohérent : un alias jamais vide, un port jamais nul, des rebonds sans
    /// espace autour.
    #[test]
    fn aucun_fichier_mute_ne_fait_paniquer_le_parseur() {
        let mut graine = Graine(0x5eed_0002_0926);
        let mut hotes_vus = 0usize;
        for _ in 0..2_000 {
            let contenu = muter(&mut graine, SOUCHE);
            let hotes = parse_config_str(&contenu);
            for h in &hotes {
                assert!(!h.alias.is_empty(), "alias vide pour :\n{contenu}");
                assert!(!h.alias.contains(['\n', '\r']), "alias multiligne");
                assert_ne!(h.port, Some(0), "port nul accepté pour :\n{contenu}");
                if let Some(pj) = &h.proxy_jump {
                    for hop in split_proxy_jump(pj) {
                        assert_eq!(hop.host.trim(), hop.host, "rebond non rogné : {hop:?}");
                    }
                }
            }
            hotes_vus += hotes.len();
        }
        assert!(
            hotes_vus > 0,
            "les mutations ont tué toutes les entrées : test sans portée"
        );
    }

    /// Trouvé par cargo-fuzz en quelques secondes, là où 2 000 mutations
    /// n'avaient jamais produit « Port 0 » : OpenSSH le refuse, nous le
    /// lisions. Un port nul n'est pas un port, ni pour l'hôte ni pour un
    /// rebond.
    #[test]
    fn le_port_zero_n_est_pas_un_port() {
        let hotes = parse_config_str("Host a\n  HostName a.local\n  Port 0\n");
        assert_eq!(hotes.len(), 1);
        assert_eq!(hotes[0].port, None);
        let hops = split_proxy_jump("relais:0, autre:2200");
        assert_eq!(hops.len(), 2);
        assert_eq!(hops[0].host, "relais:0");
        assert_eq!(hops[0].port, None);
        assert_eq!(hops[1].port, Some(2200));
    }

    /// Les mutations sont rejouables : deux exécutions rendent la même suite.
    #[test]
    fn la_suite_de_mutations_est_deterministe() {
        let a: Vec<String> = {
            let mut g = Graine(42);
            (0..20).map(|_| muter(&mut g, SOUCHE)).collect()
        };
        let b: Vec<String> = {
            let mut g = Graine(42);
            (0..20).map(|_| muter(&mut g, SOUCHE)).collect()
        };
        assert_eq!(a, b);
    }
}
