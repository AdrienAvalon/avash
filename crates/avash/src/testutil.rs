//! Outils partagés par les tests du crate.

/// Isole `HOME` le temps d'un test, et le restaure ensuite.
///
/// ⚠️ `HOME` est global au processus : deux tests qui le modifient en
/// parallèle se marchent dessus. Le verrou ci-dessous doit donc être
/// **unique pour tout le crate** — un verrou par module ne protège de rien,
/// puisque les modules s'exécutent en parallèle les uns des autres.
static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub struct HomeGuard {
    previous: Option<String>,
    dir: std::path::PathBuf,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Bascule `HOME` sur un répertoire vierge, propre à ce test.
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
    std::env::set_var("HOME", &dir);
    HomeGuard {
        previous,
        dir,
        _lock: lock,
    }
}
