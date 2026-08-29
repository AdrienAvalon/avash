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

    #[test]
    fn save_refuse_un_mot_de_passe_vide() {
        assert!(save("avash-test-vide", "").is_err());
    }

    #[test]
    fn load_rend_none_sur_une_entree_inexistante() {
        // Et surtout : ne panique pas si aucun trousseau ne tourne (CI).
        assert!(load("avash-entree-qui-n-existe-pas-xyz").is_none());
    }

    #[test]
    fn forget_ne_se_plaint_pas_sur_une_entree_absente() {
        // Idempotence : oublier deux fois ne doit pas echouer.
        let _ = forget("avash-entree-qui-n-existe-pas-xyz");
    }
}
