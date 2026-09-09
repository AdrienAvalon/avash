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
///
/// Un segment portant un caractère de contrôle est retiré, pas nettoyé : le nom
/// finit en commentaire `# Folder` dans `~/.ssh/config`, donc sous les yeux de
/// qui relit ce fichier ou lance `avash list`. La liste était `\n \r \0`, la
/// même trop courte que celle de `validate_config_value` ; l'audit du
/// 9 septembre 2026 l'a élargie à tout le plan de contrôle (ESC, BEL, DEL, C1),
/// tabulation comprise.
#[must_use]
pub fn normalize(path: &str) -> String {
    path.split('/')
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "." && *s != ".." && !s.contains(char::is_control))
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

/// Applique un remap aux dossiers d'une liste d'hôtes RDP déjà chargée, et
/// réécrit `rdp.yaml` si quelque chose a bougé.
///
/// La liste est passée par l'appelant, qui l'a chargée AVANT toute écriture SSH
/// (voir `rename_core`/`delete_core`) : un `rdp.yaml` illisible doit faire
/// échouer l'opération avant qu'on ne touche `~/.ssh/config`, pas après.
fn remap_rdp(
    rdp: &Path,
    mut hosts: Vec<crate::rdphost::RdpHost>,
    f: impl Fn(&str) -> Option<String>,
) -> Result<()> {
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
    // Trouvé par l'audit du 7 septembre 2026 : `remap_rdp` avalait en silence un
    // `rdp.yaml` illisible (YAML corrompu, champ requis manquant, permission
    // refusée — seul le fichier absent est un `Ok(Vec::new())`). Le renommage
    // continuait, le registre était réécrit, l'interface annonçait le succès,
    // puis les bureaux réapparaissaient dans l'ancien dossier une fois le fichier
    // réparé : deux dossiers là où on en attendait un. Même classe de défaut que
    // `signaler_les_recales` côté SSH. On sonde donc `rdp.yaml` ICI, avant toute
    // écriture SSH, pour qu'une erreur soit signalée sans renommage partiel.
    let hosts_rdp = crate::rdphost::load_hosts_brut_from(rdp)
        .with_context(|| format!("bureaux RDP de {} non remappés", rdp.display()))?;
    // Sonde des hôtes venus d'un `Include` (voir `hotes_inclus_affectes`) : on
    // refuse avant toute écriture, sinon l'ancien dossier réapparaît dans l'arbre.
    signaler_les_hotes_inclus(&hotes_inclus_affectes(ssh, |f| {
        remap(f, &from, &to).is_some()
    }))?;
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
    remap_rdp(rdp, hosts_rdp, |f| remap(f, &from, &to))?;
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
        // Trouvé par l'audit du 7 septembre 2026 : on découpait sur `"Host "`
        // (espace) uniquement. Or OpenSSH accepte la tabulation comme séparateur
        // et le mot-clé est insensible à la casse — `parse_config_str` et
        // `set_host_folder_at` le savaient (split_once + to_lowercase), pas cette
        // fonction. Un bloc `Host\tx y` ou `HoSt x y` échappait donc à `multiples` :
        // ses alias, éclatés en hôtes mono par `parse_config_str`, échouaient tous
        // dans `set_host_folder_at` (bloc non mono-alias) et remplissaient
        // `recales`, faisant échouer tout le renommage sur un message accusant à
        // tort les droits de `~/.ssh/config`. On découpe donc comme les deux autres.
        let Some((mot, reste)) = l.split_once(char::is_whitespace) else {
            continue;
        };
        if !mot.eq_ignore_ascii_case("host") {
            continue;
        }
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

/// Alias affectés par l'opération mais déclarés dans un fichier `Include`.
///
/// Trouvé par l'audit du 7 septembre 2026 : `rename_core`/`delete_core` ne
/// parsaient que le fichier principal (`parse_config_str`, qui ignore
/// `Include`), alors que l'arbre affiché résout les Include (`parse_ssh_config`).
/// Un hôte d'un fichier inclus portant `#Folder: prod` — typiquement un bloc
/// rangé par Avash dans le fichier principal, puis déplacé à la main dans
/// `conf.d/` avec son marqueur — n'était ni remappé ni signalé : après un
/// renommage `prod` → `production`, l'ancien dossier réapparaissait dans l'arbre
/// avec lui. `set_host_folder_at` ne sait réécrire QUE le fichier principal, on
/// ne peut donc pas remapper ces blocs en toute sûreté : on les signale, avec un
/// message distinct de `signaler_les_recales` pour ne pas accuser à tort les
/// droits de `~/.ssh/config`. La sonde est faite avant toute écriture, comme
/// celle de `rdp.yaml`, pour ne pas laisser un renommage à moitié appliqué.
fn hotes_inclus_affectes(ssh: &Path, affecte: impl Fn(&str) -> bool) -> Vec<String> {
    let principaux: std::collections::HashSet<String> = match read_optional(ssh) {
        Ok(Some(content)) => crate::parse_config_str(&content)
            .into_iter()
            .map(|h| h.alias)
            .collect(),
        _ => return Vec::new(),
    };
    crate::parse_config_resolu_at(ssh)
        .into_iter()
        .filter(|h| !principaux.contains(&h.alias) && affecte(&h.folder))
        .map(|h| h.alias)
        .collect()
}

/// Signale les hôtes affectés mais déclarés dans un fichier inclus.
fn signaler_les_hotes_inclus(inclus: &[String]) -> Result<()> {
    if inclus.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "{} hôte(s) déclaré(s) dans un fichier inclus, à déplacer à la main : {}.",
        inclus.len(),
        inclus.join(", ")
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
    // Même défaut que `rename_core` (audit du 7 septembre 2026) : on sonde
    // `rdp.yaml` avant toute écriture SSH pour qu'un fichier illisible fasse
    // échouer la suppression sans laisser les bureaux dans un dossier disparu.
    let hosts_rdp = crate::rdphost::load_hosts_brut_from(rdp)
        .with_context(|| format!("bureaux RDP de {} non remappés", rdp.display()))?;
    // Même sonde que `rename_core` : un hôte inclus dans le dossier supprimé
    // resterait sinon dans `prod` alors que l'opération annonce le succès.
    signaler_les_hotes_inclus(&hotes_inclus_affectes(ssh, |f| is_under(f, &norm)))?;
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
    remap_rdp(rdp, hosts_rdp, |f| is_under(f, &norm).then(String::new))?;
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
    fn normalize_retire_un_segment_a_caractere_de_controle() {
        // Trouvé par l'audit du 9 septembre 2026. `normalize` ne connaissait
        // que `\n`, `\r` et `\0`, la même liste trop courte que
        // `validate_config_value` : un nom de dossier finit en `# Folder` dans
        // `~/.ssh/config`, donc dans le terminal de qui relit ce fichier ou
        // lance `avash list`. ESC, BEL, DEL et les C1 tombent avec le reste.
        for c in ['\u{1b}', '\u{7}', '\u{7f}', '\u{9b}', '\t'] {
            assert_eq!(normalize(&format!("prod{c}x")), "", "U+{:04X}", c as u32);
            assert_eq!(normalize(&format!("ok/bad{c}x/end")), "ok/end");
        }
        // Rien de légitime ne tombe au passage : accents et espace interne.
        assert_eq!(
            normalize("Prod été/Bases de données"),
            "Prod été/Bases de données"
        );
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

    /// Trouvé par l'audit du 7 septembre 2026 : `remap_rdp` réécrivait la liste
    /// FILTRÉE des bureaux, si bien que renommer un dossier effaçait du fichier
    /// tout bureau qu'une version antérieure (ou une édition manuelle) avait
    /// laissé invalide — adresse à espace ici. `rename_core` charge désormais la
    /// liste BRUTE avant le remap : l'entrée invalide traverse la réécriture,
    /// son dossier remappé comme les autres.
    #[test]
    fn une_entree_invalide_survit_a_un_renommage_de_dossier() {
        let d = scratch();
        let (ssh, rdp, reg) = (d.join("config"), d.join("rdp.yaml"), d.join("folders.yaml"));
        let mut a = crate::rdphost::RdpHost::new("A", "10.0.0.1", 3389, "u", 0, 0);
        a.folder = "prod".into();
        let mut b = crate::rdphost::RdpHost::new("B", "x", 3389, "u", 0, 0);
        b.host = "srv 01".into(); // adresse à espace : invalide, non affichable
        b.folder = "prod".into();
        assert!(b.validate().is_err(), "B doit bien être invalide");
        crate::rdphost::save_hosts_to(&rdp, &[a, b.clone()]).unwrap();
        create_in(&reg, "prod").unwrap();

        rename_core(&ssh, &rdp, &reg, "prod", "production").unwrap();

        let brut = crate::rdphost::load_hosts_brut_from(&rdp).unwrap();
        let bb = brut
            .iter()
            .find(|h| h.id == b.id)
            .expect("B effacée par le renommage de dossier");
        assert_eq!(bb.name, "B");
        assert_eq!(bb.folder, "production", "dossier de B non remappé");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Trouvé par l'audit du 7 septembre 2026 : `alias_a_alias_multiples` ne
    /// reconnaissait que le préfixe `"Host "` (espace). Or OpenSSH accepte la
    /// tabulation comme séparateur (`Host\tx y`) — `parse_config_str` et
    /// `set_host_folder_at` le savaient déjà, pas ce helper. Le bloc, éclaté en
    /// hôtes mono par le parseur, échouait alors dans `set_host_folder_at`
    /// (bloc non mono-alias) sans être reconnu comme multi-alias : les hôtes
    /// remplissaient `recales` et tout le renommage échouait sur un message
    /// accusant à tort les droits de `~/.ssh/config`. Le bloc doit être ignoré
    /// en silence, comme sa variante à espace, et le renommage aboutir.
    #[test]
    fn un_bloc_multi_alias_tabule_est_ignore_sans_faire_echouer() {
        let d = scratch();
        let (ssh, rdp, reg) = (d.join("config"), d.join("rdp.yaml"), d.join("folders.yaml"));
        std::fs::write(
            &ssh,
            "Host a\n    HostName 1\n    #Folder: prod\n\nHost\tx y\n    HostName 4\n    #Folder: prod\n",
        )
        .unwrap();
        create_in(&reg, "prod").unwrap();

        let regs = rename_core(&ssh, &rdp, &reg, "prod", "production")
            .expect("un bloc multi-alias tabulé ne doit pas faire échouer le renommage");

        let hosts = crate::parse_config_str(&std::fs::read_to_string(&ssh).unwrap());
        let f = |al: &str| {
            hosts
                .iter()
                .find(|h| h.alias == al)
                .map(|h| h.folder.clone())
        };
        assert_eq!(f("a").as_deref(), Some("production"));
        assert!(regs.contains(&"production".to_string()));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Même trou pour la casse : `HoSt x y` échappait aussi aux trois
    /// `strip_prefix`, alors que les deux autres parseurs comparent le mot-clé
    /// en minuscules. Le correctif (`split_once` + `eq_ignore_ascii_case`) ferme
    /// les deux d'un coup. Audit du 7 septembre 2026.
    #[test]
    fn un_bloc_multi_alias_en_casse_melangee_est_ignore() {
        let d = scratch();
        let (ssh, rdp, reg) = (d.join("config"), d.join("rdp.yaml"), d.join("folders.yaml"));
        std::fs::write(
            &ssh,
            "Host a\n    HostName 1\n    #Folder: prod\n\nHoSt x y\n    HostName 4\n    #Folder: prod\n",
        )
        .unwrap();
        create_in(&reg, "prod").unwrap();

        let regs = rename_core(&ssh, &rdp, &reg, "prod", "production")
            .expect("un bloc multi-alias en casse mélangée ne doit pas faire échouer le renommage");

        let hosts = crate::parse_config_str(&std::fs::read_to_string(&ssh).unwrap());
        let f = |al: &str| {
            hosts
                .iter()
                .find(|h| h.alias == al)
                .map(|h| h.folder.clone())
        };
        assert_eq!(f("a").as_deref(), Some("production"));
        assert!(regs.contains(&"production".to_string()));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// `delete_core` partage le helper : le même bloc `Host\tx y` faisait
    /// échouer la suppression du dossier avec « n'ont pas pu être ramenés à la
    /// racine ». Test symétrique du précédent. Audit du 7 septembre 2026.
    #[test]
    fn delete_core_ignore_un_bloc_multi_alias_tabule() {
        let d = scratch();
        let (ssh, rdp, reg) = (d.join("config"), d.join("rdp.yaml"), d.join("folders.yaml"));
        std::fs::write(
            &ssh,
            "Host a\n    HostName 1\n    #Folder: prod\n\nHost\tx y\n    HostName 4\n    #Folder: prod\n",
        )
        .unwrap();
        create_in(&reg, "prod").unwrap();

        let regs = delete_core(&ssh, &rdp, &reg, "prod")
            .expect("un bloc multi-alias tabulé ne doit pas faire échouer la suppression");

        let hosts = crate::parse_config_str(&std::fs::read_to_string(&ssh).unwrap());
        let f = |al: &str| {
            hosts
                .iter()
                .find(|h| h.alias == al)
                .map(|h| h.folder.clone())
        };
        assert_eq!(f("a").as_deref(), Some("")); // ramené à la racine
        assert!(!regs.iter().any(|p| p.starts_with("prod")));
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

    /// Trouvé par l'audit du 7 septembre 2026 : `rename_core` ne parsait que le
    /// fichier principal (`parse_config_str`, qui ignore `Include`), alors que
    /// l'arbre affiché résout les Include (`parse_ssh_config`). Un hôte d'un
    /// fichier inclus portant `#Folder: prod` — typiquement un bloc rangé par
    /// Avash dans le fichier principal, puis déplacé à la main dans `conf.d/`
    /// avec son marqueur — n'était ni remappé ni signalé : après `prod` →
    /// `production`, l'ancien dossier `prod` réapparaissait dans l'arbre avec lui
    /// (l'arbre est dérivé des hôtes). L'opération est désormais refusée en
    /// nommant l'hôte, sans toucher au fichier principal (sonde avant écriture).
    #[test]
    fn rename_core_signale_un_hote_inclus() {
        let d = scratch();
        let confd = d.join("conf.d");
        std::fs::create_dir_all(&confd).unwrap();
        let (ssh, rdp, reg) = (d.join("config"), d.join("rdp.yaml"), d.join("folders.yaml"));
        std::fs::write(
            &ssh,
            "Include conf.d/*\n\nHost local\n    HostName 1\n    #Folder: prod\n",
        )
        .unwrap();
        std::fs::write(
            confd.join("clients"),
            "Host acme\n    HostName 2\n    #Folder: prod\n",
        )
        .unwrap();
        create_in(&reg, "prod").unwrap();

        let e = rename_core(&ssh, &rdp, &reg, "prod", "production")
            .expect_err("un hôte d'un fichier inclus doit être signalé")
            .to_string();
        assert!(e.contains("acme"), "l'hôte inclus doit être nommé : {e}");
        // Sonde avant écriture : le fichier principal ne doit pas avoir bougé.
        let contenu = std::fs::read_to_string(&ssh).unwrap();
        assert!(
            contenu.contains("#Folder: prod"),
            "le fichier principal ne doit pas être remappé : {contenu}"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// `delete_core` partage le trou : un hôte d'un fichier inclus dans le
    /// dossier supprimé restait dans `prod`, l'opération annonçant le succès.
    /// Symétrique du test de renommage. Audit du 7 septembre 2026.
    #[test]
    fn delete_core_signale_un_hote_inclus() {
        let d = scratch();
        let confd = d.join("conf.d");
        std::fs::create_dir_all(&confd).unwrap();
        let (ssh, rdp, reg) = (d.join("config"), d.join("rdp.yaml"), d.join("folders.yaml"));
        std::fs::write(
            &ssh,
            "Include conf.d/*\n\nHost local\n    HostName 1\n    #Folder: prod\n",
        )
        .unwrap();
        std::fs::write(
            confd.join("clients"),
            "Host acme\n    HostName 2\n    #Folder: prod\n",
        )
        .unwrap();
        create_in(&reg, "prod").unwrap();

        let e = delete_core(&ssh, &rdp, &reg, "prod")
            .expect_err("un hôte d'un fichier inclus doit être signalé")
            .to_string();
        assert!(e.contains("acme"), "l'hôte inclus doit être nommé : {e}");
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

    /// Trouvé par l'audit du 7 septembre 2026 : un `rdp.yaml` illisible (ici un
    /// YAML syntaxiquement cassé, mais aussi bien un champ requis retiré à la
    /// main) était avalé en silence par `remap_rdp`. Le renommage aboutissait,
    /// le registre était réécrit, l'interface annonçait le succès, puis les
    /// bureaux réapparaissaient dans l'ancien dossier dès le fichier réparé.
    /// L'erreur doit maintenant être signalée ET `~/.ssh/config` rester intact
    /// (sonde avant écriture) : c'est ce qui distingue un échec propre d'un
    /// renommage à moitié appliqué.
    #[test]
    fn rename_core_signale_un_rdp_yaml_corrompu() {
        let d = scratch();
        let (ssh, rdp, reg) = (d.join("config"), d.join("rdp.yaml"), d.join("folders.yaml"));
        let ssh_avant = "Host web-1\n    HostName 1.1.1.1\n    #Folder: prod\n";
        std::fs::write(&ssh, ssh_avant).unwrap();
        std::fs::write(&rdp, "- id: [\n").unwrap();
        create_in(&reg, "prod").unwrap();

        let e = rename_core(&ssh, &rdp, &reg, "prod", "production")
            .expect_err("un rdp.yaml illisible doit être signalé")
            .to_string();
        assert!(
            e.contains("rdp.yaml"),
            "le message doit nommer rdp.yaml : {e}"
        );
        assert_eq!(
            std::fs::read_to_string(&ssh).unwrap(),
            ssh_avant,
            "~/.ssh/config ne doit pas avoir bougé : la sonde est faite avant écriture"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Même défaut côté suppression : un `rdp.yaml` illisible laissait les
    /// bureaux dans un dossier supprimé sans le dire.
    #[test]
    fn delete_core_signale_un_rdp_yaml_corrompu() {
        let d = scratch();
        let (ssh, rdp, reg) = (d.join("config"), d.join("rdp.yaml"), d.join("folders.yaml"));
        let ssh_avant = "Host web-1\n    HostName 1.1.1.1\n    #Folder: prod\n";
        std::fs::write(&ssh, ssh_avant).unwrap();
        std::fs::write(&rdp, "- id: [\n").unwrap();
        create_in(&reg, "prod").unwrap();

        let e = delete_core(&ssh, &rdp, &reg, "prod")
            .expect_err("un rdp.yaml illisible doit être signalé")
            .to_string();
        assert!(
            e.contains("rdp.yaml"),
            "le message doit nommer rdp.yaml : {e}"
        );
        assert_eq!(
            std::fs::read_to_string(&ssh).unwrap(),
            ssh_avant,
            "~/.ssh/config ne doit pas avoir bougé : la sonde est faite avant écriture"
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
