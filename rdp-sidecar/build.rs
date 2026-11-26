//! Sous Windows (MSVC), lie le runtime Visual C++ statiquement et garde l'UCRT,
//! composant de Windows, en dynamique : exactement ce que tauri-build fait pour
//! avash.exe.
//!
//! Trouvé le 26 novembre 2026 en cherchant pourquoi une connexion RDP échouait
//! depuis Windows vers un xrdp Debian : avash-rdp.exe importait
//! VCRUNTIME140.dll, que seul le « Visual C++ Redistributable » installe. Sur un
//! Windows neuf, le processus RDP mourait au lancement (0xC0000135,
//! STATUS_DLL_NOT_FOUND) et aucune connexion RDP n'aboutissait, quel que soit
//! le serveur ; avash.exe, lui, démarrait normalement. Reproduit sur une
//! machine virtuelle Windows 11 neuve, puis vérifié : une fois le runtime
//! installé, la même connexion arrivait à l'écran d'ouverture de session
//! d'xrdp. La chaîne Windows vérifie désormais les imports
//! (scripts/verifier-runtime-windows.ps1).
//!
//! Sans effet hors MSVC : la fonction sort aussitôt.
fn main() {
    static_vcruntime::metabuild();
}
