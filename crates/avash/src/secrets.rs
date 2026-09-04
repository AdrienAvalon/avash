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

fn entry(account: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, account).map_err(|e| anyhow!("Trousseau inaccessible : {e}"))
}

/// Enregistre un mot de passe.
pub fn save(account: &str, password: &str) -> Result<()> {
    if password.is_empty() {
        return Err(anyhow!("Mot de passe vide."));
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
    match entry("diagnostic-sonde")?.get_password() {
        Ok(_) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(anyhow!("{e}")),
    }
}

/// Oublie un mot de passe. Ne se plaint pas s'il n'y en avait pas.
pub fn forget(account: &str) -> Result<()> {
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
