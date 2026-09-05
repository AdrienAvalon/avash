//! Confiance au serveur (TOFU) : empreinte du certificat, fichier des empreintes, répertoire de configuration.

use crate::atomique;
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

/// Sans répertoire de configuration, on **échoue** au lieu de retomber sur le
/// répertoire courant : y semer un fichier de confiance le rendrait inopérant
/// au prochain lancement depuis ailleurs — chaque serveur redeviendrait un
/// premier contact, en silence.
/// Où l'on note les serveurs qui n'ont que le canal graphique pour dessiner.
pub(crate) fn chemin_canal_graphique() -> Option<std::path::PathBuf> {
    Some(
        repertoire_configuration()?
            .join("avash")
            .join("rdp_canal_graphique"),
    )
}

fn chemin_empreintes() -> anyhow::Result<std::path::PathBuf> {
    Ok(repertoire_configuration()
        .context("répertoire de configuration introuvable (HOME/XDG_CONFIG_HOME)")?
        .join("avash")
        .join("rdp_known_hosts"))
}

/// Empreinte mémorisée pour `hote:port`, s'il y en a une.
pub(crate) fn empreinte_memorisee(cle: &str) -> Option<String> {
    let contenu = std::fs::read_to_string(chemin_empreintes().ok()?).ok()?;
    chercher_empreinte(&contenu, cle)
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
    let mut contenu = std::fs::read_to_string(&chemin).unwrap_or_default();
    if !contenu.is_empty() && !contenu.ends_with('\n') {
        contenu.push('\n');
    }
    contenu.push_str(&format!("{cle} {empreinte}\n"));
    // Écriture atomique (voir `atomique`) : c'est ce fichier-ci qui compte le
    // plus — le perdre ramène TOUS les serveurs à « premier contact », et le
    // TOFU cesse de protéger sans que rien ne le signale.
    atomique::ecrire(&chemin, contenu.as_bytes())
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

    /// Deux entrées pour le même hôte : c'est la première qui fait foi, et elle
    /// doit être trouvée — sans quoi une ligne ajoutée en fin de fichier
    /// masquerait l'empreinte d'origine.
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

    #[test]
    fn la_premiere_entree_fait_foi() {
        let contenu = "srv:3389 originale\nsrv:3389 ajoutee\n";
        assert_eq!(
            chercher_empreinte(contenu, "srv:3389").as_deref(),
            Some("originale")
        );
    }
}
