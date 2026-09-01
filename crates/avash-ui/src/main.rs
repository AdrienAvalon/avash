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

    // Même mesure côté Windows, mais bornée à un cas précis : avash affiché À
    // TRAVERS une session RDP (un poste piloté à distance, ou — le cas qui l'a
    // révélé — un avash Windows ouvert dans un avash Windows). WebView2 y compose
    // par le GPU, dont la surface est virtualisée par le protocole RDP : les
    // tuiles fraîchement peintes par le GPU (vignettes vidéo, canvas, aperçus
    // d'onglet) arrivent parfois AVANT que leur contenu ne soit poussé, et se
    // voient un instant en NOIR. Le pipeline classique les transmet fidèlement,
    // et la double latence d'un RDP imbriqué les fait durer assez pour qu'on les
    // remarque. `--disable-gpu-compositing` bascule la composition en logiciel :
    // plus de surface GPU virtualisée, donc plus de carrés noirs — sans toucher
    // au rendu local (SM_REMOTESESSION est faux sur un écran physique).
    //
    // On n'écrase pas une valeur déjà posée : qui veut trancher à la main garde
    // la main. WebView2 ajoute cette variable à ses arguments par défaut.
    #[cfg(target_os = "windows")]
    if session_distante() && std::env::var_os("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").is_none() {
        // SAFETY: exécuté avant tout démarrage de fil d'exécution ou de WebView2.
        unsafe {
            std::env::set_var(
                "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
                "--disable-gpu-compositing",
            )
        };
    }

    avash_ui_lib::run();
}

/// Vrai quand le processus s'exécute dans une session RDP (Terminal Services),
/// et non sur un écran physique. `GetSystemMetrics(SM_REMOTESESSION)` renvoie
/// une valeur non nulle exactement dans ce cas — c'est le test que Microsoft
/// documente pour distinguer les deux, plus fiable qu'un reniflage de variables
/// d'environnement. FFI directe vers user32 : une seule fonction, aucune caisse
/// supplémentaire à embarquer.
#[cfg(target_os = "windows")]
fn session_distante() -> bool {
    // SM_REMOTESESSION, cf. MS-RDPBCGR / winuser.h.
    const SM_REMOTESESSION: i32 = 0x1000;
    extern "system" {
        fn GetSystemMetrics(n_index: i32) -> i32;
    }
    // SAFETY: GetSystemMetrics est sans effet de bord et sans paramètre pointeur.
    unsafe { GetSystemMetrics(SM_REMOTESESSION) != 0 }
}
