//! Rechargement de la page : le cœur oublie ce que la page disparue possédait.
//!
//! Audit du 27 novembre 2026 (WI-1, FI-2, ME-3, WI-12). Les sessions vivent
//! dans le processus Rust, les onglets dans la page. Quand la page se recharge
//! (F5 ou Ctrl+R sous `WebView2`, menu contextuel « Recharger », moteur de rendu
//! relancé après un arrêt), le front repart d'une liste vide et renumérote ses
//! onglets à partir de 1, alors que le cœur gardait tout : des sessions SSH
//! établies sans onglet, toujours listées par `open_sessions` (un snippet
//! « toutes les sessions », ou le seul orphelin quand aucun onglet n'est
//! rouvert, partait vers un serveur invisible), et, pendant la connexion d'un
//! nouvel onglet qui héritait d'un ancien numéro, des frappes envoyées à
//! l'ancienne session de ce numéro. Une page neuve ne possède aucune session :
//! au début de chaque chargement, on ferme tout ce que l'ancienne tenait.
//!
//! Ce qui n'appartient pas à la page reste : les tunnels (relus par
//! `tunnel_status`), les chemins désignés, la configuration.

use super::sessions::{finaliser_enregistrement, SessionStore};
use super::TransfertsStore;
use avash::Verrou as _;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Manager as _};

/// Ce que la purge a fermé, pour le journal et les tests.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Purge {
    /// Sessions SSH et série retirées du magasin.
    pub sessions: usize,
    /// Processus de bureau distant arrêtés.
    pub bureaux: usize,
    /// Transferts SFTP (et copies directes) dont l'annulation a été demandée.
    pub transferts: usize,
}

/// Ferme tout ce que possédait la page qui vient de disparaître.
///
/// Appelée au début de chaque chargement de la fenêtre principale
/// (`on_page_load`, événement `Started`) ; au premier chargement, les magasins
/// sont vides et rien ne change.
///
/// - Sessions : la génération de page avance et le magasin se vide sous le
///   même verrou (voir `enregistrer_session_de_la_page`) ; une connexion encore
///   en vol pour l'ancienne page sera refusée à son arrivée. Les poignées sont
///   lâchées hors du verrou : leurs canaux se ferment, le relais s'arrête et
///   envoie le `disconnect`. Aucun `pty-closed` n'est émis (`clore_session` ne
///   trouve plus rien à retirer) : la nouvelle page n'a pas ces onglets, et un
///   numéro qu'elle a déjà réattribué ne doit pas se fermer sous elle.
/// - `en_cours` et `annules` décrivaient les onglets de l'ancienne page : ils
///   repartent vides, sinon un numéro réattribué hériterait d'une annulation.
/// - Transferts : leur drapeau d'annulation est levé ; la boucle s'arrête
///   entre deux blocs et la carte de reprise est écrite.
/// - Bureaux distants : ils s'arrêtent déjà sur la fermeture de leur
///   WebSocket, mais l'enfant mort restait dans `RdpStore` (zombie sous Linux)
///   jusqu'à la réutilisation du numéro ; on les arrête et on les retire,
///   comme `rdp_close`.
pub fn purger_la_page<R: tauri::Runtime>(app: &AppHandle<R>) -> Purge {
    let mut purge = Purge::default();

    let store = app.state::<SessionStore>();
    let poignees = {
        let mut inner = store.inner.verrou();
        store.page.fetch_add(1, Ordering::AcqRel);
        store.en_cours.verrou().clear();
        store.annules.verrou().clear();
        std::mem::take(&mut *inner)
    };
    purge.sessions = poignees.len();
    for (id, h) in poignees {
        // Comme à la fermeture d'un onglet : l'erreur de vidage d'un
        // enregistrement ne doit pas se perdre dans le `Drop`.
        finaliser_enregistrement(app, id, &h.enregistreur);
        drop(h);
    }

    if let Some(transferts) = app.try_state::<TransfertsStore>() {
        let inner = transferts.inner.verrou();
        for drapeau in inner.values() {
            drapeau.store(true, Ordering::Relaxed);
        }
        purge.transferts = inner.len();
    }

    if let Some(rdp) = app.try_state::<crate::rdp::RdpStore>() {
        let enfants = std::mem::take(&mut *rdp.inner.verrou());
        purge.bureaux = enfants.len();
        for (_, mut enfant) in enfants {
            let _ = enfant.start_kill();
        }
        // Verrou tokio, pris sans attendre : l'entrée standard n'est tenue que
        // le temps d'une annonce (même choix que `rdp_close`). Si elle est
        // prise, l'entrée se fermera avec le processus arrêté.
        if let Ok(mut stdins) = rdp.stdins.try_lock() {
            stdins.clear();
        }
        rdp.journaux.verrou().clear();
    }

    if purge != Purge::default() {
        tracing::info!(
            sessions = purge.sessions,
            bureaux = purge.bureaux,
            transferts = purge.transferts,
            "page rechargée : ce que l'ancienne page possédait est fermé"
        );
    }
    purge
}

#[cfg(test)]
mod tests {
    use super::{purger_la_page, Purge};
    use crate::commands::tests::{app_de_test, poignee, poignee_avec_clavier};
    use crate::commands::{
        enregistrer_session, enregistrer_session_de_la_page, open_sessions, page_courante,
        pty_close, SessionStore, TransfertsStore, CONNEXION_ANNULEE,
    };
    use avash::Verrou as _;
    use tauri::Manager as _;

    /// Audit du 27 novembre 2026 (WI-1, FI-2, ME-3) : après un rechargement,
    /// les sessions de l'ancienne page restaient établies et listées par
    /// `open_sessions`. Elles sont désormais fermées : le canal du clavier se
    /// ferme (le relais s'arrête et se déconnecte) et le magasin est vide.
    #[tokio::test]
    async fn un_rechargement_de_la_page_ferme_les_sessions_du_coeur() {
        let app = app_de_test();
        let state = app.state::<SessionStore>();
        let (h1, mut clavier1) = poignee_avec_clavier(1);
        let (h2, mut clavier2) = poignee_avec_clavier(2);
        enregistrer_session(&state, 1, h1).unwrap();
        enregistrer_session(&state, 2, h2).unwrap();
        assert_eq!(open_sessions(app.state()).len(), 2);

        let purge = purger_la_page(app.handle());

        assert_eq!(purge.sessions, 2);
        assert!(open_sessions(app.state()).is_empty());
        // L'émetteur du clavier est lâché : la boucle PTY du cœur sort sur
        // `None` et ferme le canal SSH.
        assert!(clavier1.recv().await.is_none());
        assert!(clavier2.recv().await.is_none());
    }

    /// Le premier chargement trouve des magasins vides : rien n'est fermé, rien
    /// n'est journalisé, et l'application démarre comme avant.
    #[test]
    fn le_premier_chargement_ne_ferme_rien() {
        let app = app_de_test();
        assert_eq!(purger_la_page(app.handle()), Purge::default());
    }

    /// FI-2 : une connexion lancée par l'ancienne page et aboutie après le
    /// rechargement s'insérait sous un numéro que la nouvelle page attribue à
    /// un autre onglet. Elle est refusée comme une annulation.
    #[test]
    fn une_connexion_partie_avant_le_rechargement_est_refusee_a_son_arrivee() {
        let app = app_de_test();
        let state = app.state::<SessionStore>();
        state.en_cours.verrou().insert(1);
        let page_de_depart = page_courante(&state);

        purger_la_page(app.handle());

        let issue = enregistrer_session_de_la_page(&state, 1, page_de_depart, poignee(1));
        assert_eq!(issue.unwrap_err(), CONNEXION_ANNULEE);
        assert!(open_sessions(app.state()).is_empty());
        // La nouvelle page, elle, enregistre ses sessions normalement.
        let page = page_courante(&state);
        enregistrer_session_de_la_page(&state, 1, page, poignee(2)).unwrap();
        assert_eq!(open_sessions(app.state()).len(), 1);
    }

    /// `en_cours` et `annules` décrivaient les onglets de l'ancienne page :
    /// gardés, l'annulation d'un onglet fermé pendant sa connexion aurait
    /// frappé le premier onglet de la nouvelle page qui hérite de son numéro.
    #[tokio::test]
    async fn les_annulations_de_l_ancienne_page_ne_frappent_pas_la_nouvelle() {
        let app = app_de_test();
        let state = app.state::<SessionStore>();
        state.en_cours.verrou().insert(3);
        pty_close(app.handle().clone(), app.state(), 3)
            .await
            .unwrap();
        assert!(state.annules.verrou().contains(&3));

        purger_la_page(app.handle());

        assert!(state.annules.verrou().is_empty());
        assert!(state.en_cours.verrou().is_empty());
        enregistrer_session(&state, 3, poignee(1)).unwrap();
    }

    /// Les transferts de l'ancienne page sont annulés : leur boucle s'arrête
    /// entre deux blocs, carte de reprise écrite, au lieu de continuer pour une
    /// file que personne n'affiche plus.
    #[test]
    fn un_rechargement_annule_les_transferts_en_cours() {
        let app = app_de_test();
        let drapeau: avash::sftp::Annulation = std::sync::Arc::default();
        app.state::<TransfertsStore>()
            .inner
            .verrou()
            .insert(9, drapeau.clone());

        let purge = purger_la_page(app.handle());

        assert_eq!(purge.transferts, 1);
        assert!(drapeau.load(std::sync::atomic::Ordering::Relaxed));
    }

    /// ME-3 : le processus d'un bureau de l'ancienne page restait dans
    /// `RdpStore` (zombie sous Linux) jusqu'à la réutilisation de son numéro.
    #[cfg(unix)]
    #[tokio::test]
    async fn un_rechargement_arrete_les_processus_de_bureau() {
        let app = app_de_test();
        let enfant = tokio::process::Command::new("sleep")
            .arg("30")
            .kill_on_drop(true)
            .spawn()
            .expect("processus de test");
        let pid = enfant.id().expect("pid");
        app.state::<crate::rdp::RdpStore>()
            .inner
            .verrou()
            .insert(4, enfant);

        let purge = purger_la_page(app.handle());

        assert_eq!(purge.bureaux, 1);
        assert!(app
            .state::<crate::rdp::RdpStore>()
            .inner
            .verrou()
            .is_empty());
        // Le signal est parti : le processus disparaît (ou n'est plus qu'un
        // zombie récolté par tokio) en peu de temps.
        let proc = std::path::PathBuf::from(format!("/proc/{pid}/status"));
        let mut vivant = true;
        for _ in 0..100 {
            let etat = std::fs::read_to_string(&proc).unwrap_or_default();
            if etat.is_empty() || etat.contains("State:\tZ") {
                vivant = false;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(!vivant, "le processus du bureau tourne encore");
    }
}
