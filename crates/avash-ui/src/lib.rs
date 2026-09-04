//! Avash GUI — coquille Tauri 2. Sessions PTY multi-onglets côté Rust.

pub mod commands;
pub mod langue;
pub mod rdp;

pub use commands::*;

use std::collections::HashMap;
use std::sync::Mutex;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(commands::SessionStore {
            inner: Mutex::new(HashMap::new()),
            annules: Mutex::new(std::collections::HashSet::new()),
            en_cours: Mutex::new(std::collections::HashSet::new()),
        })
        .manage(commands::TunnelStore {
            inner: Mutex::new(HashMap::new()),
            en_cours: Mutex::new(std::collections::HashSet::new()),
            annules: Mutex::new(std::collections::HashSet::new()),
        })
        .manage(rdp::RdpStore::default())
        // Trois commandes ont été retirées de cette liste : `run_command`,
        // `snippet_vars` et `password_known`, qu'aucun appel du front
        // n'utilisait. `run_command` était la plus fâcheuse — elle exécute une
        // commande arbitraire sur n'importe quel alias, avec le mot de passe du
        // trousseau chargé automatiquement. Une commande enregistrée est une
        // surface offerte à la webview ; celle qui ne sert pas ne s'enregistre
        // pas. Elles restent publiques dans le crate, donc testées.
        .plugin(langue::plugin());
    // Serveur WebDriver embarqué : la suite bout en bout pilote l'application
    // par lui sous Windows (Edge WebDriver ne lance plus une application
    // WebView2 depuis sa version 133) et pourra le faire sous macOS. Compilé
    // seulement avec la fonctionnalité `webdriver`, que la publication ne pose
    // jamais : voir Cargo.toml.
    #[cfg(feature = "webdriver")]
    let builder = builder.plugin(tauri_plugin_wdio_webdriver::init());
    builder
        .invoke_handler(tauri::generate_handler![
            commands::list_hosts,
            commands::open_external,
            commands::keyboard_locks,
            commands::pty_open,
            commands::host_needs_password,
            commands::pty_open_manual,
            commands::pty_write,
            commands::pty_resize,
            commands::pty_close,
            commands::sftp_realpath,
            commands::sftp_list,
            commands::sftp_download,
            commands::sftp_upload,
            commands::sftp_mkdir,
            commands::sftp_remove,
            commands::sftp_rename,
            commands::host_save,
            commands::import_scan,
            commands::import_apply,
            commands::enregistrement_demarrer,
            commands::enregistrement_arreter,
            commands::enregistrement_en_cours,
            commands::enregistrements_lister,
            commands::enregistrements_ouvrir_dossier,
            commands::hosts_health,
            commands::host_delete,
            commands::host_get,
            commands::host_update,
            commands::folders_list,
            commands::folder_create,
            commands::folder_delete,
            commands::folder_rename,
            commands::host_set_folder,
            commands::password_save,
            commands::password_forget,
            commands::known_hosts_forget,
            commands::keys_list,
            commands::key_generate,
            commands::key_deploy,
            commands::tunnel_defs,
            commands::tunnel_def_save,
            commands::tunnel_def_delete,
            commands::tunnel_start,
            commands::tunnel_stop,
            commands::tunnel_status,
            rdp::rdp_open,
            rdp::rdp_close,
            rdp::rdp_ouvrir_dossier,
            rdp::rdp_hosts,
            rdp::rdp_host_save,
            rdp::rdp_host_delete,
            rdp::rdp_host_set_folder,
            rdp::rdp_password_save,
            rdp::rdp_diagnostic,
            rdp::rdp_host_set_sans_nla,
            rdp::rdp_password_known,
            rdp::rdp_password_move,
            rdp::rdp_password_forget,
            commands::open_sessions,
            commands::snippet_list,
            commands::snippet_save,
            commands::snippet_delete,
            commands::snippet_send
        ])
        .run(tauri::generate_context!())
        .expect("erreur au lancement d'Avash");
}
