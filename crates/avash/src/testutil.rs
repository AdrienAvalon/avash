//! Outils partagés par les tests du crate, et par ceux d'`avash-ui` à travers
//! la fonctionnalité `outils-de-test` (serveur SSH+SFTP en mémoire). Rien ici
//! n'entre dans un binaire publié : le module n'existe que sous `cfg(test)` ou
//! sous cette fonctionnalité, que seules les `[dev-dependencies]` posent.

// Code de test : un décor qui échoue (répertoire temporaire impossible à
// créer) doit faire échouer le test sur place, d'où les `unwrap`, que
// `allow-unwrap-in-tests` ne couvre pas dans ce module compilé sous la
// fonctionnalité `outils-de-test` (hors `cfg(test)`).
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[cfg(feature = "outils-de-test")]
pub mod serveur_ssh;

/// Verrou de l'environnement du processus, **unique pour tout le crate**.
///
/// ⚠️ L'environnement est global au processus : deux tests qui le modifient en
/// parallèle se marchent dessus. Un verrou par module ne protège de rien,
/// puisque les modules s'exécutent en parallèle les uns des autres. Il n'est
/// pas réentrant : un seul garde vivant à la fois par test.
static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Preuve que l'on tient le verrou de l'environnement. [`poser_variable`] la
/// réclame : un appel hors verrou ne compile pas.
pub struct VerrouEnvironnement(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);

/// Prend le verrou de l'environnement (tolérant à l'empoisonnement : un test
/// qui panique ne doit pas faire échouer tous les suivants en cascade).
#[must_use]
pub fn verrou_environnement() -> VerrouEnvironnement {
    VerrouEnvironnement(
        HOME_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
}

/// Pose (`Some`) ou retire (`None`) une variable d'environnement, sous le
/// verrou de l'environnement.
///
/// Le seul endroit des tests du crate qui écrive l'environnement. Audit du
/// 12 septembre 2026 (C-unsafe-6) : les gardes appelaient `set_var` et
/// `remove_var` nus, légaux en édition 2021 mais `unsafe` en édition 2024 ; la
/// moitié du dépôt les enveloppait déjà. Un seul bloc, une seule justification.
pub fn poser_variable(_verrou: &VerrouEnvironnement, nom: &str, valeur: Option<&std::ffi::OsStr>) {
    match valeur {
        // SAFETY: sous HOME_LOCK (la preuve `_verrou` est exigée par la
        // signature), qui sérialise tous les écrivains de l'environnement de
        // ce binaire de test. Les lecteurs y passent par std::env, qui tient
        // son propre verrou ; aucun code C de ces binaires (ring, russh,
        // serialport sans libudev) ne lit l'environnement par getenv.
        Some(v) => unsafe { std::env::set_var(nom, v) },
        // SAFETY: même invariant que la branche ci-dessus.
        None => unsafe { std::env::remove_var(nom) },
    }
}

/// Isole le répertoire personnel le temps d'un test, et le restaure ensuite.
///
/// `HOME` **ne suffit pas** : sous Windows, `dirs::home_dir()` interroge le
/// dossier de profil du système et ignore cette variable. Les tests y
/// travaillaient donc sur le vrai profil de la machine, tous en parallèle sur
/// les mêmes fichiers — aucune isolation. On pose aussi `AVASH_HOME`, que
/// `repertoire_personnel()` honore sur toutes les plateformes.
pub struct HomeGuard {
    previous: Option<std::ffi::OsString>,
    previous_avash: Option<std::ffi::OsString>,
    previous_trousseau: Option<std::ffi::OsString>,
    dir: std::path::PathBuf,
    /// Variables posées par [`HomeGuard::poser`], avec leur valeur d'avant,
    /// restaurées à la chute du garde.
    poses: std::cell::RefCell<Vec<(String, Option<std::ffi::OsString>)>>,
    // Dernier champ : détruit après la restauration faite dans `drop`, donc
    // la restauration a lieu sous le verrou.
    verrou: VerrouEnvironnement,
}

impl HomeGuard {
    /// Le répertoire qui tient lieu de `HOME` pendant le test.
    #[must_use]
    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    /// Pose une variable sous le verrou que ce garde tient déjà (il n'est pas
    /// réentrant) ; sa valeur d'avant revient à la chute du garde.
    pub fn poser(&self, nom: &str, valeur: Option<&str>) {
        let mut poses = self.poses.borrow_mut();
        if !poses.iter().any(|(n, _)| n == nom) {
            poses.push((nom.to_owned(), std::env::var_os(nom)));
        }
        poser_variable(&self.verrou, nom, valeur.map(std::ffi::OsStr::new));
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        for (nom, avant) in self.poses.borrow_mut().drain(..).rev() {
            poser_variable(&self.verrou, &nom, avant.as_deref());
        }
        poser_variable(&self.verrou, "HOME", self.previous.as_deref());
        poser_variable(&self.verrou, "AVASH_HOME", self.previous_avash.as_deref());
        poser_variable(
            &self.verrou,
            "AVASH_TROUSSEAU",
            self.previous_trousseau.as_deref(),
        );
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Bascule le répertoire personnel sur un répertoire vierge, propre à ce test.
#[must_use]
pub fn temp_home() -> HomeGuard {
    let verrou = verrou_environnement();
    let dir = std::env::temp_dir().join(format!(
        "avash-test-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let previous = std::env::var_os("HOME");
    let previous_avash = std::env::var_os("AVASH_HOME");
    // Isole aussi le trousseau : sans cela `secrets::load`/`sonder` tapent dans
    // le vrai Secret Service du poste (keyring v1 sous Linux = D-Bus), ce que
    // l'isolation de `~/.ssh` ne couvre pas. Voir `avash::secrets::en_memoire`.
    let previous_trousseau = std::env::var_os("AVASH_TROUSSEAU");
    poser_variable(&verrou, "HOME", Some(dir.as_os_str()));
    poser_variable(&verrou, "AVASH_HOME", Some(dir.as_os_str()));
    poser_variable(
        &verrou,
        "AVASH_TROUSSEAU",
        Some(std::ffi::OsStr::new("memoire")),
    );
    HomeGuard {
        previous,
        previous_avash,
        previous_trousseau,
        dir,
        poses: std::cell::RefCell::new(Vec::new()),
        verrou,
    }
}

/// Les avertissements (`tracing::warn!`) émis sur ce fil pendant `f`, par un
/// abonné local qui ne voit que ce fil : les tests parallèles ne s'y mêlent
/// pas. Sert à vérifier qu'un échec toléré n'est plus avalé en silence
/// (audit du 12 septembre 2026, C-SIL-13).
pub fn avertissements_pendant<R>(f: impl FnOnce() -> R) -> (R, Vec<String>) {
    struct Capture(std::sync::Arc<std::sync::Mutex<Vec<String>>>);
    struct Message(String);
    impl tracing::field::Visit for Message {
        fn record_debug(&mut self, champ: &tracing::field::Field, valeur: &dyn std::fmt::Debug) {
            if champ.name() == "message" {
                self.0 = format!("{valeur:?}");
            }
        }
    }
    impl tracing::Subscriber for Capture {
        fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }
        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
        fn event(&self, evenement: &tracing::Event<'_>) {
            if *evenement.metadata().level() == tracing::Level::WARN {
                let mut m = Message(String::new());
                evenement.record(&mut m);
                self.0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(m.0);
            }
        }
        fn enter(&self, _: &tracing::span::Id) {}
        fn exit(&self, _: &tracing::span::Id) {}
    }
    let journal = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let r = tracing::subscriber::with_default(Capture(journal.clone()), f);
    let lignes = std::mem::take(
        &mut *journal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    (r, lignes)
}
