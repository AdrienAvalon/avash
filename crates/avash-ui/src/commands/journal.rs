//! Journal des commandes passées dans les sessions SSH : noter, relire,
//! purger, exporter (voir `avash::journal_commandes`).
//!
//! Le front relève la ligne de commande à l'écran et décide si le journal est
//! actif pour l'onglet ; le cœur ajoute ce que la page n'a pas à fournir
//! (l'heure, l'identité de la session, sa cible) et masque les secrets avant
//! d'écrire. Une console série n'est jamais journalisée : le journal est celui
//! des commandes passées en SSH.

use super::SessionStore;
use avash::journal_commandes as journal;
use avash::Verrou as _;

fn repertoire() -> Result<std::path::PathBuf, String> {
    journal::repertoire().ok_or_else(|| "répertoire de configuration introuvable".to_owned())
}

/// Ce que le journal retient d'une session : son identité, son libellé et où
/// elle est connectée.
fn identite(
    state: &tauri::State<'_, SessionStore>,
    id: u64,
) -> Result<(u64, String, String), String> {
    let store = state.inner.verrou();
    let h = store
        .get(&id)
        .ok_or_else(|| format!("Session {id} inconnue"))?;
    if h.securite.is_none() {
        return Err("Le journal des commandes ne vaut que pour une session SSH.".to_owned());
    }
    let (adresse, port, utilisateur) = &h.cible;
    Ok((
        h.epoch,
        h.label.clone(),
        format!("{utilisateur}@{adresse}:{port}"),
    ))
}

/// Note une commande passée dans l'onglet `id`. Rend `false` quand il n'y
/// avait rien à noter (ligne vide, ou qui commence par une espace).
#[tauri::command(async)]
pub fn journal_noter(
    state: tauri::State<'_, SessionStore>,
    id: u64,
    commande: String,
) -> Result<bool, String> {
    let (s, session, cible) = identite(&state, id)?;
    journal::noter_dans(
        &repertoire()?,
        journal::maintenant(),
        s,
        &session,
        &cible,
        &commande,
    )
    .map_err(|e| format!("{e:#}"))
}

/// Note le code de retour de la dernière commande de l'onglet `id`, quand le
/// shell distant le signale (OSC 133).
#[tauri::command(async)]
pub fn journal_noter_code(
    state: tauri::State<'_, SessionStore>,
    id: u64,
    code: i32,
) -> Result<(), String> {
    let (s, _, _) = identite(&state, id)?;
    journal::noter_code_dans(&repertoire()?, journal::maintenant(), s, code)
        .map_err(|e| format!("{e:#}"))
}

/// Les commandes du journal, de la plus récente à la plus ancienne.
#[must_use]
#[tauri::command(async)]
pub fn journal_lister() -> Vec<journal::Entree> {
    journal::repertoire().map_or_default(|d| journal::lire_dans(&d))
}

/// Efface les jours plus vieux que `jours` ; rend le nombre de fichiers
/// effacés. Appelée au démarrage et quand la durée de conservation change.
#[tauri::command(async)]
pub fn journal_purger(jours: u32) -> Result<usize, String> {
    let Some(dir) = journal::repertoire() else {
        return Ok(0);
    };
    journal::purger_dans(&dir, jours, journal::maintenant()).map_err(|e| format!("{e:#}"))
}

/// Ouvre la boîte « Enregistrer sous » native, écrit le journal en texte à
/// l'endroit choisi et rend le chemin écrit ; `None` si l'utilisateur a
/// annulé. Comme pour le diagnostic (contrat K13), le chemin vient de la boîte
/// native, jamais de la page. `decalage_minutes` donne l'heure locale.
#[tauri::command]
pub async fn journal_exporter<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    decalage_minutes: i32,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt as _;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_file_name("avash-journal-des-commandes.txt")
        .add_filter("Texte", &["txt"])
        .save_file(move |c| {
            let _ = tx.send(c);
        });
    let choisi = rx
        .await
        .map_err(|_| "La boîte d'enregistrement s'est fermée sans répondre.".to_owned())?;
    let Some(chemin) = choisi.and_then(|f| f.into_path().ok()) else {
        return Ok(None);
    };
    super::bloquant(move || ecrire_export(&chemin, decalage_minutes))
        .await
        .map(Some)
}

/// Écrit l'export du journal à `chemin`, en 0600 et d'un seul tenant, et rend
/// le chemin écrit. Séparé de la boîte de dialogue pour être testé.
pub(crate) fn ecrire_export(
    chemin: &std::path::Path,
    decalage_minutes: i32,
) -> Result<String, String> {
    if !chemin.is_absolute() {
        return Err("Le chemin de l'export doit être absolu.".to_owned());
    }
    let entrees = journal::repertoire().map_or_default(|d| journal::lire_dans(&d));
    avash::ecrire_atomiquement(
        chemin,
        journal::en_texte(&entrees, decalage_minutes).as_bytes(),
    )
    .map_err(|e| e.to_string())?;
    Ok(chemin.display().to_string())
}

/// Ouvre le répertoire du journal dans le gestionnaire de fichiers, en le
/// créant (0700) s'il n'existe pas encore. `open::that` attend le lanceur du
/// système : hors du fil principal (C-SIL-7).
#[tauri::command]
pub async fn journal_ouvrir_dossier() -> Result<String, String> {
    super::bloquant(|| {
        let dir = repertoire()?;
        std::fs::create_dir_all(&dir).map_err(|e| format!("{e:#}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
        open::that(&dir).map_err(|e| format!("Ouverture impossible : {e}"))?;
        Ok(dir.display().to_string())
    })
    .await
}
