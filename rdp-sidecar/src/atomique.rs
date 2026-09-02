//! Écriture atomique d'un petit fichier d'état, en 0600.
//!
//! Le processus RDP ne dépend pas du crate `avash`, qui a déjà cette fonction :
//! on la refait ici, plus courte, pour les deux fichiers qu'il écrit — les
//! empreintes de serveurs et la liste des serveurs à canal graphique. Perdre le
//! premier ramène tous les serveurs à « premier contact » sans que rien ne le
//! signale ; une lecture-modification-écriture non atomique perdait aussi
//! l'entrée d'un premier contact concurrent.

use std::path::Path;

/// Écrit `contenu` dans `chemin` par un temporaire voisin renommé : le fichier
/// est soit l'ancien, soit le nouveau, jamais tronqué. Le temporaire naît en
/// 0600 sous Unix, et le répertoire parent est créé au besoin.
pub fn ecrire(chemin: &Path, contenu: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    if let Some(dir) = chemin.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
            }
        }
    }
    // Unique par processus ET par appel : deux écritures rapprochées du même
    // fichier ne doivent pas se partager un temporaire.
    static SUITE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = chemin.with_extension(format!(
        "tmp{}.{}",
        std::process::id(),
        SUITE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let ecrit = (|| {
        let mut f = options.open(&tmp)?;
        f.write_all(contenu)?;
        f.sync_all()
    })();
    if let Err(e) = ecrit {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, chemin) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(chemin, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ecrire;

    fn bac(nom: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("avash-atomique-{}-{nom}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn le_fichier_est_complet_prive_et_sans_residu() {
        let d = bac("complet");
        let cible = d.join("sous").join("etat");
        ecrire(&cible, b"premier\n").unwrap();
        ecrire(&cible, b"second\n").unwrap();
        assert_eq!(std::fs::read(&cible).unwrap(), b"second\n");
        let restants: Vec<_> = std::fs::read_dir(cible.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(restants, vec!["etat".to_owned()], "résidu : {restants:?}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&cible).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "droits : {mode:o}");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn un_chemin_impossible_rend_une_erreur_sans_temporaire() {
        let d = bac("impossible");
        std::fs::create_dir_all(&d).unwrap();
        let obstacle = d.join("obstacle");
        std::fs::write(&obstacle, b"fichier").unwrap();
        // « obstacle » est un fichier : impossible d'en faire un répertoire.
        assert!(ecrire(&obstacle.join("dedans"), b"x").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
