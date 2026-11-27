//! Réglages de la vue `WebView2` que Tauri n'expose pas.
//!
//! Audit du 27 novembre 2026, WI-1 et WI-12.
//!
//! - `WebView2` garde ses raccourcis de navigateur actifs
//!   (`AreBrowserAcceleratorKeysEnabled` vaut vrai par défaut, wry ne le change
//!   que sur demande et Tauri ne le demande pas) : F5, Ctrl+R et Ctrl+Maj+R
//!   rechargeaient toute l'interface dès que le focus n'était pas dans un
//!   terminal (barre latérale, panneau SFTP, où F5 est le geste « actualiser »
//!   de l'Explorateur et de `WinSCP`), Ctrl+P ouvrait l'impression de la page.
//!   Les dix onglets disparaissaient d'un coup. Couper ces raccourcis laisse
//!   copier, coller, couper, tout sélectionner et annuler, qui ne sont pas des
//!   raccourcis de navigateur au sens de `WebView2`, et la page reçoit toujours
//!   ses `keydown` : le zoom de police d'Avash, fait en JavaScript, reste.
//! - Aucun gestionnaire de `ProcessFailed` n'était posé (wry n'en pose que sous
//!   macOS) : un moteur de rendu tombé laissait une page d'erreur, d'où F5 était
//!   la seule issue. F5 étant coupé, on recharge nous-mêmes ; le rechargement
//!   passe par `on_page_load`, qui ferme les sessions de la page disparue
//!   (voir `commands::purger_la_page`), et le bandeau de restauration propose
//!   de rouvrir les onglets mémorisés.

use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2Controller, ICoreWebView2ProcessFailedEventArgs,
    ICoreWebView2Settings3, COREWEBVIEW2_PROCESS_FAILED_KIND,
    COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED,
};
use webview2_com::ProcessFailedEventHandler;
use windows_core::Interface as _;

/// Pose les réglages sur la vue de la fenêtre principale. Un échec se
/// journalise : la ceinture du front (`web/raccourcis.ts`) annule encore F5 et
/// Ctrl+R hors des terminaux.
pub(crate) fn regler<R: tauri::Runtime>(fenetre: &tauri::WebviewWindow<R>) {
    let pose = fenetre.with_webview(|vue| {
        let controleur = vue.controller();
        // SAFETY: `controleur` est le contrôleur `WebView2` vivant de la fenêtre,
        // rendu par Tauri dans ce rappel exécuté sur le fil de l'interface, seul
        // fil où ces interfaces COM peuvent être appelées. Les appels ci-dessous
        // ne conservent aucun pointeur brut : le gestionnaire est un objet COM
        // compté par référence, gardé par `WebView2`.
        if let Err(e) = unsafe { regler_controleur(&controleur) } {
            tracing::warn!("réglages de la vue WebView2 non posés : {e}");
        }
    });
    if let Err(e) = pose {
        tracing::warn!("vue WebView2 inaccessible : {e}");
    }
}

/// Le moteur de rendu doit-il être relancé après cet arrêt ?
///
/// Seul l'arrêt du processus de rendu se répare par un rechargement : la vue
/// en relance un neuf. Un processus de rendu qui ne répond plus peut repartir
/// seul (une sortie énorme qui se résorbe) ; le recharger d'office perdrait
/// l'interface pour rien. La fin du processus du navigateur, elle, emporte la
/// vue entière : rien à recharger.
fn recharger_apres(genre: COREWEBVIEW2_PROCESS_FAILED_KIND) -> bool {
    genre == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED
}

/// # Safety
///
/// Appelée sur le fil de l'interface avec le contrôleur vivant de la vue
/// (voir `regler`).
unsafe fn regler_controleur(controleur: &ICoreWebView2Controller) -> windows_core::Result<()> {
    // SAFETY: contrat de la fonction (contrôleur vivant, fil de l'interface).
    let coeur = unsafe { controleur.CoreWebView2() }?;
    // SAFETY: `coeur` vient d'être rendu par le contrôleur vivant.
    let reglages = unsafe { coeur.Settings() }?;
    if let Ok(reglages3) = reglages.cast::<ICoreWebView2Settings3>() {
        // SAFETY: interface obtenue par `cast` sur des réglages vivants.
        unsafe { reglages3.SetAreBrowserAcceleratorKeysEnabled(false) }?;
    }
    let gestionnaire = ProcessFailedEventHandler::create(Box::new(
        |vue: Option<ICoreWebView2>, args: Option<ICoreWebView2ProcessFailedEventArgs>| {
            let (Some(vue), Some(args)) = (vue, args) else {
                return Ok(());
            };
            let mut genre = COREWEBVIEW2_PROCESS_FAILED_KIND::default();
            // SAFETY: `args` est l'argument vivant de l'événement, `genre` une
            // variable locale qui reçoit la valeur.
            unsafe { args.ProcessFailedKind(&raw mut genre) }?;
            tracing::error!(genre = genre.0, "processus de WebView2 arrêté");
            if recharger_apres(genre) {
                // SAFETY: `vue` est la vue émettrice, vivante pendant l'événement.
                unsafe { vue.Reload() }?;
            }
            Ok(())
        },
    ));
    let mut jeton = 0_i64;
    // SAFETY: `gestionnaire` est un objet COM valide ; `jeton` reçoit le
    // numéro d'abonnement, inutile ici puisque l'abonnement vit autant que la
    // vue.
    unsafe { coeur.add_ProcessFailed(&gestionnaire, &raw mut jeton) }?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::recharger_apres;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE,
    };

    /// Audit du 27 novembre 2026 (WI-12) : seul l'arrêt du moteur de rendu
    /// recharge la page ; un moteur qui ne répond plus peut repartir seul.
    #[test]
    fn seul_l_arret_du_moteur_de_rendu_recharge_la_page() {
        assert!(recharger_apres(
            COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED
        ));
        assert!(!recharger_apres(
            COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE
        ));
        assert!(!recharger_apres(
            COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED
        ));
    }
}
