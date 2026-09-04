//! Sessions série : le port du poste dans un onglet de terminal.
//!
//! Une session série est une session comme les autres pour le front et pour
//! le magasin : mêmes canaux clavier et sortie, même pump vers `pty-output`,
//! mêmes `pty_write` et `pty_close`. Elle n'a ni SFTP ni commande à distance ;
//! ce qu'on lui demande d'autre répond clairement qu'un port série ne le fait
//! pas.

use super::sessions::{
    clore_session, enregistrer_session, relayer_sortie, Enregistrement, SessionHandle,
    SessionStore, SESSION_EPOCH,
};
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use tauri::AppHandle;

/// Les ports série du poste.
#[tauri::command]
#[must_use]
pub fn serie_ports() -> Vec<avash::serie::PortSerie> {
    avash::serie::lister_ports()
}

/// Ouvre un port série dans l'onglet `id` et rend le libellé de l'onglet.
#[tauri::command]
pub async fn serie_open(
    app: AppHandle,
    state: tauri::State<'_, SessionStore>,
    id: u64,
    chemin: String,
    vitesse: u32,
) -> Result<String, String> {
    state.en_cours.lock().unwrap().insert(id);
    let session = match avash::serie::ouvrir(&chemin, vitesse) {
        Ok(s) => s,
        Err(e) => {
            state.en_cours.lock().unwrap().remove(&id);
            state.annules.lock().unwrap().remove(&id);
            return Err(e.to_string());
        }
    };
    let label = session.label.clone();
    // Un port série n'a pas de taille de fenêtre : le canal reçoit et ignore.
    let (resize, _resize_rx) = tokio::sync::mpsc::channel::<(u32, u32)>(1);
    let epoch = SESSION_EPOCH.fetch_add(1, Ordering::Relaxed);
    let enregistreur: Enregistrement = std::sync::Arc::new(Mutex::new(None));
    enregistrer_session(
        &state,
        id,
        SessionHandle {
            epoch,
            input: session.in_tx,
            resize,
            sftp: Mutex::new(None),
            ouvrir_sftp: std::sync::Arc::new(|| {
                Box::pin(async { Err("Pas de SFTP sur un port série.".to_owned()) })
            }),
            executer: std::sync::Arc::new(|_| {
                Box::pin(async {
                    Err("Pas de commande à distance sur un port série.".to_owned())
                })
            }),
            label: label.clone(),
            // Aucune adresse : un autre hôte ne peut pas joindre un port série.
            cible: (String::new(), 0, String::new()),
            enregistreur: enregistreur.clone(),
        },
    )?;
    let out_rx = session.out_rx;
    let app2 = app.clone();
    tokio::spawn(async move {
        relayer_sortie(&app2, id, out_rx, enregistreur).await;
        clore_session(&app2, id, epoch);
    });
    Ok(label)
}
