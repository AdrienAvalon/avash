//! Confiance au serveur (TOFU) : empreinte du certificat, fichier des empreintes, répertoire de configuration.

use anyhow::{Context, Result};

pub(crate) fn server_public_key(cert: &x509_cert::Certificate) -> Result<Vec<u8>> {
    cert.tbs_certificate
        .subject_public_key_info
        .subject_public_key
        .as_bytes()
        .context("clé publique non alignée")
        .map(<[u8]>::to_vec)
}

/// Verdict d'un certificat de serveur RDP, au regard des empreintes mémorisées.
#[derive(Debug, PartialEq, Eq)]
pub enum VerdictCert {
    /// Rien de mémorisé pour cet hôte : premier contact.
    PremierContact,
    /// L'empreinte présentée correspond à celle mémorisée.
    Connu,
    /// Une empreinte est mémorisée, mais ce n'est pas celle-ci.
    Change { attendue: String },
}

/// Compare l'empreinte présentée à celle mémorisée pour cet hôte.
///
/// Même modèle que le `known_hosts` de SSH. Sans cela, `ironrdp_tls::upgrade`
/// accepte **n'importe quel** certificat (il installe `NoCertificateVerification`)
/// et l'on enchaîne sur CredSSP/NLA — c'est-à-dire qu'on livre les identifiants
/// à qui se présente. L'asymétrie avec le volet SSH était totale.
#[must_use]
pub fn juger_certificat(memorisee: Option<&str>, presentee: &str) -> VerdictCert {
    match memorisee {
        None => VerdictCert::PremierContact,
        Some(m) if m == presentee => VerdictCert::Connu,
        Some(m) => VerdictCert::Change {
            attendue: m.to_owned(),
        },
    }
}

/// Empreinte SHA-256 de la clé publique du serveur, en hexadécimal minuscule.
///
/// On épingle la clé plutôt que le certificat entier : une simple reconduction
/// du certificat, à clé inchangée, ne doit pas déclencher de fausse alerte.
pub(crate) fn empreinte(der: &[u8]) -> String {
    use sha2::Digest as _;
    let condense = sha2::Sha256::digest(der);
    condense.iter().fold(String::new(), |mut acc, o| {
        use std::fmt::Write as _;
        let _ = write!(acc, "{o:02x}");
        acc
    })
}

/// Fichier des empreintes mémorisées, à côté du reste de la configuration.
///
/// Répertoire de configuration, `AVASH_HOME` faisant foi s'il est posé.
///
/// Le cœur honore déjà cette variable ; ce processus, non — et l'écart ne se
/// voyait pas sous Linux, où `config_dir()` suit `XDG_CONFIG_HOME` que le bac à
/// sable des tests pose déjà. Sous Windows, `config_dir()` interroge le shell
/// et ignore aussi bien `HOME` que `XDG_CONFIG_HOME` : la suite bout en bout y
/// aurait écrit dans le fichier de confiance RÉEL de l'utilisateur, y semant
/// des serveurs de test et, pire, l'exposant à voir une empreinte légitime
/// écrasée par celle d'un serveur jetable.
pub(crate) fn repertoire_configuration() -> Option<std::path::PathBuf> {
    if let Some(home) = std::env::var_os("AVASH_HOME") {
        return Some(std::path::PathBuf::from(home).join(".config"));
    }
    dirs::config_dir()
}

/// Où l'on note les serveurs qui n'ont que le canal graphique pour dessiner.
pub fn chemin_canal_graphique() -> Option<std::path::PathBuf> {
    Some(
        repertoire_configuration()?
            .join("avash")
            .join("rdp_canal_graphique"),
    )
}

/// Sans répertoire de configuration, on **échoue** au lieu de retomber sur le
/// répertoire courant : y semer un fichier de confiance le rendrait inopérant
/// au prochain lancement depuis ailleurs — chaque serveur redeviendrait un
/// premier contact, en silence.
fn chemin_empreintes() -> anyhow::Result<std::path::PathBuf> {
    Ok(repertoire_configuration()
        .context("répertoire de configuration introuvable (HOME/XDG_CONFIG_HOME)")?
        .join("avash")
        .join("rdp_known_hosts"))
}

/// Empreinte mémorisée pour `hote:port`, s'il y en a une.
///
/// Rend `Ok(None)` UNIQUEMENT quand le fichier n'existe pas (aucun contact
/// mémorisé). Toute autre erreur — droits, ou octet non UTF-8 laissé par un
/// éditeur Windows qui aurait réenregistré `rdp_known_hosts` en UTF-16 ou
/// Latin-1, précisément le fichier que le message de refus invite à éditer à la
/// main — est PROPAGÉE. Trouvé par l'audit du 7 septembre 2026 : le
/// `read_to_string(...).ok()?` d'avant réduisait ces erreurs à « rien de
/// mémorisé », l'appelant enchaînait sur `PremierContact`, acceptait l'empreinte
/// présentée (fût-elle celle d'un intercepteur) et `memoriser_empreinte`
/// réécrivait le fichier avec une seule ligne — désarmant le TOFU pour TOUS les
/// hôtes et effaçant toutes les autres empreintes, sans un mot.
pub(crate) fn empreinte_memorisee(cle: &str) -> Result<Option<String>> {
    let chemin = chemin_empreintes()?;
    match std::fs::read(&chemin) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("lecture de {}", chemin.display())),
        Ok(octets) => {
            let contenu =
                String::from_utf8(octets).context("rdp_known_hosts n'est pas en UTF-8")?;
            Ok(chercher_empreinte(&contenu, cle))
        }
    }
}

/// Cherche l'empreinte de `cle` dans le contenu d'un fichier d'empreintes.
///
/// Séparée de la lecture pour être exerçable : c'est ici que se joue la
/// différence entre « ce serveur est connu » et « premier contact », donc entre
/// refuser un imposteur et l'accepter.
fn chercher_empreinte(contenu: &str, cle: &str) -> Option<String> {
    contenu.lines().find_map(|l| {
        let (h, e) = l.split_once(' ')?;
        (h == cle).then(|| e.trim().to_owned())
    })
}

/// Mémorise l'empreinte d'un hôte au premier contact.
pub(crate) fn memoriser_empreinte(cle: &str, empreinte: &str) -> anyhow::Result<()> {
    let chemin = chemin_empreintes()?;
    if let Some(dir) = chemin.parent() {
        std::fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
    }
    // On relit d'abord l'existant pour deux raisons : refuser un fichier non
    // UTF-8 (un octet laissé par un ré-enregistrement Windows en UTF-16/Latin-1 ;
    // `unwrap_or_default()` le prenait pour un fichier vide) sans jamais y toucher,
    // et décider s'il faut préfixer un saut de ligne devant un fichier hérité qui
    // n'en aurait pas.
    let besoin_saut = match std::fs::read(&chemin) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(e).with_context(|| format!("lecture de {}", chemin.display())),
        Ok(octets) => {
            let contenu =
                String::from_utf8(octets).context("rdp_known_hosts n'est pas en UTF-8")?;
            !contenu.is_empty() && !contenu.ends_with('\n')
        }
    };
    // On AJOUTE la ligne en O_APPEND au lieu de réécrire tout le fichier. Trouvé
    // par l'audit du 7 septembre 2026 : la lecture-modification-réécriture (relire
    // puis renommer un temporaire par-dessus, via `atomique::ecrire`) perdait une
    // empreinte quand deux sidecars atteignaient ce point ensemble — deux onglets
    // ouverts à la suite lisaient le même contenu et le dernier `rename` effaçait
    // la ligne du premier ; l'atomicité du rename ne couvre pas ce cas, et l'hôte
    // perdu redevenait « premier contact ». Un `write_all` unique en O_APPEND est
    // atomique entre processus sur un FS local et ne tronque jamais. Une seconde
    // ligne pour le même hôte serait inoffensive : `chercher_empreinte` retient la
    // première (test `la_premiere_entree_fait_foi`).
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut fichier = options
        .open(&chemin)
        .with_context(|| format!("ouverture de {}", chemin.display()))?;
    let ligne = if besoin_saut {
        format!("\n{cle} {empreinte}\n")
    } else {
        format!("{cle} {empreinte}\n")
    };
    fichier
        .write_all(ligne.as_bytes())
        .with_context(|| format!("écriture de {}", chemin.display()))
}

#[cfg(test)]
mod tests_certificat {
    use super::{juger_certificat, VerdictCert};

    #[test]
    fn rien_de_memorise_donne_un_premier_contact() {
        assert_eq!(juger_certificat(None, "aa"), VerdictCert::PremierContact);
    }

    #[test]
    fn la_meme_empreinte_est_reconnue() {
        assert_eq!(juger_certificat(Some("aa"), "aa"), VerdictCert::Connu);
    }

    /// Le cœur du correctif : sans lui, `ironrdp_tls::upgrade` acceptait
    /// n'importe quel certificat, puis CredSSP livrait les identifiants.
    #[test]
    fn une_empreinte_differente_est_un_changement() {
        assert_eq!(
            juger_certificat(Some("aa"), "bb"),
            VerdictCert::Change {
                attendue: "aa".into()
            }
        );
    }
}

/// Verrou des tests qui posent `AVASH_HOME` : la variable est globale au
/// processus et `cargo test` fait tourner les tests en parallèle ; sans lui,
/// le test du montage VeNCrypt pouvait lire le bac à sable d'un autre test.
#[cfg(test)]
pub(crate) static VERROU_AVASH_HOME: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests_fichier_empreintes {
    use super::chercher_empreinte;

    /// Ne rien trouver vaut « premier contact », donc acceptation et
    /// mémorisation : toute entrée que la recherche rate revient à désarmer le
    /// TOFU pour cet hôte, en silence. Ces cas-là méritaient un test.
    #[test]
    fn une_entree_presente_est_retrouvee() {
        let contenu = "a:3389 aaaa\nsrv.exemple:3389 bbbb\nz:3389 cccc\n";
        assert_eq!(
            chercher_empreinte(contenu, "srv.exemple:3389").as_deref(),
            Some("bbbb")
        );
        // Dernière ligne sans saut de ligne final.
        assert_eq!(chercher_empreinte("x:1 dd", "x:1").as_deref(), Some("dd"));
    }

    #[test]
    fn un_fichier_vide_ou_abime_ne_fait_pas_trouver_n_importe_quoi() {
        for contenu in ["", "\n\n", "ligne-sans-espace\n", "  \n"] {
            assert_eq!(chercher_empreinte(contenu, "srv:3389"), None, "{contenu:?}");
        }
    }

    #[test]
    fn une_cle_voisine_ne_correspond_pas() {
        let contenu = "srv.exemple:3389 bbbb\n";
        for cle in [
            "srv.exemple:3390",
            "srv.exemple",
            "srv.exemple:33890",
            "rv.exemple:3389",
        ] {
            assert_eq!(chercher_empreinte(contenu, cle), None, "{cle}");
        }
    }

    /// `AVASH_HOME` détourne le fichier de confiance vers le bac à sable : sans
    /// cela, la suite bout en bout sous Windows écrirait dans le
    /// `rdp_known_hosts` réel de l'utilisateur, où `config_dir()` ignore `HOME`.
    #[test]
    fn avash_home_detourne_le_fichier_de_confiance() {
        // Sans cela, la suite bout en bout sous Windows écrirait dans le
        // fichier réel de l'utilisateur : `config_dir()` y ignore HOME.
        let _verrou = super::VERROU_AVASH_HOME
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let bac = std::env::temp_dir().join(format!("avash-rdp-{}", std::process::id()));
        let precedent = std::env::var_os("AVASH_HOME");
        unsafe { std::env::set_var("AVASH_HOME", &bac) };
        let sous_bac = crate::empreintes::chemin_empreintes().expect("un chemin");
        unsafe {
            match precedent {
                Some(v) => std::env::set_var("AVASH_HOME", v),
                None => std::env::remove_var("AVASH_HOME"),
            }
        }
        assert!(
            sous_bac.starts_with(&bac),
            "le fichier de confiance doit suivre AVASH_HOME, or il pointe sur {sous_bac:?}"
        );
        assert!(sous_bac.ends_with("rdp_known_hosts"));
    }

    /// Deux entrées pour le même hôte : c'est la première qui fait foi, et elle
    /// doit être trouvée — sans quoi une ligne ajoutée en fin de fichier
    /// masquerait l'empreinte d'origine.
    #[test]
    fn la_premiere_entree_fait_foi() {
        let contenu = "srv:3389 originale\nsrv:3389 ajoutee\n";
        assert_eq!(
            chercher_empreinte(contenu, "srv:3389").as_deref(),
            Some("originale")
        );
    }

    /// Un `rdp_known_hosts` non-UTF-8 refuse la connexion au lieu de désarmer le
    /// TOFU, et ne se fait pas écraser.
    ///
    /// Trouvé par l'audit du 7 septembre 2026 : l'utilisateur, invité par le
    /// message de refus à retirer une ligne de `rdp_known_hosts`, ouvre le
    /// fichier sous Windows et l'enregistre en UTF-16 (ou y colle un accent en
    /// Latin-1). Avant le correctif, `read_to_string(...).ok()?` rendait `None`
    /// pour un octet non UTF-8, l'appelant concluait `PremierContact` (empreinte
    /// acceptée, identifiants livrés à un éventuel intercepteur) et
    /// `memoriser_empreinte` réécrivait le fichier d'une seule ligne, effaçant
    /// toutes les autres empreintes. Désormais : erreur des deux côtés, fichier
    /// intact.
    #[test]
    fn un_rdp_known_hosts_non_utf8_refuse_au_lieu_de_desarmer_le_tofu() {
        use super::{empreinte_memorisee, memoriser_empreinte};
        let _verrou = super::VERROU_AVASH_HOME
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let bac = std::env::temp_dir().join(format!("avash-rdp-utf8-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&bac);
        let precedent = std::env::var_os("AVASH_HOME");
        unsafe { std::env::set_var("AVASH_HOME", &bac) };

        let resultat = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let chemin = super::chemin_empreintes().expect("un chemin");
            std::fs::create_dir_all(chemin.parent().unwrap()).unwrap();
            // Une entrée légitime, suivie d'un octet non UTF-8 (0xFF) comme en
            // laisserait un ré-enregistrement en UTF-16/Latin-1.
            let octets_abimes = b"srv.exemple:3389 aaaa\n\xff\n";
            std::fs::write(&chemin, octets_abimes).unwrap();

            // Lecture : erreur explicite, jamais « rien de mémorisé ».
            let lu = empreinte_memorisee("srv.exemple:3389");
            assert!(
                lu.is_err(),
                "un fichier non-UTF-8 doit être une erreur, pas Ok(None) : {lu:?}"
            );

            // Mémorisation : erreur AUSSI, et le fichier n'est pas écrasé.
            let ecrit = memoriser_empreinte("autre.hote:3389", "bbbb");
            assert!(
                ecrit.is_err(),
                "on ne réécrit pas un fichier qu'on n'a pas su lire : {ecrit:?}"
            );
            assert_eq!(
                std::fs::read(&chemin).unwrap(),
                octets_abimes,
                "l'empreinte d'origine et le contenu abîmé restent intacts"
            );
        }));

        unsafe {
            match precedent {
                Some(v) => std::env::set_var("AVASH_HOME", v),
                None => std::env::remove_var("AVASH_HOME"),
            }
        }
        let _ = std::fs::remove_dir_all(&bac);
        if let Err(p) = resultat {
            std::panic::resume_unwind(p);
        }
    }

    /// Plusieurs sidecars `avash-rdp` mémorisant un premier contact au même
    /// instant — deux onglets ouverts à la suite, restauration de plusieurs
    /// bureaux — ne doivent perdre aucune empreinte.
    ///
    /// Trouvé par l'audit du 7 septembre 2026 : `memoriser_empreinte` relisait
    /// tout le fichier, ajoutait sa ligne et renommait un temporaire par-dessus
    /// (`atomique::ecrire`). Deux processus lisant le même contenu voyaient le
    /// dernier `rename` effacer la ligne du premier — l'atomicité du rename ne
    /// couvre pas la lecture-modification-écriture concurrente. L'hôte perdu
    /// redevenait « premier contact » et acceptait n'importe quelle clé à la
    /// connexion suivante, TOFU désarmé en silence. L'ajout en O_APPEND, atomique
    /// entre processus sur un FS local, fait survivre toutes les lignes.
    #[test]
    fn des_premiers_contacts_simultanes_survivent_tous() {
        use super::{chemin_empreintes, chercher_empreinte, memoriser_empreinte};
        let _verrou = super::VERROU_AVASH_HOME
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let bac = std::env::temp_dir().join(format!("avash-rdp-conc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&bac);
        let precedent = std::env::var_os("AVASH_HOME");
        unsafe { std::env::set_var("AVASH_HOME", &bac) };

        let resultat = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            const N: usize = 16;
            // Tous les fils s'élancent ensemble (barrière) pour maximiser le
            // recouvrement des lectures-écritures, comme des sidecars lancés à la
            // suite. Sans le correctif, plusieurs lignes disparaissent.
            let depart = std::sync::Arc::new(std::sync::Barrier::new(N));
            let fils: Vec<_> = (0..N)
                .map(|i| {
                    let depart = std::sync::Arc::clone(&depart);
                    std::thread::spawn(move || {
                        depart.wait();
                        memoriser_empreinte(&format!("hote{i}:3389"), &format!("fp{i}"))
                            .expect("mémorisation");
                    })
                })
                .collect();
            for f in fils {
                f.join().expect("fil terminé");
            }
            let chemin = chemin_empreintes().expect("un chemin");
            let contenu = std::fs::read_to_string(&chemin).expect("lecture");
            for i in 0..N {
                assert_eq!(
                    chercher_empreinte(&contenu, &format!("hote{i}:3389")).as_deref(),
                    Some(format!("fp{i}").as_str()),
                    "empreinte de hote{i} perdue ; fichier :\n{contenu}"
                );
            }
        }));

        unsafe {
            match precedent {
                Some(v) => std::env::set_var("AVASH_HOME", v),
                None => std::env::remove_var("AVASH_HOME"),
            }
        }
        let _ = std::fs::remove_dir_all(&bac);
        if let Err(p) = resultat {
            std::panic::resume_unwind(p);
        }
    }
}
