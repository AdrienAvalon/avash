//! Canal de mise à jour : dire au front qui installe les mises à jour.
//!
//! Trouvé par l'audit du 7 septembre 2026 : le greffon `tauri-plugin-updater`
//! est enregistré sans garde et son endpoint est actif, mais l'installeur
//! intégré ne peut pas aboutir sur plusieurs empaquetages livrés.
//!
//! Le manifeste `latest.json` produit par la chaîne de publication ne déclare
//! que `linux-x86_64` (l'`AppImage`), `windows-x86_64` (le setup NSIS) et
//! `darwin-aarch64` (l'`app` macOS). Or plusieurs canaux livrés ne s'y
//! installent pas :
//!
//! - `.deb`/`.rpm` officiels : `bundle_type()` rend `Deb`/`Rpm`, la clé
//!   spécifique manque, le greffon retombe sur l'`AppImage` puis `install_deb`/
//!   `install_rpm` rejettent ses octets (`InvalidUpdaterFormat`) après une
//!   centaine de mégaoctets téléchargés. C'est `apt`/`dnf` qui met à jour.
//! - AUR/Flathub compilés par `cargo build` nu sous Linux : pas d'estampille
//!   bundler, `bundle_type()` rend `None`, même retombée sur l'`AppImage`, et de
//!   toute façon `/usr` (AUR) et `/app` (Flatpak) sont en lecture seule. C'est
//!   `pacman`/`flatpak` qui met à jour.
//! - Archive portable Windows : `release.yml` copie `target/release/avash-ui.exe`
//!   brut dans le zip (jamais estampillé par le bundler), donc `bundle_type()` y
//!   rend `None`. Le greffon téléchargerait le setup NSIS servi par
//!   `windows-x86_64` et installerait une SECONDE copie dans
//!   `%LOCALAPPDATA%\Programs` ; le dossier portable resterait à l'ancienne
//!   version et reproposerait la mise à jour à chaque lancement. Là, il n'y a
//!   pas de gestionnaire de paquets : on renvoie l'utilisateur télécharger la
//!   nouvelle archive sur la page de release.
//!
//! Seuls l'`AppImage` (Linux), le setup NSIS/MSI (Windows installé) et l'`app`
//! (macOS), que `latest.json` sert réellement, gardent la mise à jour intégrée
//! par le greffon.

use tauri::utils::config::BundleType;
use tauri::utils::platform::bundle_type;

/// Qui installe la mise à jour de l'empaquetage courant, pour le front :
///
/// - `"greffon"` : le greffon updater installe (`AppImage`, Windows installé,
///   macOS) ; le front propose la mise à jour intégrée ;
/// - `"gestionnaire"` : un gestionnaire de paquets met à jour (Flatpak, AUR,
///   Flathub, `.deb`, `.rpm`) ; le front y renvoie l'utilisateur ;
/// - `"archive"` : archive portable Windows non estampillée ; le front renvoie
///   vers la page de release pour télécharger la nouvelle archive.
///
/// Chaîne plutôt que booléen : `gestionnaire` et `archive` court-circuitent tous
/// deux le greffon mais avec un message distinct (pas de « gestionnaire de
/// paquets » sur une archive Windows).
#[tauri::command]
#[must_use]
pub fn canal_de_mise_a_jour() -> &'static str {
    canal(
        bundle_type(),
        cfg!(target_os = "linux"),
        cfg!(target_os = "windows"),
        std::env::var_os("FLATPAK_ID").is_some(),
    )
}

/// Décision isolée du greffon et de l'environnement pour être testable sur
/// n'importe quelle plateforme (linux/windows passés en paramètre).
fn canal(bundle: Option<BundleType>, linux: bool, windows: bool, flatpak: bool) -> &'static str {
    // Flatpak : /app en lecture seule, c'est flatpak qui met à jour, quelle que
    // soit l'estampille du binaire.
    if flatpak {
        return "gestionnaire";
    }
    // Linux hors AppImage : .deb/.rpm (bundle Deb/Rpm) et AUR/Flathub (bundle
    // None) retomberaient sur l'AppImage que latest.json sert seul. Le
    // gestionnaire de paquets (pacman/apt/dnf) met à jour.
    if linux && bundle != Some(BundleType::AppImage) {
        return "gestionnaire";
    }
    // Archive portable Windows : binaire brut jamais estampillé (bundle None). Le
    // greffon installerait le setup NSIS ailleurs (%LOCALAPPDATA%\Programs) en
    // laissant le dossier portable en arrière : on renvoie vers la release.
    if windows && bundle.is_none() {
        return "archive";
    }
    // AppImage, Windows installé (Nsis/Msi) et macOS (App) : le greffon installe.
    "greffon"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatpak_passe_par_le_gestionnaire() {
        // FLATPAK_ID posé : /app en lecture seule, aucune installation possible,
        // quelle que soit l'estampille du binaire.
        assert_eq!(canal(None, true, false, true), "gestionnaire");
        assert_eq!(
            canal(Some(BundleType::AppImage), true, false, true),
            "gestionnaire"
        );
    }

    #[test]
    fn linux_hors_appimage_passe_par_le_gestionnaire() {
        // Le défaut vu par l'audit : deb, rpm et AUR/Flathub (bundle None)
        // aboutissaient tous au téléchargement de l'AppImage puis à un échec
        // d'installation, parce que latest.json ne sert que la clé linux-x86_64.
        assert_eq!(
            canal(Some(BundleType::Deb), true, false, false),
            "gestionnaire"
        );
        assert_eq!(
            canal(Some(BundleType::Rpm), true, false, false),
            "gestionnaire"
        );
        assert_eq!(canal(None, true, false, false), "gestionnaire");
    }

    #[test]
    fn l_appimage_passe_par_le_greffon() {
        // Le seul canal Linux que latest.json sert vraiment : on n'y touche pas.
        assert_eq!(
            canal(Some(BundleType::AppImage), true, false, false),
            "greffon"
        );
    }

    #[test]
    fn l_archive_portable_windows_renvoie_a_la_release() {
        // Trouvé par la relecture du 7 septembre 2026 : release.yml copie
        // avash-ui.exe brut dans le zip portable, jamais estampillé par le
        // bundler, donc bundle_type() y rend None. Le greffon installerait le
        // setup NSIS ailleurs en laissant le dossier portable en arrière : la
        // pastille reproposait la mise à jour à chaque lancement (fausse
        // réussite). On renvoie télécharger la nouvelle archive.
        assert_eq!(canal(None, false, true, false), "archive");
    }

    #[test]
    fn windows_installe_et_macos_passent_par_le_greffon() {
        // Setup NSIS/MSI (Windows installé) et app (macOS) ont chacun leur clé
        // dédiée dans latest.json : le greffon les installe.
        assert_eq!(canal(Some(BundleType::Nsis), false, true, false), "greffon");
        assert_eq!(canal(Some(BundleType::Msi), false, true, false), "greffon");
        assert_eq!(canal(Some(BundleType::App), false, false, false), "greffon");
    }
}
