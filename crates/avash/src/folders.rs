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
    dirs::config_dir()
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
        .filter(|s| !s.is_empty())
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
    let tmp = path.with_extension("yaml.tmp");
    std::fs::write(&tmp, yaml).with_context(|| format!("Écriture de {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("Renommage vers {}", path.display()))?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        std::env::temp_dir().join(format!("avash-folders-{}.yaml", rand::random::<u64>()))
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
}
