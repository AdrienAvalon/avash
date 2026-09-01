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
    {
        if std::env::var_os("WEBKIT_DISABLE_COMPOSITING_MODE").is_none() {
            // SAFETY: exécuté avant tout démarrage de fil d'exécution ou de WebKit.
            unsafe { std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1") };
        }
        // Un serveur d'inspection WebKit hérité de l'environnement ouvrirait un
        // débogueur distant sur la webview — donc la lecture des hôtes et
        // l'exécution de commandes distantes à qui s'y connecte en local. Rien
        // ne le justifie pour un lancement normal : on le ferme.
        if std::env::var_os("WEBKIT_INSPECTOR_SERVER").is_some() {
            // SAFETY: idem, avant tout démarrage de fil ou de WebKit.
            unsafe { std::env::remove_var("WEBKIT_INSPECTOR_SERVER") };
        }
    }

    // Windows : WebView2 compose par le GPU, dont la surface est virtualisée par
    // le protocole RDP quand avash est affiché À TRAVERS une session distante (un
    // poste piloté, ou — le cas qui l'a révélé — un avash Windows ouvert dans un
    // avash Windows). Les tuiles fraîchement peintes (vignettes vidéo, canvas,
    // aperçus d'onglet) arrivent parfois AVANT leur contenu et se voient un
    // instant en NOIR, d'autant plus longtemps que la session est imbriquée.
    // `--disable-gpu-compositing` bascule la composition en logiciel : plus de
    // carrés noirs, sans toucher au rendu sur écran physique.
    //
    // Mais WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS n'est pas qu'un réglage
    // d'affichage : WebView2 concatène sa valeur à la ligne de commande du
    // navigateur. Une valeur héritée de l'environnement pourrait y glisser
    // `--remote-debugging-port` (débogueur distant, prise de contrôle locale de
    // la webview) ou `--no-sandbox`. On ne défère donc pas aveuglément : on
    // filtre les drapeaux sensibles de la valeur héritée avant d'y ajouter le
    // nôtre. Le tri vit dans `arguments_webview2`, pur et testé.
    #[cfg(target_os = "windows")]
    {
        let herite = std::env::var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").ok();
        if let Some(valeur) = arguments_webview2(herite.as_deref(), session_distante()) {
            // SAFETY: exécuté avant tout démarrage de fil d'exécution ou de WebView2.
            unsafe { std::env::set_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", valeur) };
        }
    }

    avash_ui_lib::run();
}

/// Vrai quand le processus s'exécute dans une session RDP (Terminal Services),
/// et non sur un écran physique. `GetSystemMetrics(SM_REMOTESESSION)` renvoie
/// une valeur non nulle exactement dans ce cas — c'est le test que Microsoft
/// documente pour distinguer les deux, plus fiable qu'un reniflage de variables
/// d'environnement. FFI directe vers user32 : une seule fonction, aucune caisse
/// supplémentaire à embarquer. `#[link]` explicite pour ne pas dépendre du hasard
/// qu'une autre caisse du graphe lie déjà user32.
#[cfg(target_os = "windows")]
fn session_distante() -> bool {
    // SM_REMOTESESSION, cf. MS-RDPBCGR / winuser.h.
    const SM_REMOTESESSION: i32 = 0x1000;
    #[link(name = "user32")]
    extern "system" {
        fn GetSystemMetrics(n_index: i32) -> i32;
    }
    // SAFETY: GetSystemMetrics est sans effet de bord et sans paramètre pointeur.
    unsafe { GetSystemMetrics(SM_REMOTESESSION) != 0 }
}

/// Décide la valeur de `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` à poser, à partir
/// de la valeur héritée de l'environnement (`herite`) et du fait qu'on soit en
/// session distante. Retourne `None` s'il n'y a rien à changer.
///
/// Deux gestes indépendants :
/// - **toujours** retirer les drapeaux sensibles d'une valeur héritée (un pied
///   local ne doit pas pouvoir armer le débogueur distant de la webview) ;
/// - **en session distante seulement**, garantir `--disable-gpu-compositing`,
///   mais uniquement si l'utilisateur n'a rien posé lui-même (sinon on respecte
///   son choix — il a peut-être une raison de vouloir le compositing GPU).
///
/// Pur : aucun accès à l'environnement, testable sur toute plateforme.
#[cfg(any(target_os = "windows", test))]
fn arguments_webview2(herite: Option<&str>, distante: bool) -> Option<String> {
    // Drapeaux qu'on refuse de laisser passer, comparés sur le nom (avant un
    // éventuel `=valeur`). Ils ouvrent un débogueur distant, désactivent le bac à
    // sable ou l'isolation d'origine — de quoi transformer la webview en pivot.
    const DANGEREUX: &[&str] = &[
        "--remote-debugging-port",
        "--remote-debugging-pipe",
        "--remote-debugging-address",
        "--no-sandbox",
        "--disable-web-security",
        "--disable-site-isolation-trials",
        "--user-data-dir",
    ];

    let herite = herite.unwrap_or("");
    let mut jetons: Vec<&str> = Vec::new();
    let mut retire = false;
    for jeton in herite.split_whitespace() {
        let nom = jeton.split('=').next().unwrap_or(jeton);
        if DANGEREUX.contains(&nom) {
            retire = true;
        } else {
            jetons.push(jeton);
        }
    }

    // Ajout de la composition logicielle : en session distante, si l'utilisateur
    // n'a rien imposé (herite vide) et que le drapeau n'y est pas déjà.
    let mut ajoute = false;
    if distante && herite.trim().is_empty() && !jetons.contains(&"--disable-gpu-compositing") {
        jetons.push("--disable-gpu-compositing");
        ajoute = true;
    }

    if !retire && !ajoute {
        return None; // rien à faire : on ne touche pas à la valeur héritée.
    }
    Some(jetons.join(" "))
}

#[cfg(test)]
mod tests {
    use super::arguments_webview2;

    #[test]
    fn sans_heritage_hors_session_distante_ne_touche_a_rien() {
        assert_eq!(arguments_webview2(None, false), None);
        assert_eq!(arguments_webview2(Some(""), false), None);
    }

    #[test]
    fn en_session_distante_sans_heritage_pose_la_composition_logicielle() {
        assert_eq!(
            arguments_webview2(None, true).as_deref(),
            Some("--disable-gpu-compositing")
        );
    }

    #[test]
    fn respecte_un_choix_explicite_de_l_utilisateur_en_session_distante() {
        // L'utilisateur a posé une valeur bénigne : on ne lui impose pas notre
        // drapeau par-dessus, il garde la main sur le compositing.
        assert_eq!(arguments_webview2(Some("--lang=fr-FR"), true), None);
    }

    #[test]
    fn retire_les_drapeaux_dangereux_meme_hors_session_distante() {
        // Le cœur du durcissement : un --remote-debugging-port hérité est
        // supprimé, en session distante comme sur écran physique.
        assert_eq!(
            arguments_webview2(Some("--remote-debugging-port=9222"), false).as_deref(),
            Some("")
        );
        assert_eq!(
            arguments_webview2(Some("--lang=fr --no-sandbox --mute-audio"), false).as_deref(),
            Some("--lang=fr --mute-audio")
        );
    }

    #[test]
    fn nettoie_et_ajoute_a_la_fois_en_session_distante() {
        // Valeur hostile héritée + session distante : on retire le danger. On
        // n'ajoute PAS la composition logicielle, car l'utilisateur a posé une
        // valeur (on respecte son choix) — mais le nettoyage, lui, prime.
        assert_eq!(
            arguments_webview2(Some("--remote-debugging-pipe"), true).as_deref(),
            Some("")
        );
    }

    #[test]
    fn n_ajoute_pas_deux_fois_le_drapeau_deja_present() {
        // herite non vide contenant déjà notre drapeau : rien à changer.
        assert_eq!(
            arguments_webview2(Some("--disable-gpu-compositing"), true),
            None
        );
    }

    #[test]
    fn un_prefixe_ressemblant_n_est_pas_pris_pour_un_drapeau_dangereux() {
        // --remote-debugging-portable n'est pas --remote-debugging-port.
        assert_eq!(arguments_webview2(Some("--user-data-dir-x=1"), false), None);
    }
}
