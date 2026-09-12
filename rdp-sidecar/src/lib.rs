//! Bibliothèque du sidecar RDP/VNC : les modules que `main.rs` orchestre,
//! rendus atteignables depuis l'extérieur du crate.
//!
//! Trouvé par l'audit du 7 septembre 2026 : le crate ne déclarait qu'un
//! `[[bin]]`, si bien que rien ne pouvait en dépendre. La règle de
//! `CLAUDE.md` (« un nouveau parseur reçoit une cible dans `fuzz/` ») était
//! donc intenable pour tout ce que le sidecar lit du serveur ou de
//! l'interface : cadrage EGFX, correspondance de motifs RDPDR, chemins
//! CLIPRDR, entrées. En exposant une bibliothèque, `tests/` (et une future
//! cible `fuzz/`) peuvent piloter ces parseurs par leur frontière publique ;
//! `main.rs` en devient un simple appelant.

// Mêmes tolérances stylistiques que le binaire, pour les modules déplacés ici :
// noms de produits en prose (doc_markdown), fonctions longues de décodage
// (too_many_lines) et coordonnées/RGBA aux noms courts idiomatiques.
#![allow(
    clippy::doc_markdown,
    clippy::too_many_lines,
    clippy::many_single_char_names
)]

pub mod acces_local;
pub mod args;
pub mod capture;
pub mod connexion;
pub mod disque;
pub mod egfx;
pub mod empreintes;
pub mod entrees;
pub mod fichiers;
pub mod magnetoscope;
pub mod presse_papiers;
pub mod progressif;
pub mod session;
pub mod son;
pub mod surface;
pub mod tls_herite;
pub mod trames;
pub mod verrou;
pub mod vnc;
pub mod vnc_tls;
