#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Redimensionnement de la fenêtre : WebKitGTK compose les couches sur le
    // GPU et réalloue ses tampons vidéo à CHAQUE image du geste. Mesuré au
    // profileur sur une machine AMD : 42 % du temps passait dans le noyau
    // (ttm_bo_alloc_resource, ttm_bo_evict, drm_gem_handle_delete), avec un
    // ralentissement très net à l'usage. Sans compositing accéléré, cette part
    // tombe à 19 % et le geste redevient fluide — pour un débit RDP inchangé
    // (10 images/s contre 9, dans le bruit).
    //
    // La variable reste surchargeable : qui veut retrouver le compositing la
    // définit à 0 avant de lancer avash.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_COMPOSITING_MODE").is_none() {
        // SAFETY: exécuté avant tout démarrage de fil d'exécution ou de WebKit.
        unsafe { std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1") };
    }

    avash_ui_lib::run();
}
