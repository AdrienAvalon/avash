//! Snippets : commandes reutilisables, avec variables `{{nom}}` et envoi
//! sur une ou plusieurs sessions. Persistes dans
//! `~/.config/avash/snippets.yaml`.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Un snippet enregistre.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snippet {
    pub id: String,
    pub name: String,
    /// Le texte a envoyer. Peut contenir des variables `{{nom}}` et des sauts
    /// de ligne (chaque ligne devient une commande).
    pub command: String,
    /// Envoyer avec Entree (executer) ou juste inserer le texte.
    #[serde(default)]
    pub run: bool,
    /// Regroupement libre, facultatif.
    #[serde(default)]
    pub category: String,
}

impl Snippet {
    #[must_use]
    pub fn new(name: &str, command: &str, run: bool, category: &str) -> Self {
        Self {
            id: format!("s-{:016x}", rand::random::<u64>()),
            name: name.trim().to_string(),
            command: command.to_string(),
            run,
            category: category.trim().to_string(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            bail!("Le snippet a besoin d'un nom.");
        }
        if self.command.trim().is_empty() {
            bail!("Le snippet est vide.");
        }
        Ok(())
    }
}

/// Extrait les variables `{{nom}}` d'une commande, sans doublon, dans l'ordre
/// d'apparition. Les espaces autour du nom sont ignores (`{{ hote }}` = `hote`).
#[must_use]
pub fn extract_vars(command: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let bytes = command.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'{' && bytes[i + 1] == b'{' {
            if let Some(end) = command[i + 2..].find("}}") {
                let name = command[i + 2..i + 2 + end].trim();
                if !name.is_empty() && !out.iter().any(|v| v == name) {
                    out.push(name.to_string());
                }
                i += 2 + end + 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Remplace chaque `{{nom}}` par sa valeur. Une variable absente de la table
/// devient une chaine vide plutot que de laisser le `{{...}}` litteral.
#[must_use]
pub fn render<S: std::hash::BuildHasher>(
    command: &str,
    vars: &HashMap<String, String, S>,
) -> String {
    let mut out = String::with_capacity(command.len());
    let bytes = command.as_bytes();
    let mut i = 0;
    while i < command.len() {
        if i + 1 < bytes.len() && bytes[i] == b'{' && bytes[i + 1] == b'{' {
            if let Some(end) = command[i + 2..].find("}}") {
                let name = command[i + 2..i + 2 + end].trim();
                out.push_str(vars.get(name).map_or("", String::as_str));
                i += 2 + end + 2;
                continue;
            }
        }
        // Pousse l'octet courant en restant sur une frontiere de caractere.
        let ch = command[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Prepare le texte a injecter dans un terminal.
///
/// Un terminal attend `\r` (Entree), pas `\n` : chaque saut de ligne d'un
/// snippet multi-lignes doit valider sa commande. Si `run`, une Entree finale
/// execute la derniere ligne ; sinon le texte est insere tel quel pour que
/// l'utilisateur relise avant de valider.
#[must_use]
pub fn terminal_payload(text: &str, run: bool) -> String {
    let mut out = text.replace("\r\n", "\n").replace('\n', "\r");
    if run && !out.ends_with('\r') {
        out.push('\r');
    }
    out
}

// ---------- Persistance ----------

/// `~/.config/avash/snippets.yaml`
#[must_use]
pub fn snippets_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("avash")
        .join("snippets.yaml")
}

pub fn load_snippets() -> Result<Vec<Snippet>> {
    load_snippets_from(&snippets_path())
}

/// Un fichier absent n'est pas une erreur : c'est l'etat initial.
pub fn load_snippets_from(path: &Path) -> Result<Vec<Snippet>> {
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

/// Ecriture atomique : un plantage en pleine ecriture ne laisse pas un
/// fichier tronque qui perdrait tous les snippets.
pub fn save_snippets_to(path: &Path, snippets: &[Snippet]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("yaml.tmp");
    std::fs::write(&tmp, serde_yaml::to_string(snippets)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Ajoute ou remplace (meme `id`) un snippet.
pub fn upsert_snippet_in(path: &Path, snippet: Snippet) -> Result<Vec<Snippet>> {
    snippet.validate()?;
    let mut all = load_snippets_from(path)?;
    match all.iter_mut().find(|s| s.id == snippet.id) {
        Some(slot) => *slot = snippet,
        None => all.push(snippet),
    }
    save_snippets_to(path, &all)?;
    Ok(all)
}

pub fn remove_snippet_in(path: &Path, id: &str) -> Result<Vec<Snippet>> {
    let mut all = load_snippets_from(path)?;
    all.retain(|s| s.id != id);
    save_snippets_to(path, &all)?;
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn extract_vars_sans_doublon_dans_l_ordre() {
        let v = extract_vars("systemctl {{action}} {{service}} && journalctl -u {{service}}");
        assert_eq!(v, vec!["action", "service"]);
    }

    #[test]
    fn extract_vars_ignore_les_espaces_et_les_accolades_seules() {
        assert_eq!(extract_vars("echo {{ hote }}"), vec!["hote"]);
        assert_eq!(extract_vars("echo ${x} {y} {{}}"), Vec::<String>::new());
    }

    #[test]
    fn render_remplace_et_laisse_vide_l_inconnue() {
        let out = render(
            "ssh {{user}}@{{host}}",
            &vars(&[("user", "root"), ("host", "srv")]),
        );
        assert_eq!(out, "ssh root@srv");
        assert_eq!(
            render("a {{x}} b", &vars(&[])),
            "a  b",
            "variable absente = vide"
        );
    }

    #[test]
    fn render_preserve_l_utf8_hors_variables() {
        assert_eq!(
            render("écho {{v}} déjà 😈", &vars(&[("v", "ok")])),
            "écho ok déjà 😈"
        );
    }

    #[test]
    fn terminal_payload_convertit_les_sauts_de_ligne() {
        assert_eq!(terminal_payload("a\nb", true), "a\rb\r");
        assert_eq!(terminal_payload("a\r\nb", false), "a\rb");
        assert_eq!(terminal_payload("x", true), "x\r");
        assert_eq!(terminal_payload("x\r", true), "x\r", "pas de double Entree");
        assert_eq!(terminal_payload("x", false), "x", "insertion sans executer");
    }

    #[test]
    fn validate_refuse_nom_ou_commande_vide() {
        assert!(Snippet::new("", "ls", false, "").validate().is_err());
        assert!(Snippet::new("Liste", "   ", false, "").validate().is_err());
        assert!(Snippet::new("Liste", "ls -la", false, "")
            .validate()
            .is_ok());
    }

    fn temp_file() -> PathBuf {
        std::env::temp_dir().join(format!(
            "avash-snip-{}-{:?}.yaml",
            std::process::id(),
            std::thread::current().id()
        ))
    }

    #[test]
    fn persistance_upsert_relecture_remove() {
        let p = temp_file();
        let _ = std::fs::remove_file(&p);
        assert!(load_snippets_from(&p).unwrap().is_empty());
        let a = Snippet::new("Bilan", "df -h", true, "sys");
        let b = Snippet::new("Logs", "journalctl -u {{svc}} -f", false, "sys");
        upsert_snippet_in(&p, a.clone()).unwrap();
        let all = upsert_snippet_in(&p, b.clone()).unwrap();
        assert_eq!(all, vec![a.clone(), b.clone()]);
        assert_eq!(load_snippets_from(&p).unwrap(), all, "relecture identique");

        // Remplacement par id : pas de doublon.
        let mut a2 = a.clone();
        a2.command = "df -h && free -h".into();
        let all = upsert_snippet_in(&p, a2.clone()).unwrap();
        assert_eq!(all, vec![a2.clone(), b.clone()]);

        let all = remove_snippet_in(&p, &a2.id).unwrap();
        assert_eq!(all, vec![b]);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn upsert_refuse_invalide_sans_ecrire() {
        let p = temp_file();
        let _ = std::fs::remove_file(&p);
        assert!(upsert_snippet_in(&p, Snippet::new("", "x", false, "")).is_err());
        assert!(!p.exists(), "rien ne doit etre ecrit");
    }

    #[test]
    fn yaml_ancien_sans_champs_optionnels() {
        let p = temp_file();
        std::fs::write(&p, "- id: s-1\n  name: Simple\n  command: ls\n").unwrap();
        let s = &load_snippets_from(&p).unwrap()[0];
        assert!(!s.run);
        assert_eq!(s.category, "");
        let _ = std::fs::remove_file(&p);
    }
}
