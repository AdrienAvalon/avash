//! Outils partagés par les tests du crate.

/// Isole le répertoire personnel le temps d'un test, et le restaure ensuite.
///
/// `HOME` **ne suffit pas** : sous Windows, `dirs::home_dir()` interroge le
/// dossier de profil du système et ignore cette variable. Les tests y
/// travaillaient donc sur le vrai profil de la machine, tous en parallèle sur
/// les mêmes fichiers — aucune isolation. On pose aussi `AVASH_HOME`, que
/// `repertoire_personnel()` honore sur toutes les plateformes.
///
/// ⚠️ L'environnement est global au processus : deux tests qui le modifient en
/// parallèle se marchent dessus. Le verrou ci-dessous doit donc être
/// **unique pour tout le crate** — un verrou par module ne protège de rien,
/// puisque les modules s'exécutent en parallèle les uns des autres.
static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub struct HomeGuard {
    previous: Option<String>,
    previous_avash: Option<String>,
    dir: std::path::PathBuf,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl HomeGuard {
    /// Le répertoire qui tient lieu de `HOME` pendant le test.
    #[must_use]
    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        match &self.previous_avash {
            Some(h) => std::env::set_var("AVASH_HOME", h),
            None => std::env::remove_var("AVASH_HOME"),
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Bascule le répertoire personnel sur un répertoire vierge, propre à ce test.
pub fn temp_home() -> HomeGuard {
    // `unwrap_or_else(into_inner)` : un test qui panique empoisonne le
    // verrou ; sans cela tous les tests suivants echoueraient en cascade.
    let lock = HOME_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = std::env::temp_dir().join(format!(
        "avash-test-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let previous = std::env::var("HOME").ok();
    let previous_avash = std::env::var("AVASH_HOME").ok();
    std::env::set_var("HOME", &dir);
    std::env::set_var("AVASH_HOME", &dir);
    HomeGuard {
        previous,
        previous_avash,
        dir,
        _lock: lock,
    }
}
