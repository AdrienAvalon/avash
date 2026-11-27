//! Avash GUI — coquille Tauri 2. Sessions PTY multi-onglets côté Rust.

pub mod commands;
#[cfg(windows)]
mod fenetre_windows;
pub mod journal;
pub mod langue;
pub mod rdp;

pub use commands::*;

/// Un fichier déposé sur la fenêtre est un geste de l'utilisateur que le natif
/// voit avant la webview : c'est ici qu'il est retenu comme chemin désigné
/// (voir `commands::choix_locaux`), pas sur la foi du front.
fn retenir_un_depot<R: tauri::Runtime>(fenetre: &tauri::Window<R>, evenement: &tauri::WindowEvent) {
    if let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = evenement {
        use tauri::Manager as _;
        let app = fenetre.app_handle().clone();
        let chemins = paths.clone();
        tauri::async_runtime::spawn(async move {
            commands::designer(
                &app.state::<commands::ChoixLocaux>(),
                &app.state::<rdp::RdpStore>(),
                chemins,
            )
            .await;
        });
    }
}

/// Dit pourquoi l'application ne s'est pas lancée, et rend le code de sortie.
///
/// Trouvé par l'audit du 12 septembre 2026 (C-panique-9) : `.run(..).expect(..)`
/// paniquait sur stderr, que Windows ne montre pas en release
/// (`windows_subsystem = "windows"`). `WebView2` absente ou `WebKitGTK` cassé,
/// l'application ne s'ouvrait pas, sans un mot. Le message part sur `sortie`
/// (stderr) et au journal ; sous Windows, `run` ouvre en plus une boîte native.
pub(crate) fn rapporter_echec_lancement(
    erreur: &dyn std::fmt::Display,
    sortie: &mut dyn std::io::Write,
) -> (String, i32) {
    let message = format!("Avash n'a pas pu démarrer : {erreur}");
    tracing::error!("{message}");
    let _ = writeln!(sortie, "{message}");
    (message, 1)
}

/// Boîte de message native pour un échec de lancement : sans console, c'est le
/// seul endroit où l'utilisateur Windows peut lire la cause.
#[cfg(windows)]
fn boite_echec_lancement(message: &str) {
    const MB_ICONERROR: u32 = 0x0000_0010;
    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(
            hwnd: *mut std::ffi::c_void,
            texte: *const u16,
            titre: *const u16,
            genre: u32,
        ) -> i32;
    }
    let large = |s: &str| {
        s.encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<u16>>()
    };
    let (texte, titre) = (large(message), large("Avash"));
    // SAFETY: `texte` et `titre` sont des tampons UTF-16 terminés par un zéro,
    // vivants pendant tout l'appel (liés au-dessus) ; une fenêtre parente nulle
    // est admise par l'API, qui ne conserve aucun des pointeurs.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            texte.as_ptr(),
            titre.as_ptr(),
            MB_ICONERROR,
        );
    }
}

/// Les états partagés par les commandes : magasins des sessions, tunnels,
/// bureaux, transferts, chemins désignés, accusés du terminal (contrat K6),
/// signalement du trousseau (K1) et moteur de rendu (K8).
fn gerer_les_etats<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder
        .manage(commands::SessionStore::default())
        .manage(commands::TunnelStore::default())
        .manage(rdp::RdpStore::default())
        .manage(commands::TransfertsStore::default())
        .manage(commands::ChoixLocaux::default())
        .manage(commands::Accuses::default())
        .manage(commands::TrousseauSignale::default())
        .manage(commands::RenduTerminal::default())
}

/// Au début de chaque chargement de la fenêtre principale, le cœur ferme ce
/// que possédait la page précédente (audit du 27 novembre 2026, WI-1, FI-2,
/// ME-3) : une page neuve n'a aucun onglet, donc aucune session. Le premier
/// chargement trouve les magasins vides.
fn purger_au_chargement<R: tauri::Runtime>(
    vue: &tauri::Webview<R>,
    charge: &tauri::webview::PageLoadPayload<'_>,
) {
    use tauri::Manager as _;
    if charge.event() == tauri::webview::PageLoadEvent::Started && vue.label() == "main" {
        commands::purger_la_page(vue.app_handle());
    }
}

/// Les greffons de Tauri dont l'application se sert.
fn greffons() -> tauri::Builder<tauri::Wry> {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_clipboard_manager::init())
}

/// Les mots de passe d'Avash rangés en persistance « Enterprise » (itinérante)
/// passent en « Local Machine », hors du fil de l'interface : le Gestionnaire
/// d'identifiants n'est pas instantané. Voir `avash::secrets`.
fn migrer_le_trousseau() {
    #[cfg(windows)]
    std::thread::spawn(
        || match avash::secrets::migrer_vers_la_persistance_locale() {
            Ok(0) => {}
            Ok(n) => tracing::info!("{n} mot(s) de passe du trousseau rendu(s) locaux au poste"),
            Err(e) => tracing::warn!("trousseau : passage en persistance locale incomplet : {e:#}"),
        },
    );
}

/// Les données du poste quittent le profil itinérant de Windows, une fois
/// (audit du 27 novembre 2026, SE-5, WI-14 et WI-11), puis le journal de
/// l'application s'installe. Le déplacement passe avant le journal, qui
/// s'ouvre dans l'un des répertoires déplacés ; son bilan y est écrit juste
/// après. Ailleurs que sous Windows, les deux répertoires coïncident : rien.
fn demarrer_journal_et_migrations() {
    let migration = avash::donnees_locales::migrer_vers_le_local();
    // Le journal d'abord : tout ce qui suit peut avoir à y écrire (C-SIL-2).
    journal::installer();
    match migration {
        Ok(bilan) if bilan != avash::donnees_locales::Bilan::default() => {
            tracing::info!(?bilan, "données déplacées hors du profil itinérant");
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("déplacement des données hors du profil itinérant : {e:#}"),
    }
    migrer_le_trousseau();
}

/// Réglages posés une fois l'application construite : raccourcis de
/// navigateur et reprise du moteur de rendu, que Tauri n'expose pas (voir
/// `fenetre_windows`), et veille du type de session (WI-10).
#[allow(clippy::unnecessary_wraps)] // signature imposée par `Builder::setup`
fn preparer(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(windows)]
    {
        use tauri::Manager as _;
        if let Some(fenetre) = app.get_webview_window("main") {
            fenetre_windows::regler(&fenetre);
        }
    }
    #[cfg(not(windows))]
    let _ = app;
    Ok(())
}

/// Les crochets de la fenêtre : dépôt de fichiers, chargement de la page,
/// préparation une fois l'application construite.
fn crochets(builder: tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry> {
    builder
        .on_window_event(retenir_un_depot)
        .on_page_load(purger_au_chargement)
        .setup(preparer)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    demarrer_journal_et_migrations();
    let builder = crochets(gerer_les_etats(greffons()))
        // Quatre commandes ont été retirées de cette liste : `run_command`,
        // `snippet_vars`, `password_known`, puis `enregistrement_en_cours`
        // (audit du 9 septembre 2026), qu'aucun appel du front n'utilisait.
        // `run_command` était la plus fâcheuse : elle exécute une commande
        // arbitraire sur n'importe quel alias, avec le mot de passe du
        // trousseau chargé automatiquement ; `enregistrement_en_cours` livrait
        // le chemin absolu du fichier d'enregistrement de n'importe quel onglet
        // à tout script de la webview. Une commande enregistrée est une surface
        // offerte à la webview ; celle qui ne sert pas ne s'enregistre pas.
        // Elles restent publiques dans le crate, donc testées, et la règle est
        // désormais tenue par un contrôle
        // (`scripts/tests/ipc-commandes-appelees-par-le-front.sh`) : la
        // quatrième avait été oubliée parce que rien ne comparait la liste aux
        // appels du front.
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
            commands::canal_de_mise_a_jour,
            commands::open_external,
            commands::keyboard_locks,
            commands::pty_open,
            commands::host_needs_password,
            commands::pty_open_manual,
            commands::pty_write,
            commands::pty_ack,
            commands::pty_resize,
            commands::session_securite,
            commands::pty_close,
            commands::sftp_realpath,
            commands::sftp_list,
            commands::sftp_download,
            commands::sftp_upload,
            commands::choisir_fichiers_locaux,
            commands::sftp_annuler,
            commands::sftp_copier_vers,
            commands::sftp_mkdir,
            commands::sftp_remove,
            commands::sftp_rename,
            commands::host_save,
            commands::import_scan,
            commands::import_apply,
            commands::enregistrement_demarrer,
            commands::enregistrement_arreter,
            commands::enregistrements_lister,
            commands::enregistrements_ouvrir_dossier,
            commands::journal_noter,
            commands::journal_noter_code,
            commands::journal_lister,
            commands::journal_purger,
            commands::journal_exporter,
            commands::journal_ouvrir_dossier,
            commands::hosts_health,
            commands::diagnostic_exporter,
            commands::diagnostic_noter_rendu,
            commands::terminal_webgl_permis,
            commands::onglets_memoriser,
            commands::onglets_memorises,
            commands::serie_ports,
            commands::serie_open,
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
            rdp::rdp_host_set_tls_herite,
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
        .unwrap_or_else(|e| {
            let (message, code) = rapporter_echec_lancement(&e, &mut std::io::stderr());
            #[cfg(windows)]
            boite_echec_lancement(&message);
            #[cfg(not(windows))]
            let _ = message;
            std::process::exit(code);
        });
}

#[cfg(test)]
mod tests_lancement {
    use super::rapporter_echec_lancement;

    /// Audit du 12 septembre 2026 (C-panique-9) : un échec de lancement se dit
    /// en clair et rend le code 1, au lieu d'une panique invisible sous Windows.
    #[test]
    fn un_echec_de_lancement_se_dit_sans_panique() {
        let mut sortie = Vec::new();
        let (message, code) = rapporter_echec_lancement(&"WebView2 absente", &mut sortie);
        assert_eq!(code, 1);
        assert!(message.contains("WebView2 absente"), "{message}");
        assert_eq!(String::from_utf8(sortie).unwrap(), format!("{message}\n"));
    }
}
