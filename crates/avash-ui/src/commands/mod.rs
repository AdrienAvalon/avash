//! Commandes Tauri d'Avash : hôtes, one-shot, sessions PTY, SFTP, tunnels.

// Un fichier par domaine ; tout est réexporté ici, si bien que `commands::x`
// reste le chemin de chaque commande, et que chaque fichier voit ses voisins
// par `use super::*`.

mod cles;
mod diagnostic;
mod dossiers;
mod enregistrement;
mod import;
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

pub use cles::*;
pub use diagnostic::*;
pub use dossiers::*;
pub use enregistrement::*;
pub use import::*;
pub use onglets::*;
pub use sante::*;
pub use secrets::*;
pub use serie::*;
pub use sessions::*;
pub use sftp::*;
pub use snippets::*;
pub use tunnels::*;
