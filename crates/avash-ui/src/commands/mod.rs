//! Commandes Tauri d'Avash : hôtes, one-shot, sessions PTY, SFTP, tunnels.

// Un fichier par domaine ; tout est réexporté ici, si bien que `commands::x`
// reste le chemin de chaque commande, et que chaque fichier voit ses voisins
// par `use super::*`.

mod cles;
mod dossiers;
mod enregistrement;
mod import;
mod sante;
mod secrets;
mod sessions;
mod sftp;
mod snippets;
mod tunnels;

#[cfg(test)]
pub(crate) mod tests;

pub use cles::*;
pub use dossiers::*;
pub use enregistrement::*;
pub use import::*;
pub use sante::*;
pub use secrets::*;
pub use sessions::*;
pub use sftp::*;
pub use snippets::*;
pub use tunnels::*;
