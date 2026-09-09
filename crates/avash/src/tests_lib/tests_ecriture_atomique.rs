use super::ecrire_atomiquement;
use crate::testutil::temp_home;

/// Le contenu doit être intégralement lisible, et le fichier ne doit jamais
/// avoir été lisible par un autre compte — le temporaire naissait avec
/// l'umask et n'était resserré qu'après le renommage.
#[test]
fn le_fichier_ecrit_est_complet_et_prive() {
    let home = temp_home();
    let cible = home.dir().join("secrets.yaml");
    ecrire_atomiquement(&cible, b"contenu complet\n").unwrap();
    assert_eq!(
        std::fs::read_to_string(&cible).unwrap(),
        "contenu complet\n"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&cible).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "lisible par d'autres comptes : {mode:o}");
    }
}

/// Réécrire remplace le contenu sans laisser d'intermédiaire : aucun
/// résidu `.tmp` ne doit subsister dans le répertoire.
#[test]
fn la_reecriture_ne_laisse_aucun_residu() {
    let home = temp_home();
    let cible = home.dir().join("liste.yaml");
    ecrire_atomiquement(&cible, b"premier").unwrap();
    ecrire_atomiquement(&cible, b"second").unwrap();
    assert_eq!(std::fs::read_to_string(&cible).unwrap(), "second");
    let restants: Vec<_> = std::fs::read_dir(home.dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        restants,
        vec!["liste.yaml".to_owned()],
        "résidu : {restants:?}"
    );
}

/// Le répertoire manquant est créé, et en 0700 : `~/.config/avash` naissait
/// lui aussi avec l'umask.
#[test]
fn le_repertoire_absent_est_cree_et_prive() {
    let home = temp_home();
    let cible = home.dir().join("neuf/sous/fichier.yaml");
    ecrire_atomiquement(&cible, b"x").unwrap();
    assert!(cible.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(cible.parent().unwrap())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "répertoire ouvert : {mode:o}");
    }
}

/// Un répertoire qui existait déjà garde ses droits : on le resserrait à
/// 0700 comme s'il venait d'être créé. Vu le 2026-09-03 quand la suite,
/// lancée en root, a passé /tmp en 0700 par les cas qui y écrivent
/// directement ; un compte ordinaire subissait la même chose sur ses
/// propres répertoires.
#[test]
#[cfg(unix)]
fn un_repertoire_existant_garde_ses_droits() {
    use std::os::unix::fs::PermissionsExt;
    let home = temp_home();
    let partage = home.dir().join("partage");
    std::fs::create_dir(&partage).unwrap();
    std::fs::set_permissions(&partage, std::fs::Permissions::from_mode(0o755)).unwrap();
    ecrire_atomiquement(&partage.join("export.yaml"), b"x").unwrap();
    let mode = std::fs::metadata(&partage).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755, "répertoire existant resserré : {mode:o}");
}

/// Les répertoires d'Avash, eux, sont resserrés même s'ils existaient
/// déjà : `~/.config/avash` et `~/.ssh` naissaient avec l'umask, souvent
/// lisibles par tous, et c'est ce que le correctif précédent ne doit pas
/// défaire.
#[test]
#[cfg(unix)]
fn les_repertoires_d_avash_sont_resserres_meme_existants() {
    use std::os::unix::fs::PermissionsExt;
    let home = temp_home();
    let ouverts = std::fs::Permissions::from_mode(0o755);
    let config = crate::repertoire_configuration().unwrap().join("avash");
    let ssh = home.dir().join(".ssh");
    for (dir, fichier) in [(&config, "folders.yaml"), (&ssh, "config")] {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::set_permissions(dir, ouverts.clone()).unwrap();
        ecrire_atomiquement(&dir.join(fichier), b"x").unwrap();
        let mode = std::fs::metadata(dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "{} non resserré : {mode:o}", dir.display());
    }
}

/// Un parent qui est un fichier fait échouer `create_dir_all` : on remonte
/// une erreur avant même de créer un temporaire.
///
/// Trouvé par l'audit du 7 septembre 2026 : ce cas s'appelait
/// `un_echec_ne_laisse_pas_de_temporaire` mais n'affirmait que `is_err()` et,
/// échouant dans `create_dir_all` avant `options.open(&tmp)`, ne touchait
/// jamais au nettoyage `remove_file` des branches d'écriture/renommage. Il est
/// renommé d'après ce qu'il vérifie vraiment ; l'absence de temporaire est
/// gardée par `un_renommage_impossible_ne_laisse_pas_de_temporaire`.
#[test]
fn un_parent_qui_est_un_fichier_remonte_une_erreur() {
    let home = temp_home();
    let obstacle = home.dir().join("obstacle");
    std::fs::write(&obstacle, b"je suis un fichier").unwrap();
    // « obstacle » est un fichier : on ne peut pas en faire un répertoire.
    let cible = obstacle.join("dedans.yaml");
    assert!(ecrire_atomiquement(&cible, b"x").is_err());
}

/// Le renommage sur un répertoire existant échoue APRÈS création du
/// temporaire : c'est la seule branche où `remove_file` (le nettoyage sur
/// échec de renommage) compte, et le test précédent ne l'atteignait pas.
///
/// Trouvé par l'audit du 7 septembre 2026 : aucun test ne gardait ce
/// nettoyage ; le retirer laissait un `<nom>.tmp<pid>.<n>` orphelin (fichier
/// 0600 abandonné dans `~/.ssh` ou `~/.config/avash`) sans qu'aucune suite ne
/// le dise. On vérifie d'abord qu'on est bien tombé dans la branche de
/// renommage — sans quoi une régression future qui ferait échouer plus tôt
/// (garde lecture seule étendu aux répertoires, par exemple) rendrait ce test
/// vert pour la mauvaise raison.
#[test]
fn un_renommage_impossible_ne_laisse_pas_de_temporaire() {
    let home = temp_home();
    let dir = home.dir().join("cible-est-un-dossier");
    std::fs::create_dir(&dir).unwrap();
    // Cible = un répertoire existant : le temporaire est créé, puis
    // `rename(tmp, dir)` échoue (EISDIR) — la branche à garder.
    let e = ecrire_atomiquement(&dir, b"x").unwrap_err().to_string();
    assert!(e.contains("Renommage vers"), "échec trop tôt : {e}");
    // Le nom du temporaire suit `with_extension` : pour une cible sans
    // extension, `cible-est-un-dossier.tmp<pid>.<n>`, d'où le `contains`.
    let restants: Vec<_> = std::fs::read_dir(home.dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        restants.iter().all(|n| !n.contains(".tmp")),
        "temporaire orphelin : {restants:?}"
    );
}
