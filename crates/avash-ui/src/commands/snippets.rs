//! Snippets : définitions et envoi.

use super::SessionStore;
use avash::snippet::Snippet;

/// Une session ouverte, telle que le sélecteur de cibles l'affiche.
#[derive(serde::Serialize)]
pub struct SessionInfo {
    pub id: u64,
    pub label: String,
}

/// Sessions actuellement ouvertes (pour choisir les cibles d'un envoi).
#[tauri::command]
#[must_use]
pub fn open_sessions(state: tauri::State<'_, SessionStore>) -> Vec<SessionInfo> {
    state
        .inner
        .lock()
        .unwrap()
        .iter()
        .map(|(id, h)| SessionInfo {
            id: *id,
            label: h.label.clone(),
        })
        .collect()
}

#[tauri::command]
pub fn snippet_list() -> Result<Vec<Snippet>, String> {
    avash::snippet::load_snippets().map_err(|e| e.to_string())
}

/// Variables `{{nom}}` d'une commande, dans l'ordre — pour demander leur
/// valeur avant l'envoi.
#[tauri::command]
#[must_use]
pub fn snippet_vars(command: String) -> Vec<String> {
    avash::snippet::extract_vars(&command)
}

/// Cree (`id` absent) ou modifie un snippet.
#[tauri::command]
pub fn snippet_save(
    id: Option<String>,
    name: String,
    command: String,
    run: bool,
    category: Option<String>,
) -> Result<Snippet, String> {
    let mut snip = Snippet::new(&name, &command, run, category.as_deref().unwrap_or(""));
    if let Some(id) = id.filter(|i| !i.is_empty()) {
        snip.id = id;
    }
    avash::snippet::upsert_snippet_in(&avash::snippet::snippets_path(), snip.clone())
        .map_err(|e| e.to_string())?;
    Ok(snip)
}

#[tauri::command]
pub fn snippet_delete(id: String) -> Result<(), String> {
    avash::snippet::remove_snippet_in(&avash::snippet::snippets_path(), &id)
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Envoie une commande (variables deja substituees cote front) a une ou
/// plusieurs sessions. Rend le nombre de sessions atteintes.
///
/// Une session fermee entre-temps est ignoree ; une erreur n'interrompt pas
/// l'envoi aux autres — la multi-execution ne doit pas s'arreter au premier
/// serveur muet.
#[tauri::command]
pub async fn snippet_send(
    state: tauri::State<'_, SessionStore>,
    session_ids: Vec<u64>,
    command: String,
    run: bool,
) -> Result<usize, String> {
    let payload = avash::snippet::terminal_payload(&command, run).into_bytes();
    let senders: Vec<_> = {
        let store = state.inner.lock().unwrap();
        session_ids
            .iter()
            .filter_map(|id| store.get(id).map(|h| h.input.clone()))
            .collect()
    };
    let mut sent = 0;
    for tx in senders {
        if tx.send(payload.clone()).await.is_ok() {
            sent += 1;
        }
    }
    Ok(sent)
}
