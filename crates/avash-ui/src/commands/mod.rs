//! Commandes Tauri d'Avash : hôtes, one-shot, sessions PTY, SFTP, tunnels.

// Un fichier par domaine ; tout est réexporté ici, si bien que `commands::x`
// reste le chemin de chaque commande, et que chaque fichier voit ses voisins
// par `use super::*`.

mod choix_locaux;
mod cles;
mod diagnostic;
mod dossiers;
mod enregistrement;
mod import;
mod maj;
mod onglets;
mod sante;
mod secrets;
mod serie;
mod sessions;
mod sftp;
mod snippets;
mod tunnels;

#[cfg(test)]
pub(crate) mod tests;
// Unix seulement : la fonctionnalité `outils-de-test` du cœur (serveur SSH+SFTP
// en mémoire) n'est tirée que par `[target.'cfg(unix)'.dev-dependencies]`
// dans Cargo.toml. Trouvé par la CI Windows du 12 septembre 2026 : le module
// était compilé partout et cherchait `avash::testutil`, absent sous Windows.
#[cfg(test)]
#[cfg(unix)]
mod tests_reseau;

pub use choix_locaux::*;
pub use cles::*;
pub use diagnostic::*;
pub use dossiers::*;
pub use enregistrement::*;
pub use import::*;
pub use maj::*;
pub use onglets::*;
pub use sante::*;
pub use secrets::*;
pub use serie::*;
pub use sessions::*;
pub use sftp::*;
pub use snippets::*;
pub use tunnels::*;
