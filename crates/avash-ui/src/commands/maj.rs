//! Canal de mise à jour : dire au front qui gère les mises à jour.
//!
//! Trouvé par l'audit du 7 septembre 2026 : le greffon `tauri-plugin-updater`
//! est enregistré sans garde et son endpoint est actif, mais l'installeur
//! intégré ne peut pas aboutir sur les canaux Linux autres que l'`AppImage`.
//!
//! Le manifeste `latest.json` produit par la chaîne de publication ne déclare
//! que la clé générique `linux-x86_64`, qui pointe l'`AppImage`. Or le greffon
//! cherche d'abord `linux-x86_64-{deb,rpm}` puis retombe sur `linux-x86_64` :
//!
//! - `.deb`/`.rpm` officiels : `bundle_type()` rend `Deb`/`Rpm`, la clé
//!   spécifique manque, le greffon prend l'`AppImage` puis `install_deb`/
//!   `install_rpm` rejettent ses octets (`InvalidUpdaterFormat`) après une
//!   centaine de mégaoctets téléchargés.
//! - AUR/Flathub compilés par `cargo build` nu : pas d'estampille bundler,
//!   `bundle_type()` rend `None`, même retombée sur l'`AppImage`, et de toute
//!   façon `/usr` (AUR) et `/app` (Flatpak) sont en lecture seule.
//!
//! Sur tous ces canaux, c'est le gestionnaire de paquets (pacman, apt, dnf,
//! flatpak) qui met à jour : le front court-circuite la vérification au lieu
//! d'appeler le greffon, ce qui évite le message « Version X disponible »
//! trompeur suivi d'un échec. Seul l'`AppImage`, que `latest.json` sert
//! réellement, garde la mise à jour intégrée sous Linux ; Windows et macOS
//! ne sont pas concernés.

use tauri::utils::config::BundleType;
use tauri::utils::platform::bundle_type;

/// Vrai quand l'empaquetage se met à jour par son gestionnaire de paquets et
/// non par le greffon updater : le front ne doit alors pas proposer de mise à
/// jour intégrée, il renvoie l'utilisateur vers pacman/apt/dnf/flatpak.
#[tauri::command]
#[must_use]
pub fn emballage_gere_ses_mises_a_jour() -> bool {
    gere_par_gestionnaire_de_paquets(bundle_type(), std::env::var_os("FLATPAK_ID").is_some())
}

/// Décision isolée du greffon et de l'environnement pour être testable.
///
/// Vrai sous Flatpak (`/app` en lecture seule) et, sous Linux, pour tout
/// empaquetage autre que l'`AppImage` : le manifeste `latest.json` ne sert que la
/// clé `linux-x86_64` (l'`AppImage`), donc `.deb`/`.rpm` (bundle `Deb`/`Rpm`) et
/// les binaires non estampillés (bundle `None` : AUR, Flathub) mèneraient à un
/// téléchargement d'`AppImage` que l'installeur refuse. Windows (`Msi`/`Nsis`) et
/// macOS (`App`) ont chacun leur clé et s'installent par le greffon : faux.
///
/// Si un jour `latest.json` déclare `linux-x86_64-deb`/`-rpm`, il faudra
/// resserrer cette garde pour laisser `.deb`/`.rpm` repasser par le greffon.
fn gere_par_gestionnaire_de_paquets(bundle: Option<BundleType>, flatpak: bool) -> bool {
    if flatpak {
        return true;
    }
    cfg!(target_os = "linux") && bundle != Some(BundleType::AppImage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_maj_ne_se_propose_pas_sous_flatpak() {
        // FLATPAK_ID posé : /app en lecture seule, aucune installation possible,
        // quelle que soit l'estampille du binaire.
        assert!(gere_par_gestionnaire_de_paquets(None, true));
        assert!(gere_par_gestionnaire_de_paquets(
            Some(BundleType::AppImage),
            true
        ));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn la_maj_ne_se_propose_pas_hors_appimage_sous_linux() {
        // Le défaut vu par l'audit : deb, rpm et AUR/Flathub (bundle None)
        // aboutissaient tous au téléchargement de l'AppImage puis à un échec
        // d'installation, parce que latest.json ne sert que la clé linux-x86_64.
        assert!(gere_par_gestionnaire_de_paquets(
            Some(BundleType::Deb),
            false
        ));
        assert!(gere_par_gestionnaire_de_paquets(
            Some(BundleType::Rpm),
            false
        ));
        assert!(gere_par_gestionnaire_de_paquets(None, false));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn la_maj_reste_proposee_pour_l_appimage() {
        // Le seul canal Linux que latest.json sert vraiment : on n'y touche pas.
        assert!(!gere_par_gestionnaire_de_paquets(
            Some(BundleType::AppImage),
            false
        ));
    }

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn la_maj_reste_proposee_hors_linux() {
        // Windows (Msi/Nsis) et macOS (App) ont chacun leur clé dédiée : le
        // greffon les installe, on ne court-circuite pas.
        assert!(!gere_par_gestionnaire_de_paquets(
            Some(BundleType::Msi),
            false
        ));
        assert!(!gere_par_gestionnaire_de_paquets(
            Some(BundleType::Nsis),
            false
        ));
        assert!(!gere_par_gestionnaire_de_paquets(
            Some(BundleType::App),
            false
        ));
    }
}
