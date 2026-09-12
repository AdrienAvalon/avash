//! Enregistrement de session (asciicast) : démarrer, arrêter, lister.

use super::{finaliser_enregistrement, Enregistrement, SessionStore};
use avash::Verrou as _;
use tauri::AppHandle;

pub(crate) fn enregistreur_de(
    state: &tauri::State<'_, SessionStore>,
    id: u64,
) -> Option<Enregistrement> {
    state
        .inner
        .verrou()
        .get(&id)
        .map(|h| h.enregistreur.clone())
}

/// Démarre l'enregistrement de la session dans un fichier asciicast v2, et
/// rend son chemin. Seule la sortie est enregistrée, jamais les frappes.
///
/// `etat_initial` est l'écran tel qu'il est au moment de démarrer, sérialisé
/// par le front (séquences d'échappement comprises) : sans lui, un
/// enregistrement lancé en cours de session rejouait à partir d'un écran noir.
#[tauri::command(async)]
pub fn enregistrement_demarrer(
    state: tauri::State<'_, SessionStore>,
    id: u64,
    cols: u32,
    rows: u32,
    etat_initial: Option<String>,
) -> Result<String, String> {
    let (enregistreur, label) = {
        let store = state.inner.verrou();
        let h = store
            .get(&id)
            .ok_or_else(|| format!("Session {id} inconnue"))?;
        (h.enregistreur.clone(), h.label.clone())
    };
    let mut slot = enregistreur.verrou();
    if let Some(en_cours) = slot.as_ref() {
        return Ok(en_cours.chemin().display().to_string());
    }
    let mut e = avash::enregistrement::Enregistreur::demarrer(&label, cols, rows)
        .map_err(|e| format!("{e:#}"))?;
    // Le démarrage se voit tout de suite sur le disque : l'en-tête et, s'il y a
    // lieu, l'état initial de l'écran. Depuis que l'enregistreur ne vide plus
    // son tampon à chaque ligne mais au rythme des messages du terminal
    // (contrat K4, audit du 12 septembre 2026), rien n'était écrit avant la
    // première sortie : un enregistrement lancé sur un écran calme restait
    // vide sur le disque (régression vue par la suite bout en bout,
    // enregistrement.spec.js, le jour même).
    let premiere = etat_initial
        .filter(|s| !s.is_empty())
        .map_or(Ok(()), |ecran| {
            e.sortie(&ecran).map_err(|err| format!("{err:#}"))
        })
        .and_then(|()| e.vider().map_err(|err| err.to_string()));
    if let Err(err) = premiere {
        // Trouvé par l'audit du 7 septembre 2026 : si la toute première
        // écriture échoue (disque plein), le fichier déjà créé par
        // `create_new` resterait sur le disque, vide ou réduit à
        // l'en-tête, et s'afficherait dans la liste comme un enregistrement
        // valide. On le retire avant de remonter l'erreur.
        let _ = std::fs::remove_file(e.chemin());
        return Err(err);
    }
    let chemin = e.chemin().display().to_string();
    *slot = Some(e);
    Ok(chemin)
}

/// Les enregistrements existants, du plus récent au plus ancien.
#[must_use]
#[tauri::command(async)]
pub fn enregistrements_lister() -> Vec<avash::enregistrement::Info> {
    avash::enregistrement::repertoire()
        .map(|d| avash::enregistrement::lister(&d))
        .unwrap_or_default()
}

/// Ouvre le répertoire des enregistrements dans le gestionnaire de fichiers,
/// en le créant s'il n'existe pas encore.
///
/// `open::that` attend le lanceur du système : hors du fil principal (audit
/// du 12 septembre 2026, C-SIL-7).
#[tauri::command]
pub async fn enregistrements_ouvrir_dossier() -> Result<String, String> {
    super::bloquant(|| {
        let dir =
            avash::enregistrement::repertoire().ok_or("répertoire de configuration introuvable")?;
        std::fs::create_dir_all(&dir).map_err(|e| format!("{e:#}"))?;
        open::that(&dir).map_err(|e| format!("Ouverture impossible : {e}"))?;
        Ok(dir.display().to_string())
    })
    .await
}

/// Arrête l'enregistrement et rend le chemin du fichier ; `None` s'il n'y en
/// avait pas.
#[tauri::command(async)]
pub fn enregistrement_arreter(
    state: tauri::State<'_, SessionStore>,
    id: u64,
) -> Result<Option<String>, String> {
    let Some(enregistreur) = enregistreur_de(&state, id) else {
        return Err(format!("Session {id} inconnue"));
    };
    let pris = enregistreur.verrou().take();
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
        e.verrou()
            .as_ref()
            .map(|x| x.chemin().display().to_string())
    })
}

/// Ferme une session (fermeture d'onglet). Coupe aussi la session SFTP liée.
#[tauri::command]
pub async fn pty_close<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, SessionStore>,
    id: u64,
) -> Result<(), String> {
    // Retrait et note d'annulation sous le même verrou, dans le même ordre que
    // `open_on_target` (inner puis annules) : sans cela les deux pouvaient
    // s'entrelacer et laisser une session vivante sans onglet.
    let handle = {
        let mut inner = state.inner.verrou();
        let h = inner.remove(&id);
        // On ne note l'annulation que si une connexion est RÉELLEMENT en cours.
        // Sans cette condition, fermer un onglet dont la connexion avait déjà
        // échoué semait un identifiant qui figeait, après rechargement de la
        // fenêtre, l'onglet qui en héritait.
        if h.is_none() && state.en_cours.verrou().contains(&id) {
            state.annules.verrou().insert(id);
        }
        h
    };
    if let Some(h) = handle {
        // Fermer l'onglet ferme le fichier : on arrête explicitement
        // l'enregistrement pour récupérer une erreur de vidage éventuelle,
        // plutôt que de laisser le `Drop` du `BufWriter` l'avaler.
        finaliser_enregistrement(&app, id, &h.enregistreur);
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
