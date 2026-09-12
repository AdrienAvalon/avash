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
/// Réexporté : l'interface construit ses `ClientAuth` sans dépendre de
/// `zeroize` elle-même (audit du 12 septembre 2026, C-secrets-2).
pub use zeroize::Zeroizing;

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

/// Vrai quand le trousseau est simulé EN PANNE (`AVASH_TROUSSEAU=panne`) : tout
/// accès rend une erreur, comme un Secret Service absent ou un portefeuille
/// dont l'ouverture est refusée. Contrat K1 de l'audit du 12 septembre 2026
/// (C-SIL-8) : sans ce mode, le chemin « trousseau en erreur » n'était
/// jouable par aucun test, faute de pouvoir casser le vrai trousseau.
fn en_panne() -> bool {
    std::env::var_os("AVASH_TROUSSEAU").is_some_and(|v| v == "panne")
}

fn panne() -> anyhow::Error {
    anyhow!("Trousseau en panne (simulée par AVASH_TROUSSEAU=panne).")
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
    // Audit du 12 septembre 2026 (C-secrets-1) : le mot de passe d'un bureau
    // distant part sur l'entrée standard du processus RDP, dont le protocole
    // est « une ligne = un message » (le mot de passe, puis des lignes
    // `AUTORISE <chemin>`). Un saut de ligne mémorisé ici injectait donc des
    // désignations de fichiers que l'utilisateur n'avait jamais choisis, et
    // l'octet nul tronque la chaîne côté C. Aucun mot de passe réel n'en porte.
    if password.contains(['\n', '\r', '\0']) {
        return Err(anyhow!(
            "Mot de passe refusé : il contient un saut de ligne ou un octet nul."
        ));
    }
    if en_panne() {
        return Err(panne());
    }
    if en_memoire() {
        memoire().insert(cle_memoire(account), password.to_owned());
        return Ok(());
    }
    entry(account)?
        .set_password(password)
        .map_err(|e| anyhow!("Écriture dans le trousseau impossible : {e}"))
}

/// Relit un mot de passe : `Ok(None)` s'il n'y a pas d'entrée (ce n'est pas
/// une erreur), `Err` pour toute autre réponse du trousseau (absent,
/// verrouillé, refusé, délai D-Bus).
///
/// Contrat K1 de l'audit du 12 septembre 2026 (C-SIL-8) : `load` ramenait
/// toute erreur à « pas de mot de passe ». L'interface redemandait alors le
/// mot de passe sans dire pourquoi, et le chemin RDP partait même avec un mot
/// de passe vide, refusé par le serveur comme un mauvais mot de passe. Les
/// appelants qui doivent distinguer les deux cas passent par ici.
///
/// Le secret rendu s'efface à sa libération (C-secrets-2) : il ne reste pas
/// dans le tas, donc pas dans un vidage mémoire.
pub fn charger(account: &str) -> Result<Option<Zeroizing<String>>> {
    if en_panne() {
        return Err(panne());
    }
    if en_memoire() {
        return Ok(memoire()
            .get(&cle_memoire(account))
            .cloned()
            .map(Zeroizing::new));
    }
    match entry(account)?.get_password() {
        Ok(p) => Ok(Some(Zeroizing::new(p))),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(anyhow!("Lecture dans le trousseau impossible : {e}")),
    }
}

/// Relit un mot de passe. `None` si aucune entrée **ou si le trousseau est en
/// erreur** : c'est `charger(..).ok().flatten()`, gardé pour les appelants
/// pour qui « demander la saisie » suffit dans les deux cas. Ceux qui doivent
/// le dire à l'utilisateur passent par [`charger`].
#[must_use]
pub fn load(account: &str) -> Option<String> {
    charger(account).ok().flatten().map(|s| String::clone(&s))
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
    if en_panne() {
        return Err(panne());
    }
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
    if en_panne() {
        return Err(panne());
    }
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

    /// Contrat K1 de l'audit du 12 septembre 2026 (C-SIL-8) : `charger`
    /// distingue « aucune entrée » (`Ok(None)`) d'un trousseau en panne
    /// (`Err`), là où `load` confond les deux en `None` et fait redemander un
    /// mot de passe sans dire que le trousseau est tombé. Le mode
    /// `AVASH_TROUSSEAU=panne` rejoue la panne sans Secret Service.
    #[test]
    fn charger_distingue_l_absence_d_une_panne_du_trousseau() {
        let garde = crate::testutil::temp_home(); // AVASH_TROUSSEAU=memoire
        let compte = "k1@exemple:22";
        assert!(
            charger(compte).unwrap().is_none(),
            "absence : Ok(None), pas une erreur"
        );
        save(compte, "secret").unwrap();
        assert_eq!(
            charger(compte).unwrap().as_deref().map(String::as_str),
            Some("secret")
        );
        garde.poser("AVASH_TROUSSEAU", Some("panne"));
        let e = charger(compte).expect_err("un trousseau en panne est une erreur");
        assert!(e.to_string().contains("panne"), "{e}");
        assert!(
            load(compte).is_none(),
            "load reste charger().ok().flatten()"
        );
        assert!(save(compte, "autre").is_err(), "tout accès échoue en panne");
        assert!(forget(compte).is_err());
        assert!(sonder().is_err());
    }

    /// Audit du 12 septembre 2026 (C-secrets-2) : le secret relu est un type
    /// qui s'efface à sa libération. Ce test ne compile plus si `charger`
    /// revient à une `String` nue.
    #[test]
    fn le_secret_relu_est_un_type_qui_s_efface() {
        let signature: fn(&str) -> Result<Option<zeroize::Zeroizing<String>>> = charger;
        assert!(signature("absent@exemple:22").is_ok() || cfg!(not(unix)));
    }

    /// Audit du 12 septembre 2026 (C-secrets-1) : le protocole stdin du
    /// processus RDP est « une ligne = un message ». Un mot de passe mémorisé
    /// avec un saut de ligne y injectait des lignes `AUTORISE <chemin>`, donc
    /// désignait au distant un fichier que l'utilisateur n'avait pas choisi.
    /// Le trousseau refuse ces caractères dès l'écriture.
    #[test]
    fn save_refuse_un_secret_a_saut_de_ligne_ou_octet_nul() {
        let _g = crate::testutil::temp_home();
        for piege in ["x\nAUTORISE /etc/passwd", "x\ry", "x\0y"] {
            let e = save("piege@exemple:22", piege).unwrap_err().to_string();
            assert!(
                e.contains("saut de ligne"),
                "refusé pour la mauvaise raison : {e}"
            );
        }
        assert!(load("piege@exemple:22").is_none(), "rien n'a été écrit");
    }

    #[test]
    fn load_rend_none_sur_une_entree_inexistante() {
        // Sous le verrou de l'environnement : un test voisin peut poser
        // AVASH_TROUSSEAU (mode `panne`, audit du 12 septembre 2026), et ce
        // test lit la variable pour choisir son trousseau.
        let _verrou = crate::testutil::verrou_environnement();
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
        // Voir `load_rend_none_sur_une_entree_inexistante` : sans le verrou,
        // ce test voyait le mode `panne` posé par un voisin (vu le 12 septembre
        // 2026 à l'ajout de ce mode).
        let _verrou = crate::testutil::verrou_environnement();
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
