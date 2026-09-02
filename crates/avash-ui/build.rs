//! Script de construction : ressources Tauri et, sous Windows (MSVC), le
//! manifeste d'application posé par l'éditeur de liens sur CHAQUE exécutable
//! du paquet — l'application, mais aussi les binaires de test.
//!
//! Pourquoi : `tauri_build` embarque le manifeste dans la ressource `.res`
//! qu'il ne lie qu'aux binaires (`rustc-link-arg-bins`). Les exécutables de
//! test n'en reçoivent aucun ; le chargeur Windows leur donne alors l'ancienne
//! `comctl32.dll` (version 5), où `TaskDialogIndirect` n'existe pas, et le
//! processus meurt avant `main` : `STATUS_ENTRYPOINT_NOT_FOUND` (0xc0000139).
//! C'est tauri-apps/tauri#13419, sans correctif amont ; l'espace de travail de
//! Tauri contourne avec `/MANIFEST:EMBED`. Comme ce paquet a aussi un binaire,
//! le manifeste est retiré de la ressource (sinon CVT1100, ressource en double)
//! et posé par l'éditeur de liens pour tout le monde, depuis le même fichier
//! `windows-app-manifest.xml` (copie de celui de `tauri-build`).

fn main() {
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if os != "windows" || env != "msvc" {
        tauri_build::build();
        return;
    }
    let manifeste = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("windows-app-manifest.xml");
    println!("cargo:rerun-if-changed={}", manifeste.display());
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg=/MANIFESTINPUT:{}",
        manifeste.display()
    );
    let attributs = tauri_build::Attributes::new()
        .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
    if let Err(erreur) = tauri_build::try_build(attributs) {
        panic!("tauri-build : {erreur:#}");
    }
}
