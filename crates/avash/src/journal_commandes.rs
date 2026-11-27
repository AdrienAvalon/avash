//! Journal des commandes passées dans les sessions SSH.
//!
//! Demandé par le mainteneur le 24 novembre 2026 : « avoir un log des
//! commandes passées en ssh ». Ce qui est noté, c'est la ligne de commande
//! telle que le terminal l'affichait au moment où elle est partie, relevée par
//! le front (`web/journal-commandes.ts`) : la commande réellement exécutée,
//! complétion et rappel d'historique compris, et jamais un mot de passe tapé à
//! l'invite de `sudo`, qui ne s'affiche pas. Les frappes brutes ne sont pas
//! notées.
//!
//! Le journal est **coupé par défaut**. Il tient dans
//! `<config>/avash/journal-commandes/`, un fichier par jour (temps universel)
//! au format JSON Lines, né en 0600 dans un répertoire 0700, et purgé au-delà
//! d'une durée réglable ([`RETENTION_DEFAUT_JOURS`] par défaut).
//!
//! Une ligne est soit une commande, soit le code de retour d'une commande
//! précédente de la même session (quand le shell distant envoie les
//! marqueurs OSC 133, le code arrive à la fin de la commande, parfois des
//! heures après) : le journal n'est jamais réécrit, seulement prolongé.
//!
//! Avant d'écrire, [`masquer_secrets`] remplace les secrets écrits en clair
//! dans la commande elle-même (`mysql -pMOTDEPASSE`, `export GITHUB_TOKEN=…`,
//! en-tête `Authorization:`, identifiants dans une URL). C'est une heuristique
//! : SECURITY.md dit ce qu'elle couvre et ce qu'elle ne couvre pas.

use anyhow::{Context, Result};
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Durée de conservation par défaut, en jours.
pub const RETENTION_DEFAUT_JOURS: u32 = 90;
/// Durée de conservation la plus longue qu'on accepte : dix ans.
pub const RETENTION_MAX_JOURS: u32 = 3650;
/// Une commande plus longue est tronquée : un journal n'a pas à garder un
/// collage de plusieurs mégaoctets.
pub const COMMANDE_MAX: usize = 4096;
/// Ce qui remplace un secret repéré dans une commande.
pub const MASQUE: &str = "••••";
/// Le nombre de commandes rendues au plus à la fenêtre du journal.
pub const LECTURE_MAX: usize = 20_000;

/// Une commande du journal, telle que la fenêtre la montre.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Entree {
    /// Quand elle est partie, en secondes depuis l'époque.
    pub t: u64,
    /// L'identité de la session qui l'a passée : deux onglets ouverts sur le
    /// même hôte ne se mélangent pas.
    pub s: u64,
    /// Le libellé de l'onglet : l'alias de l'hôte, ou `user@hôte`.
    pub session: String,
    /// Où la session est connectée : `utilisateur@adresse:port`.
    pub cible: String,
    pub commande: String,
    /// Le code de retour, si le shell distant l'a signalé (OSC 133).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<i32>,
}

/// Une ligne du fichier : une commande, ou le code d'une commande précédente
/// de la même session.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Ligne {
    t: u64,
    s: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cible: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    commande: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    code: Option<i32>,
}

/// Le répertoire du journal : `<config>/avash/journal-commandes`, et sous
/// Windows `%LOCALAPPDATA%\avash\journal-commandes`, hors du profil itinérant
/// (audit du 27 novembre 2026, SE-5 ; voir `donnees_locales`).
#[must_use]
pub fn repertoire() -> Option<PathBuf> {
    crate::donnees_locales::repertoire_de_base().map(|c| c.join("avash").join("journal-commandes"))
}

/// Secondes depuis l'époque.
#[must_use]
pub fn maintenant() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Le fichier du jour (temps universel) d'un instant : `2026-11-24.jsonl`.
#[must_use]
pub fn nom_du_jour(secondes: u64) -> String {
    let h = crate::enregistrement::horodatage(secondes);
    format!("{}-{}-{}.jsonl", &h[0..4], &h[4..6], &h[6..8])
}

/// Le jour d'un nom de fichier du journal, en jours depuis l'époque ; `None`
/// pour un fichier qui n'en est pas un. Seuls ces noms-là sont lus et purgés :
/// un fichier posé à la main dans le répertoire n'est jamais effacé.
#[must_use]
pub fn jour_du_nom(nom: &str) -> Option<i64> {
    let date = nom.strip_suffix(".jsonl")?;
    let b = date.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let chiffres = |r: std::ops::Range<usize>| -> Option<i64> {
        let s = &date[r];
        s.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| s.parse().ok())?
    };
    let (a, m, j) = (chiffres(0..4)?, chiffres(5..7)?, chiffres(8..10)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&j) {
        return None;
    }
    Some(jours_depuis_epoque(a, m, j))
}

/// Jours depuis le 1er janvier 1970 d'une date civile (algorithme de Howard
/// Hinnant, l'inverse de celui de [`crate::enregistrement::horodatage`]).
fn jours_depuis_epoque(annee: i64, mois: i64, jour: i64) -> i64 {
    let a = if mois <= 2 { annee - 1 } else { annee };
    let ere = a.div_euclid(400);
    let annee_de_l_ere = a - ere * 400;
    let m = if mois > 2 { mois - 3 } else { mois + 9 };
    let jour_de_l_annee = (153 * m + 2) / 5 + jour - 1;
    let jour_de_l_ere =
        annee_de_l_ere * 365 + annee_de_l_ere / 4 - annee_de_l_ere / 100 + jour_de_l_annee;
    ere * 146_097 + jour_de_l_ere - 719_468
}

/// La commande telle qu'elle sera notée, ou `None` s'il n'y a rien à noter.
///
/// Les caractères de contrôle sont retirés (une tabulation devient une
/// espace) : rien de ce qui est relu dans la fenêtre ne doit pouvoir y jouer
/// une séquence. Une commande qui commence par une espace n'est pas notée,
/// comme le fait `HISTCONTROL=ignorespace` : c'est le moyen de passer une
/// commande sans trace. Les secrets repérés sont masqués, puis la commande est
/// tronquée à [`COMMANDE_MAX`] caractères.
#[must_use]
pub fn preparer(commande: &str) -> Option<String> {
    if commande.starts_with([' ', '\t']) {
        return None;
    }
    let propre: String = commande
        .chars()
        .map(|c| if c == '\t' { ' ' } else { c })
        .filter(|c| !c.is_control())
        .collect();
    let propre = propre.trim_end();
    if propre.trim().is_empty() {
        return None;
    }
    let masque = masquer_secrets(propre);
    if masque.chars().count() > COMMANDE_MAX {
        let mut court: String = masque.chars().take(COMMANDE_MAX).collect();
        court.push('…');
        return Some(court);
    }
    Some(masque)
}

/// Ouvre (ou crée) le fichier du jour en ajout, 0600 dans un répertoire 0700.
fn ouvrir_le_jour(dir: &Path, secondes: u64) -> Result<std::fs::File> {
    std::fs::create_dir_all(dir).with_context(|| format!("création de {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    let chemin = dir.join(nom_du_jour(secondes));
    let mut options = std::fs::OpenOptions::new();
    options.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let fichier = options
        .open(&chemin)
        .with_context(|| format!("ouverture de {}", chemin.display()))?;
    // `mode` ne vaut qu'à la création : un fichier du jour retrouvé plus
    // ouvert (copié d'ailleurs, restauré d'une sauvegarde) est refermé.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = fichier.set_permissions(std::fs::Permissions::from_mode(0o600));
    }
    Ok(fichier)
}

/// Écrit une ligne d'un seul `write` : en mode ajout, deux sessions qui notent
/// en même temps ne s'entrelacent pas au milieu d'une ligne.
fn ecrire_ligne(dir: &Path, ligne: &Ligne) -> Result<()> {
    let mut texte = serde_json::to_string(ligne)?;
    texte.push('\n');
    let mut f = ouvrir_le_jour(dir, ligne.t)?;
    f.write_all(texte.as_bytes())
        .context("écriture dans le journal des commandes")
}

/// Note une commande dans le journal de `dir`. Rend `false` quand il n'y
/// avait rien à noter (voir [`preparer`]).
pub fn noter_dans(
    dir: &Path,
    t: u64,
    s: u64,
    session: &str,
    cible: &str,
    commande: &str,
) -> Result<bool> {
    let Some(commande) = preparer(commande) else {
        return Ok(false);
    };
    ecrire_ligne(
        dir,
        &Ligne {
            t,
            s,
            session: Some(session.chars().filter(|c| !c.is_control()).collect()),
            cible: Some(cible.chars().filter(|c| !c.is_control()).collect()),
            commande: Some(commande),
            code: None,
        },
    )?;
    Ok(true)
}

/// Note le code de retour de la dernière commande de la session `s`.
pub fn noter_code_dans(dir: &Path, t: u64, s: u64, code: i32) -> Result<()> {
    ecrire_ligne(
        dir,
        &Ligne {
            t,
            s,
            session: None,
            cible: None,
            commande: None,
            code: Some(code),
        },
    )
}

/// Les fichiers du journal de `dir`, avec leur jour, du plus ancien au plus
/// récent.
fn fichiers(dir: &Path) -> Vec<(i64, PathBuf)> {
    let Ok(entrees) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<(i64, PathBuf)> = entrees
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let nom = e.file_name();
            Some((jour_du_nom(nom.to_str()?)?, e.path()))
        })
        .collect();
    v.sort();
    v
}

/// Les commandes du journal de `dir`, de la plus récente à la plus ancienne,
/// [`LECTURE_MAX`] au plus. Une ligne illisible (fichier tronqué par une
/// coupure, retouché à la main) est sautée, pas fatale.
#[must_use]
pub fn lire_dans(dir: &Path) -> Vec<Entree> {
    let mut entrees: Vec<Entree> = Vec::new();
    // Dernière commande de chaque session encore sans code : c'est à elle
    // qu'un code qui suit se rattache.
    let mut ouvertes: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    for (_, chemin) in fichiers(dir) {
        let Ok(texte) = std::fs::read_to_string(&chemin) else {
            continue;
        };
        for ligne in texte.lines() {
            let Ok(l) = serde_json::from_str::<Ligne>(ligne) else {
                continue;
            };
            if let Some(commande) = l.commande {
                ouvertes.insert(l.s, entrees.len());
                entrees.push(Entree {
                    t: l.t,
                    s: l.s,
                    session: l.session.unwrap_or_default(),
                    cible: l.cible.unwrap_or_default(),
                    commande,
                    code: None,
                });
            } else if let Some(code) = l.code {
                if let Some(i) = ouvertes.remove(&l.s) {
                    entrees[i].code = Some(code);
                }
            }
        }
    }
    entrees.reverse();
    entrees.truncate(LECTURE_MAX);
    entrees
}

/// Efface les fichiers du journal plus vieux que `jours` jours (le jour en
/// cours compte pour un) et rend le nombre de fichiers effacés.
pub fn purger_dans(dir: &Path, jours: u32, maintenant: u64) -> Result<usize> {
    let jours = jours.clamp(1, RETENTION_MAX_JOURS);
    let aujourd_hui = i64::try_from(maintenant / 86_400).unwrap_or(i64::MAX);
    let limite = aujourd_hui - i64::from(jours);
    let mut effaces = 0;
    for (jour, chemin) in fichiers(dir) {
        if jour <= limite {
            std::fs::remove_file(&chemin)
                .with_context(|| format!("effacement de {}", chemin.display()))?;
            effaces += 1;
        }
    }
    Ok(effaces)
}

/// Le journal en texte, pour l'export : une commande par ligne, la plus
/// ancienne d'abord, l'heure locale donnée par `decalage_minutes` (minutes à
/// ajouter au temps universel ; `+60` à Paris en hiver).
#[must_use]
pub fn en_texte(entrees: &[Entree], decalage_minutes: i32) -> String {
    use std::fmt::Write as _;
    let decalage = i64::from(decalage_minutes.clamp(-14 * 60, 14 * 60));
    let signe = if decalage < 0 { '-' } else { '+' };
    let fuseau = format!(
        "{signe}{:02}:{:02}",
        decalage.abs() / 60,
        decalage.abs() % 60
    );
    let mut sortie = String::new();
    for e in entrees.iter().rev() {
        let local = i64::try_from(e.t).unwrap_or(0) + decalage * 60;
        let h = crate::enregistrement::horodatage(u64::try_from(local.max(0)).unwrap_or(0));
        let code = e.code.map_or(String::new(), |c| format!(" [{c}]"));
        let _ = writeln!(
            sortie,
            "{}-{}-{} {}:{}:{}{fuseau}  {} ({}){code}  {}",
            &h[0..4],
            &h[4..6],
            &h[6..8],
            &h[9..11],
            &h[11..13],
            &h[13..15],
            e.session,
            e.cible,
            e.commande
        );
    }
    sortie
}

// ---------- Masquage des secrets écrits dans une commande ----------

/// Les commandes de la famille MySQL, où `-pSECRET` (collé) est le mot de
/// passe. Ailleurs, `-p` veut dire autre chose (`mkdir -p`, `ssh -p 22`).
const CLIENTS_MYSQL: &[&str] = &[
    "mysql",
    "mariadb",
    "mysqldump",
    "mariadb-dump",
    "mysqladmin",
    "mariadb-admin",
    "mysqlimport",
    "mysqlshow",
    "mysqlcheck",
];

/// Un nom (variable ou option) qui désigne un secret.
fn nom_de_secret(nom: &str) -> bool {
    let n = nom.to_ascii_lowercase().replace('-', "_");
    if n == "pwd" || n == "oldpwd" {
        return false; // le répertoire courant, pas un mot de passe
    }
    [
        "pass",
        "pwd",
        "token",
        "secret",
        "api_key",
        "apikey",
        "credential",
        "private_key",
    ]
    .iter()
    .any(|m| n.contains(m))
}

/// Une option au nom de secret qui ne prend pas le secret en argument :
/// `--password-stdin`, `--password-file`, `--no-password`, `--ask-pass`.
fn option_sans_valeur_secrete(nom: &str) -> bool {
    let n = nom.to_ascii_lowercase().replace('-', "_");
    n.starts_with("no_")
        || n.starts_with("ask")
        || ["file", "stdin", "prompt", "fd", "env", "cmd", "command"]
            .iter()
            .any(|f| n.ends_with(f))
}

/// Les mots d'une commande avec leur position, les guillemets respectés : un
/// mot entre guillemets peut contenir des espaces.
fn mots(commande: &str) -> Vec<(usize, usize)> {
    let mut v = Vec::new();
    let mut debut: Option<usize> = None;
    let mut guillemet: Option<char> = None;
    let mut echappe = false;
    for (i, c) in commande.char_indices() {
        if echappe {
            echappe = false;
            continue;
        }
        match (guillemet, c) {
            (_, '\\') if guillemet != Some('\'') => {
                echappe = true;
                debut.get_or_insert(i);
            }
            (Some(g), c) if c == g => guillemet = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => {
                guillemet = Some(c);
                debut.get_or_insert(i);
            }
            (None, c) if c.is_whitespace() => {
                if let Some(d) = debut.take() {
                    v.push((d, i));
                }
            }
            (None, _) => {
                debut.get_or_insert(i);
            }
        }
    }
    if let Some(d) = debut {
        v.push((d, commande.len()));
    }
    v
}

/// Masque une valeur en gardant ses guillemets : `"abc"` devient `"••••"`.
fn masquer_valeur(valeur: &str) -> String {
    match valeur.chars().next() {
        Some(q @ ('"' | '\'')) if valeur.len() >= 2 && valeur.ends_with(q) => {
            format!("{q}{MASQUE}{q}")
        }
        _ => MASQUE.to_owned(),
    }
}

/// Le mot sans guillemets autour, pour le reconnaître.
fn nu(mot: &str) -> &str {
    mot.trim_matches(['"', '\''])
}

/// Masque, dans un mot, la partie mot de passe d'une URL
/// `schéma://utilisateur:motdepasse@hôte`.
fn masquer_url(mot: &str) -> Option<String> {
    let debut = mot.find("://")? + 3;
    let reste = &mot[debut..];
    let fin_autorite = reste
        .find(['/', '?', '#', '"', '\''])
        .unwrap_or(reste.len());
    let autorite = &reste[..fin_autorite];
    let arobase = autorite.rfind('@')?;
    let deux_points = autorite[..arobase].find(':')?;
    if deux_points + 1 == arobase {
        return None; // `user:@hôte` : rien à cacher
    }
    Some(format!(
        "{}{}{}",
        &mot[..=debut + deux_points],
        MASQUE,
        &mot[debut + arobase..]
    ))
}

/// Masque ce qui suit un en-tête d'authentification dans un mot :
/// `"Authorization: Bearer abc"` devient `"Authorization: Bearer ••••"`.
fn masquer_entete(mot: &str) -> Option<String> {
    let bas = mot.to_ascii_lowercase();
    let (pos, nom) = [
        "authorization:",
        "proxy-authorization:",
        "private-token:",
        "x-api-key:",
        "x-auth-token:",
        "cookie:",
    ]
    .iter()
    .find_map(|n| bas.find(n).map(|p| (p, n.len())))?;
    let apres = pos + nom;
    let fin = mot.len() - usize::from(mot.ends_with(['"', '\'']) && pos > 0);
    let valeur = &mot[apres..fin];
    let espaces = valeur.len() - valeur.trim_start().len();
    let valeur = valeur.trim_start();
    // Le schéma (`Bearer`, `Basic`…) dit de quoi il s'agit sans rien livrer.
    let garde = match valeur.split_once(' ') {
        Some((schema, reste))
            if !reste.trim().is_empty()
                && ["bearer", "basic", "token", "digest", "negotiate"]
                    .contains(&schema.to_ascii_lowercase().as_str()) =>
        {
            schema.len() + 1
        }
        _ => 0,
    };
    if valeur.len() == garde {
        return None;
    }
    Some(format!(
        "{}{}{}{}",
        &mot[..apres + espaces],
        &valeur[..garde],
        MASQUE,
        &mot[fin..]
    ))
}

/// Remplace par [`MASQUE`] les secrets écrits en clair dans une commande.
///
/// Couvert : `NOM=valeur` quand le nom désigne un secret (`PGPASSWORD=…`,
/// `export GITHUB_TOKEN=…`), les options `--password=…`, `--token …` et
/// semblables, `-pSECRET` des clients MySQL, `sshpass -p SECRET`, `-u
/// utilisateur:motdepasse` et `--user …` de `curl`, les en-têtes
/// `Authorization:` et apparentés, les identifiants dans une URL. Pas couvert :
/// un secret passé sans nom qui le désigne (`echo motdepasse | sudo -S …`,
/// argument positionnel), que rien ne distingue d'un mot ordinaire.
#[must_use]
pub fn masquer_secrets(commande: &str) -> String {
    let positions = mots(commande);
    let textes: Vec<&str> = positions.iter().map(|&(d, f)| &commande[d..f]).collect();
    let mut remplacements: Vec<Option<String>> = vec![None; textes.len()];
    // Les programmes vus dans le segment en cours (jusqu'au prochain `|`, `;`,
    // `&&`, `||`) : `sudo -u root mysql -pX` est une commande mysql même si
    // son premier mot est `sudo`.
    let mut segment: Vec<String> = Vec::new();
    let dans =
        |segment: &[String], noms: &[&str]| segment.iter().any(|p| noms.contains(&p.as_str()));
    // Le mot suivant est la valeur d'une option secrète ; `Some(true)` quand
    // c'est un `utilisateur:motdepasse` dont l'utilisateur reste lisible.
    let mut valeur_suivante: Option<bool> = None;
    for (i, mot) in textes.iter().enumerate() {
        let brut = nu(mot);
        if let Some(utilisateur_lisible) = valeur_suivante.take() {
            // Une option qui suit n'est pas la valeur attendue.
            if !brut.starts_with('-') {
                remplacements[i] = Some(match brut.split_once(':') {
                    Some((user, _)) if utilisateur_lisible => format!("{user}:{MASQUE}"),
                    _ => masquer_valeur(mot),
                });
                continue;
            }
        }
        if matches!(brut, "|" | ";" | "&&" | "||" | "&") {
            segment.clear();
            continue;
        }
        // `NOM=valeur`, en tête de commande ou après `export` ou `env`.
        if let Some((nom, valeur)) = brut.split_once('=') {
            if !nom.is_empty()
                && nom.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && nom_de_secret(nom)
                && !valeur.is_empty()
            {
                let pos = mot.find('=').map_or(0, |p| p + 1);
                remplacements[i] = Some(format!("{}{}", &mot[..pos], masquer_valeur(&mot[pos..])));
                continue;
            }
        }
        if !brut.starts_with('-') && !brut.contains('=') {
            segment.push(brut.rsplit('/').next().unwrap_or(brut).to_ascii_lowercase());
        }
        let curl = dans(&segment, &["curl"]);
        // Options longues : `--password=x`, `--password x`, `--user u:p`.
        if let Some(option) = brut.strip_prefix("--") {
            match option.split_once('=') {
                Some(("user" | "proxy-user", valeur)) if curl => {
                    if let Some((user, _)) = valeur.split_once(':') {
                        let pos = mot.find('=').map_or(0, |p| p + 1);
                        let q = if mot.ends_with(['"', '\'']) {
                            &mot[mot.len() - 1..]
                        } else {
                            ""
                        };
                        remplacements[i] = Some(format!("{}{user}:{MASQUE}{q}", &mot[..pos]));
                    }
                    continue;
                }
                Some((nom, valeur)) if nom_de_secret(nom) && !valeur.is_empty() => {
                    let pos = mot.find('=').map_or(0, |p| p + 1);
                    remplacements[i] =
                        Some(format!("{}{}", &mot[..pos], masquer_valeur(&mot[pos..])));
                    continue;
                }
                None if nom_de_secret(option) && !option_sans_valeur_secrete(option) => {
                    valeur_suivante = Some(false);
                    continue;
                }
                None if matches!(option, "user" | "proxy-user") && curl => {
                    valeur_suivante = Some(true);
                    continue;
                }
                _ => {}
            }
        }
        let (mysql, sshpass) = (dans(&segment, CLIENTS_MYSQL), dans(&segment, &["sshpass"]));
        match option_courte(brut, mysql, sshpass, curl) {
            Some(Courte::Masquer(r)) => {
                remplacements[i] = Some(r);
                continue;
            }
            Some(Courte::ValeurSuivante(utilisateur_lisible)) => {
                valeur_suivante = Some(utilisateur_lisible);
                continue;
            }
            None => {}
        }
        if let Some(m) = masquer_entete(mot) {
            remplacements[i] = Some(m);
            continue;
        }
        if let Some(m) = masquer_url(mot) {
            remplacements[i] = Some(m);
        }
    }
    recoudre(commande, &positions, remplacements)
}

/// Ce qu'une option courte demande au masquage.
enum Courte {
    /// Le mot entier, secret masqué.
    Masquer(String),
    /// Le mot suivant est la valeur secrète ; `true` quand c'est un
    /// `utilisateur:motdepasse` dont l'utilisateur reste lisible.
    ValeurSuivante(bool),
}

/// Les options courtes de `curl` qui prennent une valeur : dans un groupe
/// (`-sko fichier`), ce qui suit l'une d'elles est sa valeur, pas d'autres
/// options.
const CURL_AVEC_VALEUR: &str = "AbcCdDeEFHKmoPQrtTuUwxXyYz";
/// Les options courtes des clients MySQL qui prennent une valeur.
const MYSQL_AVEC_VALEUR: &str = "DehOPSu";
/// Les options courtes de `sshpass` qui prennent une valeur.
const SSHPASS_AVEC_VALEUR: &str = "dfP";

/// Dans un groupe d'options courtes (`-su`, `-vpSECRET`), la position qui suit
/// la première des `cibles`, si aucune option à valeur ne la précède.
fn dans_le_groupe(brut: &str, cibles: &[char], avec_valeur: &str) -> Option<usize> {
    let groupe = brut.strip_prefix('-').filter(|g| !g.starts_with('-'))?;
    for (i, c) in groupe.char_indices() {
        if cibles.contains(&c) {
            return Some(1 + i + c.len_utf8());
        }
        if avec_valeur.contains(c) || !c.is_ascii_alphanumeric() {
            return None;
        }
    }
    None
}

/// Les options courtes qui portent un mot de passe, seules ou dans un groupe :
/// `-pSECRET` des clients MySQL (`-vpSECRET`), `sshpass -p SECRET` et
/// `-pSECRET`, `curl -u u:p`, `-uu:p`, `-su u:p` et `-U` (mandataire). Les
/// formes collées, puis groupées, passaient en clair (revues de sécurité
/// automatiques de la PR #113). `-p` seul d'un client MySQL et `-uutilisateur`
/// sans `:` demandent le mot de passe à l'invite : rien à cacher.
fn option_courte(brut: &str, mysql: bool, sshpass: bool, curl: bool) -> Option<Courte> {
    if mysql {
        if let Some(pos) = dans_le_groupe(brut, &['p'], MYSQL_AVEC_VALEUR) {
            if pos < brut.len() {
                return Some(Courte::Masquer(format!("{}{MASQUE}", &brut[..pos])));
            }
        }
    }
    if sshpass {
        if let Some(pos) = dans_le_groupe(brut, &['p'], SSHPASS_AVEC_VALEUR) {
            return Some(if pos < brut.len() {
                Courte::Masquer(format!("{}{MASQUE}", &brut[..pos]))
            } else {
                Courte::ValeurSuivante(false)
            });
        }
    }
    if curl {
        if let Some(pos) = dans_le_groupe(brut, &['u', 'U'], CURL_AVEC_VALEUR) {
            if pos == brut.len() {
                return Some(Courte::ValeurSuivante(true));
            }
            if let Some((user, _)) = brut[pos..].split_once(':') {
                return Some(Courte::Masquer(format!("{}{user}:{MASQUE}", &brut[..pos])));
            }
        }
    }
    None
}

/// La commande avec ses mots remplacés, le reste (espaces, séparateurs) tel quel.
fn recoudre(
    commande: &str,
    positions: &[(usize, usize)],
    remplacements: Vec<Option<String>>,
) -> String {
    let mut sortie = String::with_capacity(commande.len());
    let mut curseur = 0;
    for ((debut, fin), r) in positions.iter().zip(remplacements) {
        if let Some(r) = r {
            sortie.push_str(&commande[curseur..*debut]);
            sortie.push_str(&r);
            curseur = *fin;
        }
    }
    sortie.push_str(&commande[curseur..]);
    sortie
}

#[cfg(test)]
#[path = "tests_lib/tests_journal_commandes.rs"]
mod tests;
