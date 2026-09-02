#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Pilotage WebDriver (suite bout en bout) : tauri-driver le signale par
    // TAURI_WEBVIEW_AUTOMATION=true, que Tauri lit lui-même pour ouvrir
    // l'automatisation de la webview. Les deux durcissements ci-dessous en
    // dépendent : le canal par lequel le pilote natif commande l'application
    // est une variable d'environnement qu'ils retireraient sinon.
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    let automatisation = std::env::var_os("TAURI_WEBVIEW_AUTOMATION").is_some_and(|v| v == "true");

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
        //
        // SAUF sous pilotage WebDriver : WebKitWebDriver lance l'application avec
        // cette même variable, c'est SON canal de commande — la retirer coupait
        // la suite bout en bout (session jamais établie, « IncompleteMessage »
        // côté tauri-driver). tauri-driver signale ce mode par
        // TAURI_WEBVIEW_AUTOMATION=true, que Tauri lit lui-même pour ouvrir
        // l'automatisation : qui peut poser l'un peut poser l'autre, l'exception
        // n'ajoute donc aucune surface. Décision pure et testée dans
        // `retirer_inspecteur_webkit`.
        if retirer_inspecteur_webkit(
            std::env::var_os("WEBKIT_INSPECTOR_SERVER").is_some(),
            automatisation,
        ) {
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
    // navigateur, que Chromium re-découpe ensuite (guillemets retirés). Une
    // valeur héritée de l'environnement pourrait y glisser `--remote-debugging-port`
    // (débogueur distant, prise de contrôle locale de la webview), `--no-sandbox`,
    // ou pire `--renderer-cmd-prefix` (exécution d'un binaire arbitraire). La
    // filtrer par liste noire est illusoire : `--no-sand"box` passerait le tri et
    // serait reconstitué par Chromium. avash prend donc le CONTRÔLE TOTAL de la
    // variable — il n'en défère jamais la valeur héritée : il pose la sienne en
    // session distante, la retire sinon. La décision vit dans `action_webview2`,
    // pure et testée.
    //
    // SAUF sous pilotage WebDriver — même exception que pour WebKit : Edge
    // WebDriver lance une application WebView2 en posant précisément cette
    // variable (`--remote-debugging-port`, dossier de données), c'est SON canal
    // de commande. La retirer laissait chaque scénario mourir après quatre
    // minutes sur « DevToolsActivePort file doesn't exist » — le fichier que
    // le pilote attend n'était jamais écrit, faute de port. Qui peut poser
    // TAURI_WEBVIEW_AUTOMATION peut poser l'autre : aucune surface ajoutée.
    #[cfg(target_os = "windows")]
    {
        let herite = std::env::var_os("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").is_some();
        match action_webview2(herite, session_distante(), automatisation) {
            // SAFETY: exécuté avant tout démarrage de fil d'exécution ou de WebView2.
            ActionWebview2::Poser(v) => unsafe {
                std::env::set_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", v)
            },
            ActionWebview2::Retirer => unsafe {
                std::env::remove_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS")
            },
            ActionWebview2::NePasToucher => {}
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

/// Faut-il retirer `WEBKIT_INSPECTOR_SERVER` de l'environnement ? Oui dès
/// qu'une valeur est héritée — sauf sous pilotage `WebDriver`, où cette variable
/// est le canal par lequel `WebKitWebDriver` commande l'application.
///
/// Pur : aucun accès à l'environnement, testable sur toute plateforme.
#[cfg(any(target_os = "linux", test))]
fn retirer_inspecteur_webkit(herite: bool, automatisation: bool) -> bool {
    herite && !automatisation
}

/// Ce qu'il faut faire de `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`.
#[cfg(any(target_os = "windows", test))]
#[derive(Debug, PartialEq, Eq)]
enum ActionWebview2 {
    /// Poser exactement cette valeur (la nôtre).
    Poser(&'static str),
    /// Retirer la variable de l'environnement (valeur héritée non fiable).
    Retirer,
    /// Ne rien faire (aucune valeur héritée, hors session distante).
    NePasToucher,
}

/// Décide l'action à mener sur `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`, selon
/// qu'une valeur est déjà présente dans l'environnement (`herite`), qu'on soit
/// en session distante, et qu'on soit piloté par `WebDriver` (`automatisation`).
///
/// avash ne DÉFÈRE jamais la valeur héritée : filtrer une ligne de commande
/// Chromium par liste noire est illusoire (guillemets internes reconstitués,
/// drapeaux d'exécution innombrables). Donc — en session distante on impose la
/// nôtre (`--disable-gpu-compositing`), sinon on retire toute valeur héritée pour
/// qu'aucun `--remote-debugging-port` planté par un pied local n'atteigne la
/// webview. Le confort d'un réglage utilisateur bénin ne vaut pas cette surface.
///
/// Sous pilotage `WebDriver`, la variable EST le canal de commande du pilote
/// natif (Edge `WebDriver` la pose lui-même) : on n'y touche pas, session distante
/// ou non — un scénario joué à travers RDP a besoin de son port, pas de notre
/// composition logicielle.
///
/// Pur : aucun accès à l'environnement, testable sur toute plateforme.
#[cfg(any(target_os = "windows", test))]
fn action_webview2(herite: bool, distante: bool, automatisation: bool) -> ActionWebview2 {
    if automatisation {
        ActionWebview2::NePasToucher
    } else if distante {
        ActionWebview2::Poser("--disable-gpu-compositing")
    } else if herite {
        ActionWebview2::Retirer
    } else {
        ActionWebview2::NePasToucher
    }
}

#[cfg(test)]
mod tests {
    use super::{action_webview2, retirer_inspecteur_webkit, ActionWebview2};

    #[test]
    fn l_inspecteur_webkit_herite_est_retire_hors_pilotage() {
        assert!(retirer_inspecteur_webkit(true, false));
        assert!(!retirer_inspecteur_webkit(false, false));
    }

    #[test]
    fn l_inspecteur_webkit_est_garde_sous_webdriver() {
        // Régression vue en CI : la suite bout en bout ne démarrait plus, la
        // variable posée par WebKitWebDriver étant retirée avant WebKit.
        assert!(!retirer_inspecteur_webkit(true, true));
        assert!(!retirer_inspecteur_webkit(false, true));
    }

    #[test]
    fn hors_session_distante_sans_heritage_ne_touche_a_rien() {
        assert_eq!(
            action_webview2(false, false, false),
            ActionWebview2::NePasToucher
        );
    }

    #[test]
    fn en_session_distante_pose_toujours_la_composition_logicielle() {
        // Peu importe une éventuelle valeur héritée : on impose la nôtre, on ne la
        // fusionne pas (une ligne de commande Chromium n'est pas filtrable sûrement).
        assert_eq!(
            action_webview2(false, true, false),
            ActionWebview2::Poser("--disable-gpu-compositing")
        );
        assert_eq!(
            action_webview2(true, true, false),
            ActionWebview2::Poser("--disable-gpu-compositing")
        );
    }

    #[test]
    fn hors_session_distante_une_valeur_heritee_est_retiree() {
        // Le cœur du durcissement : un --remote-debugging-port (ou un
        // --renderer-cmd-prefix, ou un --no-sand"box échappant à tout filtre)
        // planté dans l'environnement par un pied local n'atteint jamais la webview,
        // parce qu'on retire la variable au lieu d'essayer de la nettoyer.
        assert_eq!(action_webview2(true, false, false), ActionWebview2::Retirer);
    }

    #[test]
    fn sous_webdriver_la_variable_posee_par_le_pilote_est_gardee() {
        // Régression vue en CI Windows : chaque scénario mourait sur
        // « DevToolsActivePort file doesn't exist » — Edge WebDriver pose le port
        // de débogage dans cette variable, et on la retirait avant WebView2.
        assert_eq!(
            action_webview2(true, false, true),
            ActionWebview2::NePasToucher
        );
        assert_eq!(
            action_webview2(false, false, true),
            ActionWebview2::NePasToucher
        );
        // Même à travers RDP : le pilote a besoin de son port, pas de notre
        // composition logicielle.
        assert_eq!(
            action_webview2(true, true, true),
            ActionWebview2::NePasToucher
        );
    }
}
