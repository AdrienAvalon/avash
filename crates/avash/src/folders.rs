//! Registre des dossiers de rangement des hôtes (arbre unifié SSH + RDP).
//!
//! L'appartenance d'un hôte à un dossier est stockée avec l'hôte lui-même
//! (`#Folder:` dans `~/.ssh/config`, champ `folder` dans `rdp.yaml`). Ce
//! registre ne sert qu'à retenir la LISTE des dossiers — en particulier les
//! dossiers vides, qui n'apparaîtraient sinon nulle part. L'arbre affiché est
//! l'union de ce registre et des dossiers référencés par les hôtes.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
struct FoldersFile {
    #[serde(default)]
    folders: Vec<String>,
}

#[must_use]
pub fn folders_path() -> PathBuf {
    crate::repertoire_configuration()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("avash")
        .join("folders.yaml")
}

/// Normalise un chemin de dossier : segments non vides, sans espaces de bord,
/// joints par `/`. `""` = racine.
#[must_use]
pub fn normalize(path: &str) -> String {
    path.split('/')
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "." && *s != ".." && !s.contains(['\n', '\r', '\0']))
        .collect::<Vec<_>>()
        .join("/")
}

/// Tous les ancêtres d'un chemin, lui inclus (« a/b/c » → a, a/b, a/b/c).
fn with_ancestors(path: &str) -> Vec<String> {
    let mut acc = String::new();
    let mut out = Vec::new();
    for seg in path.split('/').filter(|s| !s.is_empty()) {
        if acc.is_empty() {
            acc = seg.to_string();
        } else {
            acc = format!("{acc}/{seg}");
        }
        out.push(acc.clone());
    }
    out
}

fn load_from(path: &Path) -> Result<Vec<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let f: FoldersFile = serde_yaml::from_str(&text).context("folders.yaml illisible")?;
            let mut v: Vec<String> = f
                .folders
                .iter()
                .map(|p| normalize(p))
                .filter(|p| !p.is_empty())
                .collect();
            v.sort();
            v.dedup();
            Ok(v)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(anyhow::anyhow!("Lecture de {} : {e}", path.display())),
    }
}

fn save_to(path: &Path, folders: &[String]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("Création de {}", dir.display()))?;
    }
    let mut sorted: Vec<String> = folders
        .iter()
        .map(|p| normalize(p))
        .filter(|p| !p.is_empty())
        .collect();
    sorted.sort();
    sorted.dedup();
    let f = FoldersFile { folders: sorted };
    let yaml = serde_yaml::to_string(&f).context("sérialisation folders.yaml")?;
    crate::ecrire_atomiquement(path, yaml.as_bytes())
}

/// Liste des dossiers connus (triée). Voir aussi les dossiers dérivés des hôtes.
///
/// # Errors
/// Si le fichier existe mais est illisible.
pub fn list() -> Result<Vec<String>> {
    load_from(&folders_path())
}

/// Enregistre un dossier (et ses ancêtres). Idempotent.
///
/// # Errors
/// Si le fichier est illisible/inscriptible.
pub fn create(path: &str) -> Result<Vec<String>> {
    create_in(&folders_path(), path)
}

pub fn create_in(file: &Path, path: &str) -> Result<Vec<String>> {
    let norm = normalize(path);
    if norm.is_empty() {
        anyhow::bail!("Nom de dossier vide.");
    }
    let mut all = load_from(file)?;
    for p in with_ancestors(&norm) {
        if !all.contains(&p) {
            all.push(p);
        }
    }
    save_to(file, &all)?;
    all.sort();
    Ok(all)
}

/// Retire un dossier et tous ses descendants du registre (le déplacement des
/// hôtes est géré par l'appelant). Renvoie la liste restante.
///
/// # Errors
/// Si le fichier est illisible/inscriptible.
pub fn remove_in(file: &Path, path: &str) -> Result<Vec<String>> {
    let norm = normalize(path);
    let prefix = format!("{norm}/");
    let mut all = load_from(file)?;
    all.retain(|p| p != &norm && !p.starts_with(&prefix));
    save_to(file, &all)?;
    Ok(all)
}

/// Renomme un dossier (et remappe ses descendants) dans le registre. Le remap
/// des hôtes est géré par l'appelant. Renvoie la liste résultante.
///
/// # Errors
/// Si le fichier est illisible/inscriptible, ou la cible vide.
pub fn rename_in(file: &Path, from: &str, to: &str) -> Result<Vec<String>> {
    let from = normalize(from);
    let to = normalize(to);
    if to.is_empty() {
        anyhow::bail!("Nom de dossier vide.");
    }
    let prefix = format!("{from}/");
    let mut all = load_from(file)?;
    for p in &mut all {
        if p == &from {
            p.clone_from(&to);
        } else if let Some(rest) = p.strip_prefix(&prefix) {
            *p = format!("{to}/{rest}");
        }
    }
    for p in with_ancestors(&to) {
        if !all.contains(&p) {
            all.push(p);
        }
    }
    save_to(file, &all)?;
    all.sort();
    all.dedup();
    Ok(all)
}

/// Nouveau dossier d'un hôte lors d'un renommage `from`→`to`. `None` = inchangé.
#[must_use]
pub fn remap(current: &str, from: &str, to: &str) -> Option<String> {
    if current == from {
        Some(to.to_string())
    } else {
        current
            .strip_prefix(&format!("{from}/"))
            .map(|rest| format!("{to}/{rest}"))
    }
}

/// Vrai si `current` est le dossier `path` ou un de ses sous-dossiers.
#[must_use]
pub fn is_under(current: &str, path: &str) -> bool {
    current == path || current.starts_with(&format!("{path}/"))
}

fn read_optional(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(c) => Ok(Some(c)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::anyhow!("Lecture de {} : {e}", path.display())),
    }
}

/// Applique un remap aux dossiers des hôtes RDP d'un fichier donné.
fn remap_rdp(rdp: &Path, f: impl Fn(&str) -> Option<String>) -> Result<()> {
    let Ok(mut hosts) = crate::rdphost::load_hosts_from(rdp) else {
        return Ok(()); // fichier absent/illisible : rien à remapper
    };
    let mut changed = false;
    for h in &mut hosts {
        if let Some(nf) = f(&h.folder) {
            h.folder = nf;
            changed = true;
        }
    }
    if changed {
        crate::rdphost::save_hosts_to(rdp, &hosts)?;
    }
    Ok(())
}

/// Renomme un dossier et remappe les hôtes SSH + RDP (chemins explicites, testable).
///
/// Les blocs `Host` à alias multiples (non modifiables) sont ignorés sans faire
/// échouer l'opération. Renvoie la liste des dossiers restante.
///
/// # Errors
/// Si un fichier existant est illisible/inscriptible, ou la cible est vide.
pub fn rename_core(
    ssh: &Path,
    rdp: &Path,
    reg: &Path,
    from: &str,
    to: &str,
) -> Result<Vec<String>> {
    let from = normalize(from);
    let to = normalize(to);
    if from.is_empty() || to.is_empty() {
        anyhow::bail!("Dossier invalide.");
    }
    if let Some(content) = read_optional(ssh)? {
        let multiples = alias_a_alias_multiples(&content);
        let mut recales = Vec::new();
        for host in crate::parse_config_str(&content) {
            if let Some(nf) = remap(&host.folder, &from, &to) {
                if crate::set_host_folder_at(ssh, &host.alias, &nf).is_err()
                    && !multiples.contains(&host.alias)
                {
                    recales.push(host.alias.clone());
                }
            }
        }
        signaler_les_recales(&recales, "déplacés")?;
    }
    remap_rdp(rdp, |f| remap(f, &from, &to))?;
    rename_in(reg, &from, &to)
}

/// Alias déclarés dans un bloc `Host a b c` — plusieurs noms sur une ligne.
///
/// `set_host_folder_at` refuse volontairement d'y toucher : il ne saurait pas
/// où poser le marqueur de dossier sans changer le sens du bloc pour les autres
/// alias. Ces échecs-là sont attendus et ne doivent pas être signalés.
fn alias_a_alias_multiples(content: &str) -> std::collections::HashSet<String> {
    let mut multiples = std::collections::HashSet::new();
    for ligne in content.lines() {
        let l = ligne.trim();
        let Some(reste) = l
            .strip_prefix("Host ")
            .or_else(|| l.strip_prefix("host "))
            .or_else(|| l.strip_prefix("HOST "))
        else {
            continue;
        };
        let alias: Vec<&str> = reste.split_whitespace().collect();
        if alias.len() > 1 {
            multiples.extend(alias.into_iter().map(str::to_owned));
        }
    }
    multiples
}

/// Signale les hôtes qu'on n'a pas su déplacer.
///
/// L'échec était intégralement avalé (`let _ =`). C'est justifié pour un bloc
/// à alias multiples — écarté en amont — mais cela masquait aussi les vraies
/// erreurs : `~/.ssh/config` en lecture seule, disque plein. Le registre était
/// alors mis à jour, `Ok` renvoyé, l'interface annonçait le renommage, et
/// l'ancien dossier réapparaissait aussitôt dans l'arbre — il est dérivé des
/// hôtes. Deux dossiers là où l'on en attendait un, sans explication.
fn signaler_les_recales(recales: &[String], quoi: &str) -> Result<()> {
    if recales.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "{} hôte(s) n'ont pas pu être {quoi} : {}. Vérifie que ~/.ssh/config est accessible en écriture.",
        recales.len(),
        recales.join(", ")
    )
}

/// Supprime un dossier : ses hôtes (et ceux des sous-dossiers) reviennent à la
/// racine, puis le dossier et ses descendants quittent le registre.
///
/// # Errors
/// idem [`rename_core`].
pub fn delete_core(ssh: &Path, rdp: &Path, reg: &Path, path: &str) -> Result<Vec<String>> {
    let norm = normalize(path);
    if norm.is_empty() {
        anyhow::bail!("Dossier invalide.");
    }
    if let Some(content) = read_optional(ssh)? {
        let multiples = alias_a_alias_multiples(&content);
        let mut recales = Vec::new();
        for host in crate::parse_config_str(&content) {
            if is_under(&host.folder, &norm)
                && crate::set_host_folder_at(ssh, &host.alias, "").is_err()
                && !multiples.contains(&host.alias)
            {
                recales.push(host.alias.clone());
            }
        }
        signaler_les_recales(&recales, "ramenés à la racine")?;
    }
    remap_rdp(rdp, |f| is_under(f, &norm).then(String::new))?;
    remove_in(reg, &norm)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        std::env::temp_dir().join(format!("avash-folders-{}.yaml", rand::random::<u64>()))
    }

    /// Le registre décrit l'infrastructure (dossiers, donc organisation des
    /// hôtes) : il ne doit pas être lisible par les autres comptes de la
    /// machine. Il héritait auparavant de l'umask, souvent 0644.
    #[cfg(unix)]
    #[test]
    fn le_registre_n_est_lisible_que_par_son_proprietaire() {
        use std::os::unix::fs::PermissionsExt;
        let _h = crate::testutil::temp_home();
        create("prod/web").unwrap();
        let droits = std::fs::metadata(folders_path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(droits, 0o600, "droits du registre : {droits:o}");
    }

    #[test]
    fn create_ajoute_les_ancetres() {
        let f = tmp();
        let all = create_in(&f, "prod/web/front").unwrap();
        assert!(all.contains(&"prod".to_string()));
        assert!(all.contains(&"prod/web".to_string()));
        assert!(all.contains(&"prod/web/front".to_string()));
        let _ = std::fs::remove_file(&f);
    }

    #[test]
    fn remove_emporte_les_descendants() {
        let f = tmp();
        create_in(&f, "prod/web").unwrap();
        create_in(&f, "perso").unwrap();
        let all = remove_in(&f, "prod").unwrap();
        assert_eq!(all, vec!["perso".to_string()]);
        let _ = std::fs::remove_file(&f);
    }

    #[test]
    fn rename_remappe_les_descendants() {
        let f = tmp();
        create_in(&f, "prod/web").unwrap();
        let all = rename_in(&f, "prod", "production").unwrap();
        assert!(all.contains(&"production".to_string()));
        assert!(all.contains(&"production/web".to_string()));
        assert!(!all.iter().any(|p| p.starts_with("prod/")));
        let _ = std::fs::remove_file(&f);
    }

    #[test]
    fn normalize_nettoie() {
        assert_eq!(normalize(" /a// b /c/ "), "a/b/c");
        assert_eq!(normalize("///"), "");
    }

    #[test]
    fn normalize_rejette_dot_dot_absolu_et_sauts_de_ligne() {
        assert_eq!(normalize("../a"), "a");
        assert_eq!(normalize("a/../b"), "a/b");
        assert_eq!(normalize("/etc/passwd"), "etc/passwd");
        assert_eq!(normalize("a/./b"), "a/b");
        // Un segment contenant un saut de ligne (tentative d'injection) est retiré.
        assert_eq!(normalize("prod\nProxyCommand x"), "");
        assert_eq!(normalize("ok/bad\nx/end"), "ok/end");
    }

    fn scratch() -> PathBuf {
        let d = std::env::temp_dir().join(format!("avash-core-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Un `~/.ssh/config` inscriptible par personne faisait échouer chaque
    /// déplacement d'hôte — silencieusement. Le registre était quand même mis à
    /// jour et `Ok` renvoyé : l'interface annonçait le renommage, puis l'ancien
    /// dossier réapparaissait dans l'arbre, qui est dérivé des hôtes.
    #[test]
    #[cfg(unix)]
    fn rename_core_signale_un_config_non_inscriptible() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("avash-ro-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        let ssh = dir.join("config");
        let rdp = dir.join("rdp.yaml");
        let reg = dir.join("folders.yaml");
        std::fs::write(
            &ssh,
            "Host web-1\n    HostName 1.1.1.1\n    #Folder: prod\n",
        )
        .unwrap();
        // Lecture seule : parse_config_str lit encore, l'écriture échoue.
        std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o400)).unwrap();

        let issue = rename_core(&ssh, &rdp, &reg, "prod", "production");

        std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o600)).unwrap();
        let e = issue
            .expect_err("un config non inscriptible doit être signalé")
            .to_string();
        assert!(e.contains("web-1"), "l'hôte concerné doit être nommé : {e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rename_core_remappe_ssh_et_rdp_et_ignore_multi_alias() {
        let d = scratch();
        let (ssh, rdp, reg) = (d.join("config"), d.join("rdp.yaml"), d.join("folders.yaml"));
        std::fs::write(
            &ssh,
            "Host a\n    HostName 1\n    #Folder: prod\n\nHost b\n    HostName 2\n    #Folder: prod/web\n\nHost c\n    HostName 3\n\nHost x y\n    HostName 4\n    #Folder: prod\n",
        )
        .unwrap();
        let mut r = crate::rdphost::RdpHost::new("AD", "10.0.0.1", 3389, "u", 0, 0);
        r.folder = "prod".into();
        crate::rdphost::save_hosts_to(&rdp, &[r]).unwrap();
        create_in(&reg, "prod/web").unwrap();

        let regs = rename_core(&ssh, &rdp, &reg, "prod", "production").unwrap();

        let hosts = crate::parse_config_str(&std::fs::read_to_string(&ssh).unwrap());
        let f = |al: &str| {
            hosts
                .iter()
                .find(|h| h.alias == al)
                .map(|h| h.folder.clone())
        };
        assert_eq!(f("a").as_deref(), Some("production"));
        assert_eq!(f("b").as_deref(), Some("production/web"));
        assert_eq!(f("c").as_deref(), Some("")); // hors sous-arbre : inchangé
        assert_eq!(
            crate::rdphost::load_hosts_from(&rdp).unwrap()[0].folder,
            "production"
        );
        assert!(
            regs.contains(&"production".to_string())
                && regs.contains(&"production/web".to_string())
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn delete_core_ramene_a_la_racine() {
        let d = scratch();
        let (ssh, rdp, reg) = (d.join("config"), d.join("rdp.yaml"), d.join("folders.yaml"));
        std::fs::write(
            &ssh,
            "Host a\n    HostName 1\n    #Folder: prod\n\nHost b\n    HostName 2\n    #Folder: prod/web\n\nHost c\n    HostName 3\n    #Folder: autre\n",
        )
        .unwrap();
        let mut r = crate::rdphost::RdpHost::new("AD", "10.0.0.1", 3389, "u", 0, 0);
        r.folder = "prod/web".into();
        crate::rdphost::save_hosts_to(&rdp, &[r]).unwrap();
        create_in(&reg, "prod/web").unwrap();
        create_in(&reg, "autre").unwrap();

        let regs = delete_core(&ssh, &rdp, &reg, "prod").unwrap();

        let hosts = crate::parse_config_str(&std::fs::read_to_string(&ssh).unwrap());
        let f = |al: &str| hosts.iter().find(|h| h.alias == al).unwrap().folder.clone();
        assert_eq!(f("a"), "");
        assert_eq!(f("b"), "");
        assert_eq!(f("c"), "autre"); // hors du dossier supprimé : intact
        assert_eq!(crate::rdphost::load_hosts_from(&rdp).unwrap()[0].folder, "");
        assert!(!regs.iter().any(|p| p.starts_with("prod")));
        assert!(regs.contains(&"autre".to_string()));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn cores_survivent_a_l_absence_de_fichiers() {
        let d = scratch();
        // Ni ssh ni rdp n'existent : l'opération ne doit pas échouer.
        let (ssh, rdp, reg) = (d.join("nope"), d.join("nope.yaml"), d.join("folders.yaml"));
        create_in(&reg, "prod").unwrap();
        assert!(delete_core(&ssh, &rdp, &reg, "prod").is_ok());
        let _ = std::fs::remove_dir_all(&d);
    }
}
