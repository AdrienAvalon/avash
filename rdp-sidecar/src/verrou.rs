//! Prendre un verrou même empoisonné.
//!
//! Recopié du cœur (audit du 12 septembre 2026, C-panique-4) : le processus RDP
//! est hors de l'espace de travail et ne dépend pas d'`avash`. Un
//! `.lock().unwrap()` panique si un autre fil a paniqué sous le verrou ; la file
//! du canal graphique, prise à chaque PDU et à chaque tic de la boucle,
//! condamnait alors toute la session pour une panique locale, que le
//! `catch_unwind` des décodeurs venait justement de contenir. Aucune section
//! critique de ce processus ne laisse ses données à moitié modifiées : on
//! reprend le verrou tel quel.

/// Un verrou qu'on prend même empoisonné.
pub trait Verrou<T: ?Sized> {
    /// Le garde du verrou, empoisonné ou non.
    fn verrou(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T: ?Sized> Verrou<T> for std::sync::Mutex<T> {
    fn verrou(&self) -> std::sync::MutexGuard<'_, T> {
        self.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::Verrou as _;

    /// Un fil qui panique sous le verrou l'empoisonne : `lock().unwrap()`
    /// aurait paniqué à son tour chez tous les suivants.
    #[test]
    fn un_verrou_empoisonne_se_reprend_avec_ses_donnees() {
        let m = std::sync::Arc::new(std::sync::Mutex::new(vec![1u8]));
        let m2 = std::sync::Arc::clone(&m);
        let _ = std::thread::spawn(move || {
            let mut g = m2.lock().unwrap();
            g.push(2);
            panic!("panique volontaire sous le verrou");
        })
        .join();
        assert!(m.lock().is_err(), "le verrou doit être empoisonné");
        assert_eq!(*m.verrou(), vec![1, 2]);
        m.verrou().push(3);
        assert_eq!(m.verrou().len(), 3);
    }
}
