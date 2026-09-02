//! Enregistrement de session (asciicast) : démarrer, arrêter, lister.

use super::{Enregistrement, SessionStore};

pub(crate) fn enregistreur_de(
    state: &tauri::State<'_, SessionStore>,
    id: u64,
) -> Option<Enregistrement> {
    state
        .inner
        .lock()
        .unwrap()
        .get(&id)
        .map(|h| h.enregistreur.clone())
}

/// Démarre l'enregistrement de la session dans un fichier asciicast v2, et
/// rend son chemin. Seule la sortie est enregistrée, jamais les frappes.
///
/// `etat_initial` est l'écran tel qu'il est au moment de démarrer, sérialisé
/// par le front (séquences d'échappement comprises) : sans lui, un
/// enregistrement lancé en cours de session rejouait à partir d'un écran noir.
#[tauri::command]
pub fn enregistrement_demarrer(
    state: tauri::State<'_, SessionStore>,
    id: u64,
    cols: u32,
    rows: u32,
    etat_initial: Option<String>,
) -> Result<String, String> {
    let (enregistreur, label) = {
        let store = state.inner.lock().unwrap();
        let h = store
            .get(&id)
            .ok_or_else(|| format!("Session {id} inconnue"))?;
        (h.enregistreur.clone(), h.label.clone())
    };
    let mut slot = enregistreur.lock().unwrap();
    if let Some(en_cours) = slot.as_ref() {
        return Ok(en_cours.chemin().display().to_string());
    }
    let mut e = avash::enregistrement::Enregistreur::demarrer(&label, cols, rows)
        .map_err(|e| format!("{e:#}"))?;
    if let Some(ecran) = etat_initial.filter(|s| !s.is_empty()) {
        e.sortie(&ecran).map_err(|e| format!("{e:#}"))?;
    }
    let chemin = e.chemin().display().to_string();
    *slot = Some(e);
    Ok(chemin)
}

/// Les enregistrements existants, du plus récent au plus ancien.
#[must_use]
#[tauri::command]
pub fn enregistrements_lister() -> Vec<avash::enregistrement::Info> {
    avash::enregistrement::repertoire()
        .map(|d| avash::enregistrement::lister(&d))
        .unwrap_or_default()
}

/// Ouvre le répertoire des enregistrements dans le gestionnaire de fichiers,
/// en le créant s'il n'existe pas encore.
#[tauri::command]
pub fn enregistrements_ouvrir_dossier() -> Result<String, String> {
    let dir =
        avash::enregistrement::repertoire().ok_or("répertoire de configuration introuvable")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("{e:#}"))?;
    open::that(&dir).map_err(|e| format!("Ouverture impossible : {e}"))?;
    Ok(dir.display().to_string())
}

/// Arrête l'enregistrement et rend le chemin du fichier ; `None` s'il n'y en
/// avait pas.
#[tauri::command]
pub fn enregistrement_arreter(
    state: tauri::State<'_, SessionStore>,
    id: u64,
) -> Result<Option<String>, String> {
    let Some(enregistreur) = enregistreur_de(&state, id) else {
        return Err(format!("Session {id} inconnue"));
    };
    let pris = enregistreur.lock().unwrap().take();
    match pris {
        Some(e) => e
            .arreter()
            .map(|p| Some(p.display().to_string()))
            .map_err(|e| format!("{e:#}")),
        None => Ok(None),
    }
}

/// Le chemin de l'enregistrement en cours, s'il y en a un.
#[must_use]
#[tauri::command]
pub fn enregistrement_en_cours(state: tauri::State<'_, SessionStore>, id: u64) -> Option<String> {
    enregistreur_de(&state, id).and_then(|e| {
        e.lock()
            .unwrap()
            .as_ref()
            .map(|x| x.chemin().display().to_string())
    })
}

/// Ferme une session (fermeture d'onglet). Coupe aussi la session SFTP liée.
#[tauri::command]
pub async fn pty_close(state: tauri::State<'_, SessionStore>, id: u64) -> Result<(), String> {
    // Retrait et note d'annulation sous le même verrou, dans le même ordre que
    // `open_on_target` (inner puis annules) : sans cela les deux pouvaient
    // s'entrelacer et laisser une session vivante sans onglet.
    let handle = {
        let mut inner = state.inner.lock().unwrap();
        let h = inner.remove(&id);
        // On ne note l'annulation que si une connexion est RÉELLEMENT en cours.
        // Sans cette condition, fermer un onglet dont la connexion avait déjà
        // échoué semait un identifiant qui figeait, après rechargement de la
        // fenêtre, l'onglet qui en héritait.
        if h.is_none() && state.en_cours.lock().unwrap().contains(&id) {
            state.annules.lock().unwrap().insert(id);
        }
        h
    };
    if let Some(h) = handle {
        // into_inner() echoue si le mutex a ete empoisonne par un panic
        // ailleurs. Fermer un onglet ne doit jamais planter pour autant :
        // on recupere la valeur malgre l'empoisonnement.
        let sftp = h
            .sftp
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(s) = sftp {
            // Fermeture explicite si on détient la dernière référence.
            if let Ok(owned) = std::sync::Arc::try_unwrap(s) {
                let _ = owned.close().await;
            }
        }
    }
    Ok(())
}
