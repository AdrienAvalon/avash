//! Mémoire des onglets ouverts : ce qui l'était à la dernière fermeture, pour
//! proposer de le rouvrir au lancement suivant.
//!
//! Le front l'écrit à chaque ouverture ou fermeture d'onglet, pas à la sortie
//! de l'application : sous `WebKitGTK`, la fermeture de la fenêtre ne laisse
//! pas toujours le temps à un `beforeunload`, et une mémoire qui manquait la
//! dernière minute mentait. Elle vit dans le répertoire de configuration
//! (`onglets.json`), à côté des bureaux RDP et des snippets : la suite bout en
//! bout le vide avant chaque fichier de scénarios, l'isolation est la même.
//! Seuls des hôtes déclarés y figurent (alias SSH, identifiant de bureau) :
//! une connexion directe, avec son mot de passe saisi, n'est pas rejouable et
//! n'a pas à laisser de trace.

use serde::{Deserialize, Serialize};

/// Un onglet à rouvrir : un hôte de `~/.ssh/config` ou un bureau enregistré.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum OngletMemorise {
    Ssh { alias: String },
    Rdp { host_id: String },
}

/// Plus que ça, ce n'est plus une session de travail, c'est un fichier
/// accidentel : on ne rouvrirait pas cent onglets d'un coup.
const ONGLETS_MAX: usize = 32;

fn chemin() -> Result<std::path::PathBuf, String> {
    avash::repertoire_configuration()
        .map(|d| d.join("avash").join("onglets.json"))
        .ok_or_else(|| "Répertoire de configuration introuvable.".to_owned())
}

/// Écrit la liste (atomiquement, en 0600) ; une liste vide retire le fichier.
#[tauri::command]
pub fn onglets_memoriser(onglets: Vec<OngletMemorise>) -> Result<(), String> {
    let chemin = chemin()?;
    if onglets.is_empty() {
        return match std::fs::remove_file(&chemin) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        };
    }
    let liste: Vec<&OngletMemorise> = onglets.iter().take(ONGLETS_MAX).collect();
    let json = serde_json::to_vec_pretty(&liste).map_err(|e| e.to_string())?;
    if let Some(parent) = chemin.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    avash::ecrire_atomiquement(&chemin, &json).map_err(|e| e.to_string())
}

/// La liste mémorisée ; un fichier absent ou illisible vaut « rien » : on ne
/// bloque pas un lancement pour une mémoire cassée.
#[tauri::command]
#[must_use]
pub fn onglets_memorises() -> Vec<OngletMemorise> {
    let Ok(chemin) = chemin() else {
        return Vec::new();
    };
    std::fs::read(&chemin)
        .ok()
        .and_then(|b| serde_json::from_slice::<Vec<OngletMemorise>>(&b).ok())
        .map_or_else(Vec::new, |mut l| {
            l.truncate(ONGLETS_MAX);
            l
        })
}

#[cfg(test)]
mod tests_onglets {
    use super::{onglets_memoriser, onglets_memorises, OngletMemorise};
    use crate::commands::tests::with_ssh_config;

    /// Aller-retour : ce qui est écrit se relit dans l'ordre, une liste vide
    /// efface, et l'absence de fichier vaut « rien ».
    #[test]
    fn la_memoire_se_relit_dans_l_ordre_et_s_efface_sur_une_liste_vide() {
        let _g = with_ssh_config("");
        assert!(onglets_memorises().is_empty());
        let liste = vec![
            OngletMemorise::Ssh {
                alias: "web-1".into(),
            },
            OngletMemorise::Rdp {
                host_id: "b7".into(),
            },
            OngletMemorise::Ssh {
                alias: "db-1".into(),
            },
        ];
        onglets_memoriser(liste.clone()).unwrap();
        assert_eq!(onglets_memorises(), liste);
        onglets_memoriser(Vec::new()).unwrap();
        assert!(onglets_memorises().is_empty());
        // Effacer deux fois ne se plaint pas.
        onglets_memoriser(Vec::new()).unwrap();
    }

    /// Un fichier corrompu ou trop long ne bloque rien : rien à rouvrir, ou
    /// seulement les premiers.
    #[test]
    fn une_memoire_cassee_vaut_rien_et_une_trop_longue_est_tronquee() {
        let _g = with_ssh_config("");
        let chemin = avash::repertoire_configuration()
            .unwrap()
            .join("avash")
            .join("onglets.json");
        std::fs::create_dir_all(chemin.parent().unwrap()).unwrap();
        std::fs::write(&chemin, b"{pas du json").unwrap();
        assert!(onglets_memorises().is_empty());
        let trop: Vec<OngletMemorise> = (0..100)
            .map(|i| OngletMemorise::Ssh {
                alias: format!("h{i}"),
            })
            .collect();
        onglets_memoriser(trop).unwrap();
        assert_eq!(onglets_memorises().len(), super::ONGLETS_MAX);
    }
}
