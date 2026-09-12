//! Journal de l'application : `~/.config/avash/journal/avash.log`.
//!
//! Trouvé par l'audit du 12 septembre 2026 (C-SIL-2) : l'application publiée
//! n'écrivait nulle part. Un `emit` refusé, une sonde qui échoue, un trousseau
//! en panne, une tâche qui panique : tout ce qui n'avait pas de chemin vers
//! l'interface disparaissait, et un rapport « l'onglet est resté figé » était
//! invérifiable. Le journal comble ce trou, avec trois règles :
//!
//! - borné : deux fichiers d'un mégaoctet au plus (`avash.log`, puis
//!   `avash.log.1` pour le précédent), jamais davantage ;
//! - privé : fichiers en 0600, dans le répertoire d'Avash ;
//! - sans secret : on n'y écrit que des faits (erreurs, identifiants d'onglet),
//!   jamais une valeur saisie ; le `Debug` de `Target` masque déjà le mot de
//!   passe.
//!
//! Niveau `warn` par défaut, `info` si `AVASH_JOURNAL=info`.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Taille au-delà de laquelle `avash.log` devient `avash.log.1`.
pub(crate) const TAILLE_MAX: u64 = 1024 * 1024;

/// Nom du fichier courant ; le précédent porte le suffixe `.1`.
const NOM: &str = "avash.log";

/// Le répertoire du journal, sous le répertoire de configuration d'Avash
/// (qui suit `AVASH_HOME`, comme les tests et la suite bout en bout l'exigent).
#[must_use]
pub fn repertoire() -> Option<PathBuf> {
    avash::repertoire_configuration().map(|d| d.join("avash").join("journal"))
}

/// Le niveau retenu d'après `AVASH_JOURNAL`. Pur, pour être testé.
#[must_use]
pub(crate) fn niveau(variable: Option<&str>) -> tracing::level_filters::LevelFilter {
    match variable.map(str::trim) {
        Some(v) if v.eq_ignore_ascii_case("info") => tracing::level_filters::LevelFilter::INFO,
        _ => tracing::level_filters::LevelFilter::WARN,
    }
}

/// Écrivain à rotation par taille. Chaque événement arrive d'un seul
/// `write` (le formateur de `tracing-subscriber` compose la ligne avant de
/// l'écrire) : la décision de tourner se prend donc entre deux lignes.
pub(crate) struct Tournant {
    dir: PathBuf,
    max: u64,
    courant: Mutex<Option<(std::fs::File, u64)>>,
}

impl Tournant {
    pub(crate) fn nouveau(dir: PathBuf, max: u64) -> Self {
        Self {
            dir,
            max,
            courant: Mutex::new(None),
        }
    }

    fn ouvrir(&self) -> std::io::Result<(std::fs::File, u64)> {
        std::fs::create_dir_all(&self.dir)?;
        let chemin = self.dir.join(NOM);
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let f = options.open(&chemin)?;
        // Un fichier créé par une version antérieure, ou par un umask large,
        // est resserré ; le répertoire aussi, puisqu'il est à Avash.
        avash::restreindre_au_proprietaire(&chemin);
        let taille = f.metadata()?.len();
        Ok((f, taille))
    }

    fn ecrire(&self, ligne: &[u8]) -> std::io::Result<()> {
        use avash::Verrou as _;
        // Une ligne démesurée ne fait pas sauter la borne : elle est coupée.
        let borne = usize::try_from(self.max).unwrap_or(usize::MAX);
        let ligne = &ligne[..ligne.len().min(borne)];
        let mut courant = self.courant.verrou();
        if courant.is_none() {
            *courant = Some(self.ouvrir()?);
        }
        let deborde = courant
            .as_ref()
            .is_some_and(|(_, taille)| *taille > 0 && *taille + ligne.len() as u64 > self.max);
        if deborde {
            *courant = None;
            std::fs::rename(self.dir.join(NOM), self.dir.join(format!("{NOM}.1")))?;
            *courant = Some(self.ouvrir()?);
        }
        if let Some((f, taille)) = courant.as_mut() {
            f.write_all(ligne)?;
            *taille += ligne.len() as u64;
        }
        Ok(())
    }
}

/// Ce que `tracing-subscriber` demande pour chaque événement.
pub(crate) struct Ecrivain(Arc<Tournant>);

impl std::io::Write for Ecrivain {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.ecrire(buf)?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// La fabrique d'écrivains passée au formateur (un type local : `MakeWriter`
/// ne s'implémente pas directement pour un `Arc` étranger).
pub(crate) struct Fabrique(Arc<Tournant>);

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Fabrique {
    type Writer = Ecrivain;
    fn make_writer(&'a self) -> Self::Writer {
        Ecrivain(self.0.clone())
    }
}

/// L'abonné qui écrit dans `dir`, au niveau donné. Séparé de `installer` pour
/// que les tests l'utilisent localement (`tracing::subscriber::with_default`)
/// sans poser d'abonné global.
pub(crate) fn abonne(
    dir: PathBuf,
    max: u64,
    niveau: tracing::level_filters::LevelFilter,
) -> impl tracing::Subscriber + Send + Sync {
    tracing_subscriber::fmt()
        .with_writer(Fabrique(Arc::new(Tournant::nouveau(dir, max))))
        .with_max_level(niveau)
        .with_target(false)
        .finish()
}

/// Pose le journal pour tout le processus, et un crochet de panique qui y
/// écrit avant de laisser le crochet par défaut faire son travail (stderr).
/// Sans répertoire de configuration, rien n'est posé : l'application démarre
/// quand même.
pub fn installer() {
    let Some(dir) = repertoire() else {
        return;
    };
    let niveau = niveau(std::env::var("AVASH_JOURNAL").ok().as_deref());
    if tracing::subscriber::set_global_default(abonne(dir, TAILLE_MAX, niveau)).is_err() {
        return;
    }
    // Trouvé par l'audit du 12 septembre 2026 : tokio avale la panique d'une
    // tâche détachée (relais d'un onglet, tunnel, sidecar suivi) ; sans ce
    // crochet elle ne laissait aucune trace dans une application sans console.
    let precedent = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("panique : {info}");
        precedent(info);
    }));
}

/// Les dernières lignes du journal (fichier précédent compris), au plus `n`,
/// pour le diagnostic exporté. Un journal absent ou illisible le dit.
#[must_use]
pub(crate) fn dernieres_lignes(dir: &Path, n: usize) -> String {
    let mut lignes: Vec<String> = Vec::new();
    for nom in [format!("{NOM}.1"), NOM.to_owned()] {
        if let Ok(texte) = std::fs::read(dir.join(nom)) {
            lignes.extend(String::from_utf8_lossy(&texte).lines().map(str::to_owned));
        }
    }
    if lignes.is_empty() {
        return "(journal vide)".to_owned();
    }
    let debut = lignes.len().saturating_sub(n);
    lignes[debut..].join("\n")
}

#[cfg(test)]
mod tests_journal {
    use super::{abonne, dernieres_lignes, niveau, TAILLE_MAX};
    use crate::commands::tests::with_ssh_config;

    /// Le niveau par défaut n'écrit que les avertissements ; `info` s'active
    /// par la variable, sans tenir compte de la casse.
    #[test]
    fn le_niveau_vaut_warn_sauf_avash_journal_info() {
        use tracing::level_filters::LevelFilter;
        assert_eq!(niveau(None), LevelFilter::WARN);
        assert_eq!(niveau(Some("debug")), LevelFilter::WARN);
        assert_eq!(niveau(Some("INFO")), LevelFilter::INFO);
        assert_eq!(niveau(Some(" info ")), LevelFilter::INFO);
    }

    /// Audit du 12 septembre 2026 (C-SIL-2) : trois mégaoctets
    /// d'avertissements ne dépassent jamais deux fichiers d'un mégaoctet, les
    /// fichiers sont privés, et une cible avec mot de passe, tracée par son
    /// `Debug`, n'y laisse pas le secret.
    #[test]
    fn le_journal_est_borne_et_sans_secret() {
        let _g = with_ssh_config("");
        let dir = super::repertoire().unwrap();
        let cible = crate::commands::Target::manual(
            "10.0.0.1".into(),
            None,
            "deploy".into(),
            Some("tres-secret".into()),
            None,
        )
        .unwrap();
        let ligne = "x".repeat(200);
        tracing::subscriber::with_default(abonne(dir.clone(), TAILLE_MAX, niveau(None)), || {
            for i in 0..15_000 {
                tracing::warn!("avertissement {i} {ligne}");
            }
            tracing::warn!("cible {cible:?}");
            tracing::info!("sous le niveau, jamais écrit");
        });
        let mut total = 0;
        let mut noms = Vec::new();
        for e in std::fs::read_dir(&dir).unwrap() {
            let e = e.unwrap();
            let meta = e.metadata().unwrap();
            assert!(meta.len() <= TAILLE_MAX, "{:?} : {}", e.path(), meta.len());
            total += meta.len();
            noms.push(e.file_name().to_string_lossy().into_owned());
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                assert_eq!(meta.permissions().mode() & 0o777, 0o600, "{noms:?}");
            }
            let texte = std::fs::read_to_string(e.path()).unwrap();
            assert!(!texte.contains("tres-secret"), "secret écrit dans {noms:?}");
            assert!(!texte.contains("jamais écrit"));
        }
        noms.sort();
        assert_eq!(noms, ["avash.log", "avash.log.1"]);
        assert!(total <= 2 * TAILLE_MAX, "{total}");
        assert!(dernieres_lignes(&dir, 5).contains("masqué"));
    }

    /// Sans journal, le diagnostic le dit au lieu d'une section vide.
    #[test]
    fn un_journal_absent_se_dit() {
        let dir = std::env::temp_dir().join("avash-journal-absent-n-existe-pas");
        assert_eq!(dernieres_lignes(&dir, 10), "(journal vide)");
    }
}
