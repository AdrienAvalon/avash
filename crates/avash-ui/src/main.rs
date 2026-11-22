#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// Sous cargo-llvm-cov (`cfg(coverage)`, posé par lui seul), le profil
/// d'exécution est réécrit toutes les secondes. Le profil ne s'écrit
/// normalement qu'à la sortie du processus ; or le pilote WebDriver de la
/// suite bout en bout arrête l'application sans la laisser sortir, et cette
/// suite est le seul test à traverser les commandes Tauri. Sans ce fil, leur
/// couverture resterait à zéro (scripts/couverture.sh). Jamais dans un
/// binaire publié.
#[cfg(coverage)]
fn ecrire_le_profil_en_continu() {
    extern "C" {
        fn __llvm_profile_write_file() -> i32;
    }
    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
        // SAFETY: fonction du runtime de profilage, liée dès que le binaire
        // est instrumenté ; sans argument ni état partagé avec nous.
        let _ = unsafe { __llvm_profile_write_file() };
    });
}

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
    // AppImage : la bibliothèque Wayland du système d'abord (voir
    // `relance_avec_wayland_du_systeme`). Avant tout le reste : si elle a lieu,
    // la relance remplace ce processus et rien de ce qui suit n'a été fait deux
    // fois.
    #[cfg(target_os = "linux")]
    relance_avec_wayland_du_systeme();

    #[cfg(target_os = "linux")]
    {
        // Pilote NVIDIA sur X11 ou XWayland : le rendu DMABUF de WebKitGTK ne
        // parvient pas à allouer ses tampons (« Failed to create GBM buffer »)
        // et la fenêtre reste entièrement grise. Vu le 22 novembre 2026 sur une
        // RTX 3090 (pilote 620, WebKitGTK du système) en lançant le binaire avec
        // GDK_BACKEND=x11 ; en Wayland natif la même machine affiche tout. Sans
        // ce rendu, la fenêtre s'affiche et la frappe reste immédiate (mesuré).
        // Surchargeable : qui pose la variable lui-même garde sa valeur.
        if couper_dmabuf(
            std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_some(),
            std::path::Path::new("/proc/driver/nvidia/version").exists(),
            std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty()),
            std::env::var("GDK_BACKEND").ok().as_deref(),
        ) {
            // SAFETY: aucun autre fil n'existe encore (le fil de profilage, sous
            // cfg(coverage), est lancé plus bas, après ce bloc) et WebKit n'est
            // pas chargé : personne ne lit l'environnement en parallèle.
            unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
        }
        if std::env::var_os("WEBKIT_DISABLE_COMPOSITING_MODE").is_none() {
            // SAFETY: aucun autre fil n'existe encore (le fil de profilage, sous
            // cfg(coverage), est lancé plus bas, après ce bloc) et WebKit n'est
            // pas chargé : personne ne lit l'environnement en parallèle.
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
            // SAFETY: même garantie : aucun autre fil (profilage compris, lancé
            // plus bas) et WebKit pas encore chargé.
            unsafe { std::env::remove_var("WEBKIT_INSPECTOR_SERVER") };
        }
        // Trouvé par l'audit du 7 septembre 2026 : la même WebKitGTK honore aussi
        // WEBKIT_INSPECTOR_HTTP_SERVER=adresse:port, qui ouvre un serveur HTTP
        // d'inspection distante (pilotable depuis n'importe quel navigateur, et
        // rien n'impose une adresse de bouclage). Dans webkitInitialize elle est
        // même testée AVANT WEBKIT_INSPECTOR_SERVER : ne retirer que cette
        // dernière ne fermait donc rien quand les deux étaient héritées. On la
        // retire, elle aussi. SANS l'exception WebDriver, contrairement à
        // WEBKIT_INSPECTOR_SERVER : WebKitWebDriver ne pose jamais la variante
        // HTTP (littéral absent de `strings /usr/bin/WebKitWebDriver`), donc la
        // garder sous pilotage n'exposerait le port que pour rien.
        if retirer_inspecteur_http_webkit(
            std::env::var_os("WEBKIT_INSPECTOR_HTTP_SERVER").is_some(),
        ) {
            // SAFETY: même garantie : aucun autre fil (profilage compris, lancé
            // plus bas) et WebKit pas encore chargé.
            unsafe { std::env::remove_var("WEBKIT_INSPECTOR_HTTP_SERVER") };
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
            // SAFETY: sous Windows, set_var et remove_var sont toujours sûrs
            // (std::env, section Safety) ; on est de surcroît avant WebView2 et
            // avant tout autre fil (le profilage est lancé plus bas).
            ActionWebview2::Poser(v) => unsafe {
                std::env::set_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", v)
            },
            // SAFETY: même garantie que le bras précédent (audit du 12 septembre
            // 2026, C-unsafe-5 : un commentaire au-dessus du premier bras ne
            // couvre pas le second).
            ActionWebview2::Retirer => unsafe {
                std::env::remove_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS")
            },
            ActionWebview2::NePasToucher => {}
        }
    }

    // Le fil du profil de couverture part APRÈS le bloc d'environnement
    // (audit du 12 septembre 2026, C-unsafe-2) : lancé en tête, il rendait
    // fausse, sous cfg(coverage), l'affirmation « aucun autre fil » des
    // commentaires de sûreté ci-dessus.
    #[cfg(coverage)]
    ecrire_le_profil_en_continu();

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
    // SAFETY: GetSystemMetrics n'a ni pointeur, ni précondition, ni effet de
    // bord : l'assertion de sûreté remonte à la déclaration (`safe fn`, Rust
    // 1.82), et l'appel n'a plus besoin de bloc (audit du 12 septembre 2026,
    // C-unsafe-5).
    #[link(name = "user32")]
    unsafe extern "system" {
        safe fn GetSystemMetrics(n_index: i32) -> i32;
    }
    GetSystemMetrics(SM_REMOTESESSION) != 0
}

/// Là où les distributions rangent `libwayland-client` : Arch et dérivées,
/// Fedora et openSUSE, Debian et Ubuntu.
#[cfg(any(target_os = "linux", test))]
const WAYLAND_DU_SYSTEME: [&str; 4] = [
    "/usr/lib/libwayland-client.so.0",
    "/usr/lib64/libwayland-client.so.0",
    "/usr/lib/x86_64-linux-gnu/libwayland-client.so.0",
    "/lib/x86_64-linux-gnu/libwayland-client.so.0",
];

/// `AppImage` : relance le processus avec la `libwayland-client` du système en
/// tête de `LD_PRELOAD`.
///
/// L'`AppImage` embarque les `libwayland-*` de la machine de construction
/// (Ubuntu), mais pas Mesa : le pilote EGL du système se retrouve lié à une
/// `libwayland-client` plus vieille que lui et refuse de créer son écran,
/// « Could not create default EGL display: `EGL_BAD_PARAMETER`. Aborting... »,
/// fenêtre jamais affichée. Signalé le 22 novembre 2026 sur un portable Intel
/// et NVIDIA sous `CachyOS` (0.14.0), reproduit sur un poste NVIDIA en forçant
/// le pilote EGL de Mesa ; ni `WEBKIT_DISABLE_DMABUF_RENDERER` ni le retrait de
/// `WAYLAND_DISPLAY` n'y changent rien, seule la bibliothèque du système répare.
/// L'éditeur de liens ne relit pas `LD_PRELOAD` en cours de route : il faut un
/// nouveau processus.
///
/// `LD_PRELOAD` reste posé après la relance : les processus de `WebKit` (rendu,
/// réseau) sont des fils de celui-ci et butent sur la même erreur sans lui. Un
/// premier jet le retirait au second passage, et la fenêtre restait vide
/// (essai du 22 novembre 2026). Les autres fils (bureau distant, navigateur,
/// `ProxyCommand`) héritent donc d'une bibliothèque du système préchargée, sans
/// effet pour eux. C'est aussi ce qui arrête la boucle : au second passage la
/// bibliothèque est déjà dans `LD_PRELOAD`, il n'y a plus rien à faire.
#[cfg(target_os = "linux")]
fn relance_avec_wayland_du_systeme() {
    use std::os::unix::process::CommandExt;

    let preload = std::env::var("LD_PRELOAD").unwrap_or_default();
    let Some(appdir) = std::env::var_os("APPDIR").filter(|v| !v.is_empty()) else {
        return;
    };
    let Some(bibliotheque) = wayland_du_systeme(
        std::path::Path::new(&appdir),
        &preload,
        std::path::Path::exists,
    ) else {
        return;
    };
    let nouveau = if preload.is_empty() {
        bibliotheque.to_owned()
    } else {
        format!("{bibliotheque}:{preload}")
    };
    // `exec` ne revient que sur un échec : on continue alors sans la relance,
    // ce qui laisse l'application telle qu'elle était avant ce correctif.
    // `arg0` : le nom sous lequel on a été lancé, pas « /proc/self/exe », que
    // GTK prendrait pour nom de programme (classe de fenêtre, regroupement dans
    // la barre des tâches).
    let mut arguments = std::env::args_os();
    let nom = arguments.next().unwrap_or_else(|| "avash-ui".into());
    let erreur = std::process::Command::new("/proc/self/exe")
        .arg0(nom)
        .args(arguments)
        .env("LD_PRELOAD", nouveau)
        .exec();
    eprintln!("avash : relance avec la bibliothèque Wayland du système impossible ({erreur})");
}

/// La `libwayland-client` du système à précharger, ou `None` s'il n'y a rien à
/// faire : une bibliothèque Wayland est déjà préchargée (par l'utilisateur ou
/// par une relance précédente), ou aucune n'existe hors de l'`AppImage`.
///
/// Pur à l'accès au disque près, passé en paramètre : testable partout.
#[cfg(any(target_os = "linux", test))]
fn wayland_du_systeme(
    appdir: &std::path::Path,
    preload: &str,
    existe: impl Fn(&std::path::Path) -> bool,
) -> Option<&'static str> {
    if preload.contains("libwayland-client") {
        return None;
    }
    WAYLAND_DU_SYSTEME.into_iter().find(|chemin| {
        let chemin = std::path::Path::new(chemin);
        !chemin.starts_with(appdir) && existe(chemin)
    })
}

/// Faut-il couper le rendu DMABUF de `WebKitGTK` ? Oui avec le pilote NVIDIA dès
/// que GTK passe par X11 (session X11, `XWayland`, ou `GDK_BACKEND=x11` comme
/// dans l'`AppImage`), jamais si l'utilisateur a déjà tranché.
///
/// Pur : aucun accès à l'environnement, testable sur toute plateforme.
#[cfg(any(target_os = "linux", test))]
fn couper_dmabuf(
    deja_pose: bool,
    nvidia: bool,
    wayland_display: bool,
    gdk_backend: Option<&str>,
) -> bool {
    // GTK essaie les moteurs de GDK_BACKEND dans l'ordre ; sans la variable, il
    // prend Wayland dès qu'un compositeur répond.
    let premier = gdk_backend.and_then(|v| v.split(',').next()).map(str::trim);
    let par_x11 = match premier {
        Some("x11") => true,
        Some("wayland") => false,
        _ => !wayland_display,
    };
    !deja_pose && nvidia && par_x11
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

/// Faut-il retirer `WEBKIT_INSPECTOR_HTTP_SERVER` de l'environnement ? Oui dès
/// qu'une valeur est héritée, SANS exception de pilotage : contrairement à
/// `WEBKIT_INSPECTOR_SERVER`, `WebKitWebDriver` ne pose jamais cette variante
/// HTTP (littéral absent de son binaire), donc la garder sous pilotage
/// n'ouvrirait le serveur d'inspection HTTP que pour rien. Aucun drapeau
/// `automatisation` ici, à dessein.
///
/// Pur : aucun accès à l'environnement, testable sur toute plateforme.
#[cfg(any(target_os = "linux", test))]
fn retirer_inspecteur_http_webkit(herite: bool) -> bool {
    herite
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
    use super::{
        action_webview2, couper_dmabuf, retirer_inspecteur_http_webkit, retirer_inspecteur_webkit,
        wayland_du_systeme, ActionWebview2,
    };
    use std::path::Path;

    #[test]
    fn l_appimage_precharge_la_bibliotheque_wayland_du_systeme() {
        // Signalé le 22 novembre 2026 : l'AppImage 0.14.0 ne s'ouvrait pas sur
        // un portable Intel sous CachyOS (EGL_BAD_PARAMETER), la bibliothèque
        // embarquée masquant celle du système.
        let arch = |p: &Path| p == Path::new("/usr/lib/libwayland-client.so.0");
        assert_eq!(
            wayland_du_systeme(Path::new("/tmp/.mount_AvashX"), "", arch),
            Some("/usr/lib/libwayland-client.so.0")
        );
        let debian = |p: &Path| p == Path::new("/usr/lib/x86_64-linux-gnu/libwayland-client.so.0");
        assert_eq!(
            wayland_du_systeme(Path::new("/tmp/.mount_AvashX"), "/opt/autre.so", debian),
            Some("/usr/lib/x86_64-linux-gnu/libwayland-client.so.0")
        );
    }

    #[test]
    fn rien_a_precharger_sans_bibliotheque_du_systeme_ou_si_c_est_deja_fait() {
        let aucune = |_: &Path| false;
        assert_eq!(wayland_du_systeme(Path::new("/tmp/app"), "", aucune), None);
        // Déjà préchargée (par l'utilisateur, comme le contournement donné le
        // 22 novembre 2026) : pas de seconde relance.
        let toutes = |_: &Path| true;
        assert_eq!(
            wayland_du_systeme(
                Path::new("/tmp/app"),
                "/usr/lib/libwayland-client.so.0",
                toutes
            ),
            None
        );
        // Une AppImage extraite sous /usr/lib ne se précharge pas elle-même.
        assert_eq!(
            wayland_du_systeme(Path::new("/usr/lib"), "", |p| p
                == Path::new("/usr/lib/libwayland-client.so.0")),
            None
        );
    }

    #[test]
    fn le_rendu_dmabuf_est_coupe_pour_nvidia_sur_x11_seulement() {
        // Vu le 22 novembre 2026 : fenêtre grise, « Failed to create GBM
        // buffer », binaire natif sous XWayland avec une RTX 3090.
        assert!(couper_dmabuf(false, true, true, Some("x11")));
        assert!(couper_dmabuf(false, true, false, None), "session X11 pure");
        assert!(couper_dmabuf(false, true, true, Some("x11,wayland")));
        // Wayland natif : la même machine affiche tout, on n'y touche pas.
        assert!(!couper_dmabuf(false, true, true, None));
        assert!(!couper_dmabuf(false, true, true, Some("wayland,x11")));
        // Autre pilote, ou choix déjà fait par l'utilisateur.
        assert!(!couper_dmabuf(false, false, false, Some("x11")));
        assert!(!couper_dmabuf(true, true, false, Some("x11")));
    }

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
    fn l_inspecteur_http_herite_est_retire_meme_sous_pilotage() {
        // Trouvé par l'audit du 7 septembre 2026 : WEBKIT_INSPECTOR_HTTP_SERVER
        // ouvre le même débogueur distant (en HTTP) et n'était pas retirée. Elle
        // se retire dès qu'elle est héritée, y compris sous pilotage WebDriver :
        // WebKitWebDriver ne pose jamais cette variante (littéral absent de son
        // binaire), donc pas d'exception d'automatisation à lui accorder.
        assert!(retirer_inspecteur_http_webkit(true));
        assert!(!retirer_inspecteur_http_webkit(false));
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
