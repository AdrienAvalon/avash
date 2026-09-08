//! Stockage des mots de passe dans le trousseau du système.
//!
//! ⚠️ Pourquoi pas `~/.ssh/config` : ce fichier est en clair, et OpenSSH
//! n'a d'ailleurs aucune directive pour y mettre un mot de passe. L'y écrire
//! reviendrait à le poser en clair sur le disque.
//!
//! Le trousseau du système fait ce travail correctement, et il est déjà là :
//! `KWallet` ou GNOME Keyring sous Linux (via Secret Service), le Gestionnaire
//! d'identifiants sous Windows, le Trousseau sous macOS. Le déverrouillage,
//! le chiffrement et la révocation sont gérés par le système — pas par nous.

use anyhow::{anyhow, Result};

/// Nom sous lequel Avash apparaît dans le trousseau.
const SERVICE: &str = "avash";

/// Identifiant d'une entrée. `user@hôte:port` est lisible tel quel dans
/// `KWallet` ou seahorse, ce qui permet de retrouver et révoquer à la main.
#[must_use]
pub fn account_id(user: &str, addr: &str, port: u16) -> String {
    format!("{user}@{addr}:{port}")
}

/// Trousseau simulé en mémoire, activé par `AVASH_TROUSSEAU=memoire`.
///
/// Trouvé par l'audit du 7 septembre 2026 : `keyring = "4"` active par défaut
/// `zbus-secret-service` sous Linux, donc chaque `save`/`load`/`sonder`/`forget`
/// est un aller-retour D-Bus vers le vrai Secret Service. Les tests d'`avash-ui`
/// qui passent par `Target::from_alias` (donc `load`) ou par le diagnostic
/// (`sonder`) interrogeaient ainsi le trousseau réel du poste : `with_ssh_config`
/// et `temp_home` isolent `~/.ssh`, pas le trousseau. Poste avec `KWallet` fermé,
/// la suite ouvrait la boîte de déverrouillage ou attendait le délai D-Bus ; et
/// une entrée réelle `deploy@10.0.0.1:2222` remontait un mot de passe fantôme
/// qui faisait échouer `assert!(t.password.is_none())`.
///
/// La dérogation `AVASH_TROUSSEAU=memoire` remplace le trousseau par cette table.
/// Les entrées sont préfixées par `AVASH_HOME` : chaque bac à sable de test a son
/// trousseau vierge, comme il a déjà son `~/.ssh` vierge. Les helpers de test
/// (`testutil::temp_home`, `with_ssh_config` côté interface) posent la variable
/// sous le verrou `HOME_LOCK`, à côté d'`AVASH_HOME`.
static MEMOIRE: std::sync::Mutex<std::collections::BTreeMap<String, String>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// Vrai quand le trousseau est simulé en mémoire.
///
/// Le test se fait dans chaque fonction publique, AVANT tout `keyring::Entry::new`
/// (et donc avant `entry()`) : le premier appel à ce constructeur déclenche un
/// `LazyLock` global qui ouvre déjà la connexion D-Bus. Court-circuiter en amont
/// garantit qu'aucun trousseau réel n'est touché sous ce mode.
fn en_memoire() -> bool {
    std::env::var_os("AVASH_TROUSSEAU").is_some_and(|v| v == "memoire")
}

/// Verrou de la table mémoire, tolérant à l'empoisonnement (un test qui panique
/// ne doit pas figer les suivants).
fn memoire() -> std::sync::MutexGuard<'static, std::collections::BTreeMap<String, String>> {
    MEMOIRE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Clé de la table mémoire, préfixée par le bac à sable (`AVASH_HOME`) pour que
/// deux tests n'y partagent rien.
fn cle_memoire(account: &str) -> String {
    let home = std::env::var("AVASH_HOME").unwrap_or_default();
    format!("{home}\u{0}{account}")
}

fn entry(account: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, account).map_err(|e| anyhow!("Trousseau inaccessible : {e}"))
}

/// Enregistre un mot de passe.
pub fn save(account: &str, password: &str) -> Result<()> {
    if password.is_empty() {
        return Err(anyhow!("Mot de passe vide."));
    }
    if en_memoire() {
        memoire().insert(cle_memoire(account), password.to_owned());
        return Ok(());
    }
    entry(account)?
        .set_password(password)
        .map_err(|e| anyhow!("Écriture dans le trousseau impossible : {e}"))
}

/// Relit un mot de passe. `None` si aucune entrée — ce n'est pas une erreur.
#[must_use]
pub fn load(account: &str) -> Option<String> {
    // Toute erreur (trousseau verrouillé, absent, entrée inexistante) est
    // traitee comme « pas de mot de passe » : l'interface demandera la
    // saisie. Bloquer la connexion parce que le trousseau dort serait pire.
    if en_memoire() {
        return memoire().get(&cle_memoire(account)).cloned();
    }
    entry(account).ok()?.get_password().ok()
}

/// Dit si le trousseau répond, sans rien y écrire ni y lire de réel.
///
/// Pour le diagnostic exporté : « mot de passe redemandé à chaque fois » vient
/// presque toujours d'un trousseau absent (pas de Secret Service dans la
/// session, `KWallet` fermé), que `load` masque à dessein. On interroge une
/// entrée qui n'existe pas : un trousseau vivant répond « aucune entrée », un
/// trousseau absent ou verrouillé répond autre chose, et c'est cette réponse
/// qu'on rapporte.
pub fn sonder() -> Result<()> {
    // Backend mémoire : il répond toujours, il n'y a pas de trousseau système
    // à sonder.
    if en_memoire() {
        return Ok(());
    }
    match entry("diagnostic-sonde")?.get_password() {
        Ok(_) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(anyhow!("{e}")),
    }
}

/// Oublie un mot de passe. Ne se plaint pas s'il n'y en avait pas.
pub fn forget(account: &str) -> Result<()> {
    if en_memoire() {
        memoire().remove(&cle_memoire(account));
        return Ok(());
    }
    match entry(account)?.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(anyhow!("Suppression dans le trousseau impossible : {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_id_est_lisible_dans_le_trousseau() {
        // Ce libelle s'affiche tel quel dans KWallet : il doit permettre de
        // reconnaitre et revoquer une entree a la main.
        assert_eq!(account_id("root", "10.0.0.1", 22), "root@10.0.0.1:22");
        assert_eq!(account_id("deploy", "srv", 2222), "deploy@srv:2222");
    }

    /// Le garde du mot de passe vide doit être vérifié SUR SON MESSAGE.
    ///
    /// Un simple `is_err()` passait pour la mauvaise raison : sans trousseau —
    /// le cas de l'intégration continue — `entry()` échoue de toute façon, et
    /// supprimer le garde laissait le test vert.
    #[test]
    fn save_refuse_un_mot_de_passe_vide() {
        let e = save("avash-test-vide", "").unwrap_err().to_string();
        assert!(
            e.contains("Mot de passe vide"),
            "refusé pour la mauvaise raison : {e}"
        );
    }

    /// Trouvé par l'audit du 7 septembre 2026 : avec un Secret Service actif
    /// (ici ksecretd/KWallet), `secrets::load` lisait le trousseau réel du
    /// poste, même sous `temp_home`/`with_ssh_config` qui n'isolent que
    /// `~/.ssh`. On plante une entrée directement dans le vrai trousseau (hors
    /// backend mémoire), puis on vérifie que, sous `AVASH_TROUSSEAU=memoire`,
    /// `load` ne la voit pas : le bac à sable démarre trousseau vierge.
    ///
    /// Avant le correctif, `load(compte)` ramenait « mdp-du-poste » et
    /// l'assertion échouait ; c'est exactement ce qui faisait tomber
    /// `assert!(t.password.is_none())` d'`avash-ui` sur un poste portant une
    /// entrée `deploy@10.0.0.1:2222`.
    #[test]
    fn le_backend_memoire_isole_du_trousseau_reel_du_poste() {
        let compte = "avash-audit-isolation-2026-09-07@exemple:22";
        // Plante dans le VRAI trousseau, sans passer par le backend mémoire.
        let Ok(e) = keyring::Entry::new(SERVICE, compte) else {
            return; // pas de trousseau accessible (CI) : rien à isoler
        };
        if e.set_password("mdp-du-poste").is_err() {
            return; // trousseau présent mais non inscriptible : idem
        }
        {
            let _g = crate::testutil::temp_home(); // pose AVASH_TROUSSEAU=memoire
            assert!(
                load(compte).is_none(),
                "le backend mémoire ne doit jamais voir une entrée du trousseau réel"
            );
        }
        // Ménage systématique de l'entrée plantée.
        let _ = e.delete_credential();
    }

    /// Sous le backend mémoire, le cycle save/load/forget est déterministe et
    /// ne touche aucun trousseau système : utilisable même en CI sans Secret
    /// Service, là où `save` échouait faute de démon.
    #[test]
    fn le_backend_memoire_fait_un_cycle_save_load_forget() {
        let _g = crate::testutil::temp_home(); // pose AVASH_TROUSSEAU=memoire
        let compte = "cycle@exemple:22";
        assert!(
            load(compte).is_none(),
            "trousseau vierge à l'entrée du bac à sable"
        );
        save(compte, "secret").unwrap();
        assert_eq!(load(compte).as_deref(), Some("secret"));
        forget(compte).unwrap();
        assert!(load(compte).is_none(), "oublié après forget");
        sonder().unwrap();
    }

    #[test]
    fn load_rend_none_sur_une_entree_inexistante() {
        // Et surtout : ne panique pas si aucun trousseau ne tourne (CI).
        assert!(load("avash-entree-qui-n-existe-pas-xyz").is_none());
    }

    /// Idempotence : une entrée absente n'est pas une erreur.
    ///
    /// Le test n'affirmait rien — le `Result` était jeté — et n'appelait qu'une
    /// fois, alors que c'est le bras `Err(NoEntry) => Ok(())` qui est visé.
    ///
    /// Il ne peut pas exiger `is_ok()` sans condition : sans démon de trousseau
    /// — le cas de l'intégration continue — `entry()` échoue avant même
    /// d'atteindre ce bras, et l'échec est alors LÉGITIME. Ce qu'on affirme,
    /// c'est que « pas d'entrée » ne remonte jamais comme une erreur : le seul
    /// échec toléré est l'inaccessibilité du trousseau lui-même.
    #[test]
    fn forget_ne_se_plaint_pas_sur_une_entree_absente() {
        let compte = "avash-entree-qui-n-existe-pas-xyz";
        match forget(compte) {
            // Trousseau présent : le second appel doit passer aussi.
            Ok(()) => assert!(forget(compte).is_ok(), "l'absence n'est pas une erreur"),
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("Trousseau inaccessible"),
                    "seule l'absence de trousseau est un échec acceptable ici : {msg}"
                );
            }
        }
    }
}
