//! Avash GUI — coquille Tauri 2. Sessions PTY multi-onglets côté Rust.

mod commands;

pub use commands::*;

use std::collections::HashMap;
use std::sync::Mutex;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(commands::SessionStore {
            inner: Mutex::new(HashMap::new()),
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_hosts,
            commands::run_command,
            commands::pty_open,
            commands::pty_open_manual,
            commands::pty_write,
            commands::pty_resize,
            commands::pty_close,
            commands::sftp_list,
            commands::sftp_download,
            commands::sftp_upload,
            commands::host_save,
            commands::keys_list,
            commands::key_generate,
            commands::key_deploy
        ])
        .run(tauri::generate_context!())
        .expect("erreur au lancement d'Avash");
}
