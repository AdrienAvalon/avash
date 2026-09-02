//! Enregistrement d'une session de terminal au format asciicast v2, celui
//! d'`asciinema` : relisible par `asciinema play`, partageable, et surtout
//! traçable — une revue d'incident se rejoue au lieu de se raconter.
//!
//! Le fichier commence par une ligne d'en-tête JSON, puis une ligne par
//! événement : `[secondes, "o", texte]` pour ce que le terminal affiche,
//! `[secondes, "r", "COLSxROWS"]` pour un redimensionnement. **Seule la sortie
//! est enregistrée**, jamais les frappes : un mot de passe tapé à l'invite d'un
//! `sudo` n'apparaît pas à l'écran, il ne doit pas apparaître dans le fichier.
//!
//! Le fichier naît en 0600 dans le répertoire de configuration, et chaque
//! événement est écrit et vidé aussitôt : une coupure laisse un enregistrement
//! tronqué mais lisible jusqu'à la dernière ligne complète.

use anyhow::{Context, Result};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Un enregistrement en cours : un fichier ouvert et l'instant de départ.
pub struct Enregistreur {
    fichier: std::io::BufWriter<std::fs::File>,
    chemin: PathBuf,
    depart: Instant,
    octets: u64,
}

impl std::fmt::Debug for Enregistreur {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Enregistreur")
            .field("chemin", &self.chemin)
            .field("octets", &self.octets)
            .finish_non_exhaustive()
    }
}

/// Le répertoire des enregistrements : `<config>/avash/enregistrements`.
#[must_use]
pub fn repertoire() -> Option<PathBuf> {
    crate::repertoire_configuration().map(|c| c.join("avash").join("enregistrements"))
}

/// Un nom de fichier sûr, dérivé du libellé de la session et de l'heure :
/// `prod-web-20260902-143012.cast`. Le libellé est réduit aux caractères qui
/// ne posent de question sur aucun système de fichiers.
#[must_use]
pub fn nom_de_fichier(libelle: &str, horodatage: &str) -> String {
    let mut base: String = libelle
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    while base.contains("--") {
        base = base.replace("--", "-");
    }
    let base = base.trim_matches(['-', '.']);
    let base = if base.is_empty() { "session" } else { base };
    format!(
        "{}-{horodatage}.cast",
        base.chars().take(48).collect::<String>()
    )
}

fn secondes_epoque() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn horodatage_maintenant() -> String {
    horodatage(secondes_epoque())
}

/// `AAAAMMJJ-HHMMSS` en temps universel pour un instant en secondes depuis
/// l'époque, sans dépendance : l'algorithme des jours civils de Howard Hinnant.
#[must_use]
pub fn horodatage(secondes: u64) -> String {
    let jours = secondes / 86_400;
    let dans_le_jour = secondes % 86_400;
    let (heures, minutes, secs) = (
        dans_le_jour / 3600,
        (dans_le_jour % 3600) / 60,
        dans_le_jour % 60,
    );
    let depuis_0000_03_01 = jours + 719_468;
    let ere = depuis_0000_03_01 / 146_097;
    let jour_de_l_ere = depuis_0000_03_01 - ere * 146_097;
    let annee_de_l_ere = (jour_de_l_ere - jour_de_l_ere / 1460 + jour_de_l_ere / 36_524
        - jour_de_l_ere / 146_096)
        / 365;
    let jour_de_l_annee =
        jour_de_l_ere - (365 * annee_de_l_ere + annee_de_l_ere / 4 - annee_de_l_ere / 100);
    let mois_decale = (5 * jour_de_l_annee + 2) / 153;
    let jour = jour_de_l_annee - (153 * mois_decale + 2) / 5 + 1;
    let mois = if mois_decale < 10 {
        mois_decale + 3
    } else {
        mois_decale - 9
    };
    let annee = annee_de_l_ere + ere * 400 + u64::from(mois <= 2);
    format!("{annee:04}{mois:02}{jour:02}-{heures:02}{minutes:02}{secs:02}")
}

impl Enregistreur {
    /// Ouvre un nouvel enregistrement dans le répertoire par défaut.
    pub fn demarrer(libelle: &str, cols: u32, rows: u32) -> Result<Self> {
        let dir = repertoire().context("répertoire de configuration introuvable")?;
        Self::demarrer_dans(&dir, libelle, cols, rows)
    }

    /// Ouvre un nouvel enregistrement dans `dir` (créé au besoin, 0700).
    pub fn demarrer_dans(dir: &Path, libelle: &str, cols: u32, rows: u32) -> Result<Self> {
        std::fs::create_dir_all(dir).with_context(|| format!("création de {}", dir.display()))?;
        // Le répertoire ne regarde que son propriétaire : 0700, pas 0600 — un
        // répertoire sans droit de parcours ne laisse rien créer dedans.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
        let base = nom_de_fichier(libelle, &horodatage_maintenant());
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        // Deux enregistrements de la même session dans la même seconde — arrêter
        // puis redémarrer aussitôt — portent le même nom : le second est suffixé
        // plutôt que refusé. `create_new` garantit qu'on n'écrase jamais.
        let (chemin, fichier) = (1u32..)
            .map(|n| {
                if n == 1 {
                    dir.join(&base)
                } else {
                    dir.join(base.replace(".cast", &format!("-{n}.cast")))
                }
            })
            .take(1000)
            .find_map(|c| match options.open(&c) {
                Ok(f) => Some(Ok((c, f))),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(e) => Some(Err(
                    anyhow::Error::new(e).context(format!("création de {}", c.display()))
                )),
            })
            .unwrap_or_else(|| {
                Err(anyhow::anyhow!(
                    "mille enregistrements dans la même seconde"
                ))
            })?;
        let mut moi = Self {
            fichier: std::io::BufWriter::new(fichier),
            chemin,
            depart: Instant::now(),
            octets: 0,
        };
        let horodatage = secondes_epoque();
        let entete = serde_json::json!({
            "version": 2,
            "width": cols,
            "height": rows,
            "timestamp": horodatage,
            "title": libelle,
            "env": { "TERM": "xterm-256color", "SHELL": "" },
        });
        moi.ligne(&entete.to_string())?;
        Ok(moi)
    }

    fn secondes(&self) -> f64 {
        self.depart.elapsed().as_secs_f64()
    }

    fn ligne(&mut self, contenu: &str) -> Result<()> {
        self.fichier.write_all(contenu.as_bytes())?;
        self.fichier.write_all(b"\n")?;
        self.fichier.flush()?;
        Ok(())
    }

    /// Ce que le terminal vient d'afficher.
    pub fn sortie(&mut self, texte: &str) -> Result<()> {
        if texte.is_empty() {
            return Ok(());
        }
        self.octets += texte.len() as u64;
        let ev = serde_json::json!([self.secondes(), "o", texte]);
        self.ligne(&ev.to_string())
    }

    /// Le terminal a changé de taille.
    pub fn redimension(&mut self, cols: u32, rows: u32) -> Result<()> {
        let ev = serde_json::json!([self.secondes(), "r", format!("{cols}x{rows}")]);
        self.ligne(&ev.to_string())
    }

    /// Où l'enregistrement s'écrit.
    #[must_use]
    pub fn chemin(&self) -> &Path {
        &self.chemin
    }

    /// Volume de sortie enregistré, en octets de texte.
    #[must_use]
    pub fn octets(&self) -> u64 {
        self.octets
    }

    /// Termine proprement et rend le chemin du fichier.
    pub fn arreter(mut self) -> Result<PathBuf> {
        self.fichier.flush()?;
        Ok(self.chemin.clone())
    }
}

/// Relecture minimale d'un fichier asciicast v2 : l'en-tête et les événements.
/// Sert aux tests et à un futur lecteur intégré ; tolère une dernière ligne
/// tronquée, comme le ferait `asciinema play`.
/// Un enregistrement sur le disque, pour la liste.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct Info {
    pub chemin: PathBuf,
    pub nom: String,
    pub octets: u64,
    /// Dernière modification, en secondes depuis l'époque.
    pub modifie: u64,
}

/// Les enregistrements d'un répertoire, du plus récent au plus ancien. Un
/// répertoire absent donne une liste vide, pas une erreur : personne n'a
/// encore enregistré.
#[must_use]
pub fn lister(dir: &Path) -> Vec<Info> {
    let Ok(entrees) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    // Le tri se fait à la précision du système de fichiers, pas à la seconde :
    // deux enregistrements arrêtés puis relancés dans la même seconde doivent
    // sortir dans l'ordre où ils ont été écrits.
    let mut v: Vec<(std::time::Duration, Info)> = entrees
        .flatten()
        .filter_map(|e| {
            let chemin = e.path();
            if chemin.extension().and_then(|x| x.to_str()) != Some("cast") {
                return None;
            }
            let meta = e.metadata().ok()?;
            let precis = meta
                .modified()
                .ok()
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .unwrap_or_default();
            Some((
                precis,
                Info {
                    nom: chemin.file_name()?.to_str()?.to_string(),
                    chemin,
                    octets: meta.len(),
                    modifie: precis.as_secs(),
                },
            ))
        })
        .collect();
    v.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.nom.cmp(&a.1.nom)));
    v.into_iter().map(|(_, i)| i).collect()
}

/// Un événement relu : instant en secondes, type (`o` ou `r`), donnée.
pub type Evenement = (f64, String, String);

#[must_use]
pub fn relire(contenu: &str) -> Option<(serde_json::Value, Vec<Evenement>)> {
    let mut lignes = contenu.lines();
    let entete: serde_json::Value = serde_json::from_str(lignes.next()?).ok()?;
    let mut evenements = Vec::new();
    for l in lignes {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(l) else {
            break;
        };
        let (Some(t), Some(k), Some(d)) = (
            v.get(0).and_then(serde_json::Value::as_f64),
            v.get(1).and_then(serde_json::Value::as_str),
            v.get(2).and_then(serde_json::Value::as_str),
        ) else {
            break;
        };
        evenements.push((t, k.to_string(), d.to_string()));
    }
    Some((entete, evenements))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bac(nom: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("avash-cast-{}-{nom}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn le_nom_de_fichier_est_sur_et_porte_l_heure() {
        assert_eq!(
            nom_de_fichier("prod web", "20260902-143012"),
            "prod-web-20260902-143012.cast"
        );
        assert_eq!(
            nom_de_fichier("adrien@10.0.0.7:2222", "x"),
            "adrien-10.0.0.7-2222-x.cast"
        );
        assert_eq!(nom_de_fichier("../../etc", "x"), "etc-x.cast");
        assert_eq!(nom_de_fichier("", "x"), "session-x.cast");
        assert_eq!(nom_de_fichier("a\nb", "x"), "a-b-x.cast");
        assert_eq!(
            nom_de_fichier("a_b.c-d", "x"),
            "a_b.c-d-x.cast",
            "tiret, souligné, point gardés"
        );
        assert_eq!(
            nom_de_fichier("a  b", "x"),
            "a-b-x.cast",
            "deux tirets n'en font qu'un"
        );
    }

    #[test]
    fn l_horodatage_a_la_forme_attendue() {
        let h = horodatage_maintenant();
        assert_eq!(h.len(), 15, "{h}");
        assert!(h.starts_with("20"), "{h}");
        assert_eq!(&h[8..9], "-");
    }

    /// L'arithmétique du calendrier, contre des dates connues : le test de
    /// forme ci-dessus laissait survivre treize mutants de cette fonction
    /// (un `%` pour un `/`, un `+` pour un `-`) — cargo-mutants l'a montré.
    #[test]
    fn l_horodatage_donne_la_bonne_date_civile() {
        assert_eq!(horodatage(0), "19700101-000000");
        assert_eq!(horodatage(86_399), "19700101-235959");
        assert_eq!(
            horodatage(951_782_400),
            "20000229-000000",
            "année bissextile séculaire"
        );
        assert_eq!(horodatage(1_788_353_891), "20260902-125811");
        assert_eq!(horodatage(4_102_444_799), "20991231-235959");
        assert_eq!(
            horodatage(4_102_444_800),
            "21000101-000000",
            "2100 n'est pas bissextile"
        );
        assert_eq!(
            horodatage(4_107_542_400),
            "21000301-000000",
            "le 1er mars 2100 suit le 28 février"
        );
        assert_eq!(horodatage(1_709_164_800), "20240229-000000", "29 février");
        assert_eq!(
            horodatage(1_709_251_200),
            "20240301-000000",
            "lendemain du 29 février"
        );
    }

    /// Le répertoire suit `AVASH_HOME`, comme tout le reste du cœur, et
    /// n'est jamais vide.
    #[test]
    fn le_repertoire_suit_la_configuration() {
        let garde = crate::testutil::temp_home();
        let dir = repertoire().expect("un répertoire");
        assert!(dir.starts_with(garde.dir()), "{}", dir.display());
        assert!(dir.ends_with("avash/enregistrements"), "{}", dir.display());
    }

    /// Un enregistrement complet : en-tête v2, sortie, redimension, relecture.
    /// Et rien d'autre que la sortie : aucune frappe n'existe dans ce format.
    #[test]
    fn un_enregistrement_se_relit_avec_asciinema_en_tete() {
        let dir = bac("relecture");
        let mut e = Enregistreur::demarrer_dans(&dir, "prod web", 120, 40).unwrap();
        e.sortie("$ ls\r\n").unwrap();
        e.sortie("").unwrap();
        e.redimension(100, 30).unwrap();
        e.sortie("rapport.md\r\n").unwrap();
        assert_eq!(
            e.octets(),
            "$ ls\r\n".len() as u64 + "rapport.md\r\n".len() as u64
        );
        let chemin = e.arreter().unwrap();
        assert!(chemin.starts_with(&dir));
        assert!(chemin
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("prod-web-"));
        let contenu = std::fs::read_to_string(&chemin).unwrap();
        let (entete, ev) = relire(&contenu).expect("asciicast lisible");
        assert_eq!(entete["version"], 2);
        assert_eq!(entete["width"], 120);
        assert_eq!(entete["height"], 40);
        assert_eq!(entete["title"], "prod web");
        assert!(entete["timestamp"].as_u64().unwrap() > 1_700_000_000);
        let kinds: Vec<&str> = ev.iter().map(|(_, k, _)| k.as_str()).collect();
        assert_eq!(kinds, vec!["o", "r", "o"], "{ev:?}");
        assert_eq!(ev[1].2, "100x30");
        assert!(
            ev.windows(2).all(|w| w[0].0 <= w[1].0),
            "les temps croissent"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&chemin).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Arrêter puis redémarrer dans la même seconde donnait deux fois le même
    /// nom, et le second démarrage échouait « fichier existant ». Trouvé par le
    /// scénario bout en bout, qui enchaîne exactement cela.
    #[test]
    fn deux_enregistrements_dans_la_meme_seconde_ne_se_marchent_pas_dessus() {
        let dir = bac("collision");
        let a = Enregistreur::demarrer_dans(&dir, "x", 80, 24).unwrap();
        let b = Enregistreur::demarrer_dans(&dir, "x", 80, 24).unwrap();
        let c = Enregistreur::demarrer_dans(&dir, "x", 80, 24).unwrap();
        assert_ne!(a.chemin(), b.chemin());
        assert_ne!(b.chemin(), c.chemin());
        let nb = b
            .chemin()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let nc = c
            .chemin()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        // Même seconde ou non, les suffixes sont ceux attendus dès qu'il y a collision.
        if a.chemin().file_name() != b.chemin().file_name() && nb.ends_with("-2.cast") {
            assert!(nc.ends_with("-3.cast"), "{nc}");
        }
        assert_eq!(lister(&dir).len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn la_liste_ne_retient_que_les_cast_du_plus_recent_au_plus_ancien() {
        let dir = bac("liste");
        assert!(lister(&dir).is_empty(), "répertoire absent : liste vide");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ancien.cast"), "{}\n").unwrap();
        std::fs::write(dir.join("notes.txt"), "pas un enregistrement").unwrap();
        let vieux = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::open(dir.join("ancien.cast"))
            .unwrap()
            .set_modified(vieux)
            .unwrap();
        std::fs::write(dir.join("recent.cast"), "{}\n[0.1,\"o\",\"x\"]\n").unwrap();
        let l = lister(&dir);
        let noms: Vec<&str> = l.iter().map(|i| i.nom.as_str()).collect();
        assert_eq!(noms, vec!["recent.cast", "ancien.cast"]);
        assert!(l[0].octets > l[1].octets);
        assert!(l[0].modifie > l[1].modifie);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn une_derniere_ligne_tronquee_ne_perd_pas_le_reste() {
        let contenu =
            "{\"version\":2,\"width\":80,\"height\":24}\n[0.1,\"o\",\"a\"]\n[0.2,\"o\",\"b";
        let (_, ev) = relire(contenu).unwrap();
        assert_eq!(ev.len(), 1);
        assert!(relire("pas du json").is_none());
    }
}
