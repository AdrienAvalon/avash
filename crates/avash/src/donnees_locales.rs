//! Les données d'Avash qui ne quittent pas le poste : journal des commandes,
//! enregistrements des sessions, journal de l'application.
//!
//! Audit du 27 novembre 2026 (SE-5 = WI-14), choix du mainteneur : « tout en
//! local ». Sous Windows, tout vivait sous `%APPDATA%\avash`, c'est-à-dire
//! `AppData\Roaming`. Dans un domaine à profils itinérants ou à redirection de
//! dossiers, ce répertoire est recopié sur un serveur de fichiers à chaque
//! fermeture de session : le journal des commandes (une donnée sensible, voir
//! `SECURITY.md`) et les enregistrements (tout ce qui s'est affiché dans le
//! terminal, secrets montrés compris) quittaient la machine, et chaque
//! écriture traversait le réseau quand le dossier est redirigé. Ces données
//! vont désormais sous `%LOCALAPPDATA%\avash` (`AppData\Local`, jamais
//! itinérant) ; la configuration (hôtes, dossiers, bureaux, empreintes RDP,
//! snippets) reste dans `%APPDATA%`, qu'elle suive l'utilisateur ou non.
//! Ailleurs que sous Windows, rien ne change : `~/.config` est local.
//!
//! Les fichiers existants sont déplacés une fois, au lancement, par
//! [`migrer_vers_le_local`] : sans perte (un fichier déjà présent à
//! l'arrivée n'est jamais écrasé) et sans danger à rejouer (ce qui est parti
//! ne revient pas, une migration interrompue reprend où elle s'était
//! arrêtée).

use std::path::{Path, PathBuf};

/// Les sous-répertoires de `avash` qui sont des données du poste et non de la
/// configuration.
pub const SOUS_REPERTOIRES: [&str; 3] = ["journal-commandes", "enregistrements", "journal"];

/// Le répertoire de base des données locales : celui qui contient `avash`.
///
/// Sous Windows `%LOCALAPPDATA%` ; ailleurs, ou quand `AVASH_HOME` isole un
/// test ou la suite bout en bout, le même que la configuration.
#[must_use]
pub fn repertoire_de_base() -> Option<PathBuf> {
    choisir(
        cfg!(windows),
        std::env::var_os("AVASH_HOME").is_some(),
        crate::repertoire_configuration(),
        dirs::data_local_dir(),
    )
}

/// La règle de [`repertoire_de_base`], pure pour être testée partout.
fn choisir(
    windows: bool,
    isole: bool,
    configuration: Option<PathBuf>,
    local: Option<PathBuf>,
) -> Option<PathBuf> {
    if windows && !isole {
        local
    } else {
        configuration
    }
}

/// Ce que la migration a fait, pour le journal de l'application.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Bilan {
    /// Fichiers déplacés vers le répertoire local.
    pub deplaces: usize,
    /// Fichiers déjà présents à l'arrivée sous le même nom et au même
    /// contenu : la copie itinérante, redondante, est retirée.
    pub doublons: usize,
    /// Fichiers déjà présents à l'arrivée avec un autre contenu : déplacés
    /// sous un nom suffixé (`.itinerant-1`), rien n'est écrasé.
    pub renommes: usize,
}

/// Déplace les données locales de `%APPDATA%\avash` vers
/// `%LOCALAPPDATA%\avash`, une fois. Ne fait rien hors Windows, ni quand les
/// deux répertoires coïncident (`AVASH_HOME`).
///
/// # Errors
/// Au premier fichier qui ne peut être ni déplacé ni copié ; ce qui a été
/// déplacé avant reste à sa nouvelle place, le reste à l'ancienne, et le
/// lancement suivant reprend.
pub fn migrer_vers_le_local() -> anyhow::Result<Bilan> {
    let (Some(ancien), Some(nouveau)) = (crate::repertoire_configuration(), repertoire_de_base())
    else {
        return Ok(Bilan::default());
    };
    migrer_entre(&ancien.join("avash"), &nouveau.join("avash"))
}

/// [`migrer_vers_le_local`] entre deux répertoires `avash` donnés, pour les
/// tests.
///
/// # Errors
/// Voir [`migrer_vers_le_local`].
pub fn migrer_entre(ancien: &Path, nouveau: &Path) -> anyhow::Result<Bilan> {
    let mut bilan = Bilan::default();
    if ancien == nouveau {
        return Ok(bilan);
    }
    for sous in SOUS_REPERTOIRES {
        let source = ancien.join(sous);
        if source.is_dir() {
            deplacer_repertoire(&source, &nouveau.join(sous), &mut bilan)?;
        }
    }
    Ok(bilan)
}

/// Déplace le contenu de `source` dans `cible`, fichier par fichier, puis
/// retire `source` s'il est vide.
fn deplacer_repertoire(source: &Path, cible: &Path, bilan: &mut Bilan) -> anyhow::Result<()> {
    use anyhow::Context as _;
    std::fs::create_dir_all(cible).with_context(|| format!("création de {}", cible.display()))?;
    for entree in std::fs::read_dir(source)? {
        let entree = entree?;
        let chemin = entree.path();
        let arrivee = cible.join(entree.file_name());
        // `file_type` ne suit pas les liens : un lien n'est pas suivi hors du
        // répertoire, il est déplacé comme un fichier.
        if entree.file_type()?.is_dir() {
            deplacer_repertoire(&chemin, &arrivee, bilan)?;
            continue;
        }
        if !arrivee.exists() {
            deplacer_fichier(&chemin, &arrivee)?;
            bilan.deplaces += 1;
        } else if memes_octets(&chemin, &arrivee)? {
            // Une migration précédente a copié ce fichier sans pouvoir retirer
            // l'original : la copie locale est complète, l'original part.
            std::fs::remove_file(&chemin)?;
            bilan.doublons += 1;
        } else {
            deplacer_fichier(&chemin, &libre_a_cote(&arrivee))?;
            bilan.renommes += 1;
        }
    }
    // Vide, il s'en va ; sinon (fichier ajouté entre-temps) il attend le
    // lancement suivant.
    let _ = std::fs::remove_dir(source);
    Ok(())
}

/// Renomme, ou copie puis retire quand les deux répertoires ne sont pas sur le
/// même volume (`%APPDATA%` redirigé vers un partage réseau). La copie est
/// synchronisée avant que l'original ne parte : une coupure laisse deux
/// exemplaires, jamais aucun.
fn deplacer_fichier(depuis: &Path, vers: &Path) -> anyhow::Result<()> {
    use anyhow::Context as _;
    if std::fs::rename(depuis, vers).is_ok() {
        return Ok(());
    }
    let temporaire = vers.with_extension("migration");
    std::fs::copy(depuis, &temporaire).with_context(|| format!("copie de {}", depuis.display()))?;
    std::fs::File::open(&temporaire)?.sync_all()?;
    std::fs::rename(&temporaire, vers)?;
    std::fs::remove_file(depuis).with_context(|| format!("retrait de {}", depuis.display()))?;
    Ok(())
}

fn memes_octets(a: &Path, b: &Path) -> std::io::Result<bool> {
    let (ma, mb) = (std::fs::metadata(a)?, std::fs::metadata(b)?);
    if ma.len() != mb.len() {
        return Ok(false);
    }
    Ok(std::fs::read(a)? == std::fs::read(b)?)
}

/// Un nom libre à côté de `chemin` : `2026-11-24.itinerant-1.jsonl`, puis
/// `-2`, etc. L'extension est gardée : l'enregistrement reste lisible par un
/// lecteur asciicast, le journal par un éditeur.
fn libre_a_cote(chemin: &Path) -> PathBuf {
    let tige = chemin
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let ext = chemin
        .extension()
        .map_or_else(String::new, |e| format!(".{}", e.to_string_lossy()));
    // Borné, pour qu'aucune boucle ne tourne sans fin ; au-delà de dix mille
    // copies d'un même nom, l'instant présent départage, sans rien écraser.
    (1..=10_000)
        .map(|n| chemin.with_file_name(format!("{tige}.itinerant-{n}{ext}")))
        .find(|p| !p.exists())
        .unwrap_or_else(|| {
            let instant = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            chemin.with_file_name(format!("{tige}.itinerant-{instant}{ext}"))
        })
}

#[cfg(test)]
mod tests {
    use super::{choisir, migrer_entre, Bilan};
    use std::path::{Path, PathBuf};

    /// Un répertoire temporaire propre au test, retiré à la fin.
    struct Bac(PathBuf);
    impl Bac {
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Bac {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn bac() -> Bac {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "avash-donnees-locales-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Bac(dir)
    }

    fn ecrire(chemin: &Path, contenu: &str) {
        std::fs::create_dir_all(chemin.parent().unwrap()).unwrap();
        std::fs::write(chemin, contenu).unwrap();
    }

    /// Audit du 27 novembre 2026 (SE-5 = WI-14) : sous Windows, le journal des
    /// commandes et les enregistrements suivaient le profil itinérant
    /// (`AppData\Roaming`). Ils vont sous `AppData\Local` ; ailleurs, et sous
    /// `AVASH_HOME` (tests, suite bout en bout), rien ne change.
    #[test]
    fn sous_windows_les_donnees_vont_dans_le_profil_local() {
        let config = Some(PathBuf::from("C:/Users/ada/AppData/Roaming"));
        let local = Some(PathBuf::from("C:/Users/ada/AppData/Local"));
        assert_eq!(choisir(true, false, config.clone(), local.clone()), local);
        assert_eq!(choisir(true, true, config.clone(), local.clone()), config);
        assert_eq!(choisir(false, false, config.clone(), local), config);
    }

    /// Les données existantes sont déplacées une fois, sans perte, et la
    /// configuration reste où elle est.
    #[test]
    fn les_donnees_existantes_sont_deplacees_sans_perte() {
        let bac = bac();
        let ancien = bac.path().join("Roaming/avash");
        let nouveau = bac.path().join("Local/avash");
        ecrire(
            &ancien.join("journal-commandes/2026-11-24.jsonl"),
            "{\"commande\":\"ls\"}\n",
        );
        ecrire(
            &ancien.join("enregistrements/web-1.cast"),
            "{\"version\":2}\n",
        );
        ecrire(&ancien.join("journal/avash.log"), "démarrage\n");
        ecrire(&ancien.join("hosts-folders.yaml"), "dossiers: []\n");

        let bilan = migrer_entre(&ancien, &nouveau).unwrap();

        assert_eq!(bilan.deplaces, 3);
        assert_eq!(
            std::fs::read_to_string(nouveau.join("journal-commandes/2026-11-24.jsonl")).unwrap(),
            "{\"commande\":\"ls\"}\n"
        );
        assert!(nouveau.join("enregistrements/web-1.cast").is_file());
        assert!(nouveau.join("journal/avash.log").is_file());
        assert!(!ancien.join("journal-commandes").exists());
        assert!(!ancien.join("enregistrements").exists());
        // La configuration ne bouge pas.
        assert!(ancien.join("hosts-folders.yaml").is_file());
        assert!(!nouveau.join("hosts-folders.yaml").exists());
    }

    /// Rejouée, la migration ne fait rien : elle ne s'applique qu'une fois.
    #[test]
    fn la_migration_rejouee_ne_fait_rien() {
        let bac = bac();
        let ancien = bac.path().join("Roaming/avash");
        let nouveau = bac.path().join("Local/avash");
        ecrire(&ancien.join("enregistrements/a.cast"), "a");
        migrer_entre(&ancien, &nouveau).unwrap();
        assert_eq!(migrer_entre(&ancien, &nouveau).unwrap(), Bilan::default());
        assert_eq!(
            std::fs::read_to_string(nouveau.join("enregistrements/a.cast")).unwrap(),
            "a"
        );
    }

    /// Un fichier déjà présent à l'arrivée n'est jamais écrasé : même contenu,
    /// la copie itinérante part ; autre contenu, elle arrive sous un nom à
    /// part.
    #[test]
    fn un_fichier_deja_present_a_l_arrivee_n_est_jamais_ecrase() {
        let bac = bac();
        let ancien = bac.path().join("Roaming/avash");
        let nouveau = bac.path().join("Local/avash");
        ecrire(
            &ancien.join("journal-commandes/2026-11-24.jsonl"),
            "itinérant\n",
        );
        ecrire(
            &nouveau.join("journal-commandes/2026-11-24.jsonl"),
            "local\n",
        );
        ecrire(&ancien.join("enregistrements/a.cast"), "même");
        ecrire(&nouveau.join("enregistrements/a.cast"), "même");

        let bilan = migrer_entre(&ancien, &nouveau).unwrap();

        assert_eq!(
            bilan,
            Bilan {
                deplaces: 0,
                doublons: 1,
                renommes: 1
            }
        );
        assert_eq!(
            std::fs::read_to_string(nouveau.join("journal-commandes/2026-11-24.jsonl")).unwrap(),
            "local\n"
        );
        assert_eq!(
            std::fs::read_to_string(nouveau.join("journal-commandes/2026-11-24.itinerant-1.jsonl"))
                .unwrap(),
            "itinérant\n"
        );
        assert_eq!(
            std::fs::read_to_string(nouveau.join("enregistrements/a.cast")).unwrap(),
            "même"
        );
        assert!(!ancien.join("enregistrements").exists());
    }

    /// Les deux répertoires confondus (`AVASH_HOME`, ou hors Windows) : rien.
    #[test]
    fn sans_second_repertoire_rien_ne_bouge() {
        let bac = bac();
        let avash = bac.path().join("avash");
        ecrire(&avash.join("enregistrements/a.cast"), "a");
        assert_eq!(migrer_entre(&avash, &avash).unwrap(), Bilan::default());
        assert!(avash.join("enregistrements/a.cast").is_file());
    }
}
