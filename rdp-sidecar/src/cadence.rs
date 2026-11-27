//! Cadencement des trames vers l'interface, et suspension de l'affichage chez
//! le serveur quand personne ne regarde le bureau.
//!
//! Extrait de la grande boucle de `session.rs` par l'audit du 27 novembre 2026
//! (PR-1, QT-2) : la pause d'un onglet masqué n'y avait aucun test, et la
//! correction qui suspend enfin le serveur réécrivait de toute façon ce
//! cadencement. Tout ici est pur (l'instant est passé en argument) : la boucle
//! demande « que faut-il émettre ? » et « que faut-il dire au serveur ? », la
//! machine répond, les tests la pilotent sans réseau ni horloge.

use crate::trames::ajouter_rect;
use ironrdp::connector::connection_activation::DisplayUpdateSupport;
use ironrdp::pdu::geometry::InclusiveRectangle;
use ironrdp::pdu::rdp::headers::ShareDataPdu;
use ironrdp::pdu::rdp::refresh_rectangle::RefreshRectanglePdu;
use ironrdp::pdu::rdp::suppress_output::SuppressOutputPdu;
use std::time::{Duration, Instant};

/// Filet anti-gel : si un ACK de rendu se perd, on renvoie l'état courant
/// passé ce délai plutôt que de figer l'affichage.
pub(crate) const ACK_TIMEOUT: Duration = Duration::from_millis(250);

/// Combien de temps l'onglet doit rester masqué avant qu'on suspende le
/// serveur. Passer d'un onglet à l'autre au clavier (Ctrl+Tab) traverse un
/// bureau en quelques dizaines de millisecondes : le suspendre puis le relancer
/// à chaque passage coûterait deux PDU et une image complète pour rien. Une
/// seconde, comme le conseille la documentation de `display_suppressed_handle`
/// dans ironrdp-server (côté serveur, contre les clients qui pulsent le PDU) ;
/// côté client, c'est le même compromis vu de l'autre bout.
pub(crate) const ANTI_REBOND: Duration = Duration::from_secs(1);

/// Ce qu'il faut dire au serveur.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ordre {
    /// Plus personne ne regarde : Suppress Output, mises à jour coupées.
    Suspendre,
    /// Le bureau est de nouveau vu : Suppress Output qui les rétablit, puis
    /// Refresh Rect si le serveur le permet, pour qu'il renvoie l'écran à jour
    /// plutôt que de laisser l'image d'avant la pause.
    Reprendre,
}

/// L'état du cadencement d'une session.
///
/// Au plus une trame « en vol ». Les mises à jour qui arrivent pendant le rendu
/// de la webview sont fusionnées (union des rectangles) ; à l'ACK, on envoie
/// l'état le plus récent. Rapide : chaque trame part aussitôt ; lent : on
/// fusionne, jamais de file qui s'accumule.
pub(crate) struct Cadence {
    /// La zone sale, en attente d'envoi. Publique dans le crate pour
    /// `nouvelle_taille`, qui la vide quand l'image change de taille.
    pub(crate) dirty: Vec<InclusiveRectangle>,
    /// Une trame est partie, son accusé n'est pas revenu.
    pub(crate) awaiting_ack: bool,
    last_send: Instant,
    /// Onglet masqué (message `[11, 1]`). Le canvas n'est pas à l'écran, mais
    /// l'accusé de rendu partait quand même : le sidecar poussait des trames
    /// pleines (8 Mo en 1080p) que personne ne regardait. En pause, on
    /// accumule la zone sale sans rien émettre.
    en_pause: bool,
    /// Depuis quand la pause dure, pour l'anti-rebond.
    pause_depuis: Option<Instant>,
    /// Le Suppress Output de suspension est parti, et pas encore son contraire.
    suspendu: bool,
    /// Ce que le serveur a annoncé dans son ensemble général de capacités.
    affichage: DisplayUpdateSupport,
}

impl Cadence {
    /// Une session neuve. `en_pause` vient du tour précédent quand il y en a
    /// un (redirection, reprise avec le canal graphique) : un onglet quitté
    /// avant la reprise doit rester en pause après elle (audit du 27 novembre
    /// 2026, PF-3).
    pub(crate) fn nouvelle(
        affichage: DisplayUpdateSupport,
        en_pause: bool,
        maintenant: Instant,
    ) -> Self {
        Self {
            dirty: Vec::new(),
            awaiting_ack: false,
            last_send: maintenant,
            en_pause,
            pause_depuis: en_pause.then_some(maintenant),
            suspendu: false,
            affichage,
        }
    }

    /// Une mise à jour décodée : sa zone rejoint celle à envoyer.
    pub(crate) fn ajouter(&mut self, r: &InclusiveRectangle) {
        ajouter_rect(&mut self.dirty, r);
    }

    /// La zone à émettre maintenant, s'il y en a une et que la voie est libre.
    /// Rendre une zone vaut envoi : la trame est désormais en vol.
    pub(crate) fn a_emettre(&mut self, maintenant: Instant) -> Option<Vec<InclusiveRectangle>> {
        if self.awaiting_ack || self.en_pause || self.dirty.is_empty() {
            return None;
        }
        self.awaiting_ack = true;
        self.last_send = maintenant;
        Some(std::mem::take(&mut self.dirty))
    }

    /// L'accusé de rendu de la page : la voie est libre. Rend le temps écoulé
    /// depuis l'envoi, en millisecondes (latence de bout en bout).
    pub(crate) fn accuse(&mut self, maintenant: Instant) -> f32 {
        self.awaiting_ack = false;
        maintenant
            .saturating_duration_since(self.last_send)
            .as_secs_f32()
            * 1000.0
    }

    /// `[11]` : l'onglet passe à l'arrière-plan (`true`) ou revient.
    pub(crate) fn pause(&mut self, pause: bool, maintenant: Instant) -> Option<Ordre> {
        if pause {
            if !self.en_pause {
                self.pause_depuis = Some(maintenant);
            }
            self.en_pause = true;
            return None;
        }
        self.lever_pause()
    }

    /// `[9]` : l'onglet a été réaffiché et son canvas peut être vide. L'image
    /// entière part hors cadencement ; la zone sale qu'elle couvre est oubliée.
    /// Un REFRESH ne s'obtient qu'en revenant au premier plan : il lève la
    /// pause, si bien que le flux ne peut pas rester gelé si l'interface
    /// oubliait de la lever explicitement.
    pub(crate) fn rafraichir(&mut self, maintenant: Instant) -> Option<Ordre> {
        self.dirty.clear();
        self.awaiting_ack = true;
        self.last_send = maintenant;
        self.lever_pause()
    }

    fn lever_pause(&mut self) -> Option<Ordre> {
        self.en_pause = false;
        self.pause_depuis = None;
        // Seul un serveur réellement suspendu reçoit l'ordre contraire : un
        // Suppress Output inattendu n'a pas à partir chez un serveur qui ne
        // l'a jamais annoncé.
        std::mem::take(&mut self.suspendu).then_some(Ordre::Reprendre)
    }

    /// Le tic de 100 ms de la boucle. Libère la voie si un accusé s'est perdu,
    /// et suspend le serveur quand l'onglet est masqué depuis assez longtemps.
    ///
    /// `dessine` : une première image a été reçue. Suspendre avant, c'est
    /// risquer deux pièges : mstsc a montré qu'une surface jamais initialisée
    /// ne se rétablit pas toujours (documentation de `display_suppressed_handle`
    /// côté ironrdp-server), et le délai `ATTENTE_EGFX` de la session prendrait
    /// un bureau suspendu pour un serveur muet, puis relancerait toute la
    /// connexion avec le canal graphique.
    pub(crate) fn tic(&mut self, maintenant: Instant, dessine: bool) -> Option<Ordre> {
        if self.awaiting_ack && maintenant.saturating_duration_since(self.last_send) > ACK_TIMEOUT {
            self.awaiting_ack = false;
        }
        let assez_longtemps = self
            .pause_depuis
            .is_some_and(|d| maintenant.saturating_duration_since(d) >= ANTI_REBOND);
        if self.en_pause
            && assez_longtemps
            && dessine
            && !self.suspendu
            && self.affichage.suppress_output
        {
            self.suspendu = true;
            return Some(Ordre::Suspendre);
        }
        None
    }

    /// Le serveur a déroulé une désactivation et une réactivation (changement
    /// de résolution) et y a pu annoncer un autre ensemble de capacités. Un
    /// serveur suspendu est suspendu de nouveau : rien ne dit qu'il a gardé
    /// l'état d'avant la réactivation, et le PDU est sans effet s'il l'a gardé.
    /// Un serveur qui ne l'annonce plus ne le reçoit plus.
    pub(crate) fn reactivation(&mut self, affichage: DisplayUpdateSupport) -> Option<Ordre> {
        self.affichage = affichage;
        if !affichage.suppress_output {
            self.suspendu = false;
            return None;
        }
        self.suspendu.then_some(Ordre::Suspendre)
    }
}

/// Les PDU qui portent un ordre au serveur, pour un bureau de cette taille.
pub(crate) fn pdus_de(
    ordre: Ordre,
    largeur: u16,
    hauteur: u16,
    affichage: DisplayUpdateSupport,
) -> Vec<ShareDataPdu> {
    let bureau = InclusiveRectangle {
        left: 0,
        top: 0,
        right: largeur.saturating_sub(1),
        bottom: hauteur.saturating_sub(1),
    };
    match ordre {
        Ordre::Suspendre => vec![ShareDataPdu::SuppressOutput(SuppressOutputPdu {
            desktop_rect: None,
        })],
        Ordre::Reprendre => {
            let mut v = vec![ShareDataPdu::SuppressOutput(SuppressOutputPdu {
                desktop_rect: Some(bureau.clone()),
            })];
            if affichage.refresh_rect {
                v.push(ShareDataPdu::RefreshRectangle(RefreshRectanglePdu {
                    areas_to_refresh: vec![bureau],
                }));
            }
            v
        }
    }
}

#[cfg(test)]
mod tests {
    //! Audit du 27 novembre 2026 (PR-1, QT-2) : la pause d'un bureau masqué
    //! n'avait aucun test, et elle ne retenait que l'envoi à la page ; le
    //! serveur continuait d'envoyer et le processus de décoder (300 % d'un
    //! cœur au banc, mire masquée).
    use super::*;

    const TOUT: DisplayUpdateSupport = DisplayUpdateSupport {
        suppress_output: true,
        refresh_rect: true,
    };
    const RIEN: DisplayUpdateSupport = DisplayUpdateSupport {
        suppress_output: false,
        refresh_rect: false,
    };

    fn rect(l: u16, t: u16, r: u16, b: u16) -> InclusiveRectangle {
        InclusiveRectangle {
            left: l,
            top: t,
            right: r,
            bottom: b,
        }
    }

    fn apres(t0: Instant, ms: u64) -> Instant {
        t0 + Duration::from_millis(ms)
    }

    #[test]
    fn en_pause_aucune_trame_ne_part_et_la_zone_sale_s_accumule() {
        let t0 = Instant::now();
        let mut c = Cadence::nouvelle(TOUT, false, t0);
        assert_eq!(c.pause(true, t0), None);
        c.ajouter(&rect(0, 0, 9, 9));
        c.ajouter(&rect(100, 100, 109, 109));
        assert!(c.a_emettre(t0).is_none(), "aucune trame en pause");
        assert_eq!(c.dirty.len(), 2, "les deux zones attendent la reprise");
    }

    #[test]
    fn a_la_reprise_une_seule_trame_couvre_les_zones_accumulees() {
        let t0 = Instant::now();
        let mut c = Cadence::nouvelle(TOUT, true, t0);
        c.ajouter(&rect(0, 0, 9, 9));
        c.ajouter(&rect(5, 5, 19, 19));
        c.pause(false, t0);
        let zones = c.a_emettre(t0).expect("la reprise émet");
        assert!(!zones.is_empty());
        assert!(c.a_emettre(t0).is_none(), "une seule trame en vol");
    }

    #[test]
    fn une_trame_en_vol_bloque_la_suivante_jusqu_a_l_accuse() {
        let t0 = Instant::now();
        let mut c = Cadence::nouvelle(TOUT, false, t0);
        c.ajouter(&rect(0, 0, 9, 9));
        assert!(c.a_emettre(t0).is_some());
        c.ajouter(&rect(20, 20, 29, 29));
        assert!(c.a_emettre(apres(t0, 10)).is_none(), "voie occupée");
        let rtt = c.accuse(apres(t0, 40));
        assert!((rtt - 40.0).abs() < 1.0, "latence mesurée : {rtt}");
        assert!(
            c.a_emettre(apres(t0, 41)).is_some(),
            "l'accusé libère la voie"
        );
    }

    #[test]
    fn un_accuse_perdu_ne_gele_pas_l_affichage() {
        let t0 = Instant::now();
        let mut c = Cadence::nouvelle(TOUT, false, t0);
        c.ajouter(&rect(0, 0, 9, 9));
        assert!(c.a_emettre(t0).is_some());
        c.ajouter(&rect(0, 0, 9, 9));
        c.tic(apres(t0, 100), true);
        assert!(
            c.a_emettre(apres(t0, 100)).is_none(),
            "encore dans le délai"
        );
        c.tic(apres(t0, 300), true);
        assert!(
            c.a_emettre(apres(t0, 300)).is_some(),
            "délai passé : voie libre"
        );
    }

    #[test]
    fn un_refresh_leve_la_pause_meme_sans_message_de_reprise() {
        let t0 = Instant::now();
        let mut c = Cadence::nouvelle(TOUT, false, t0);
        c.pause(true, t0);
        c.ajouter(&rect(0, 0, 9, 9));
        c.rafraichir(apres(t0, 10));
        assert!(!c.en_pause);
        assert!(c.dirty.is_empty(), "l'image entière couvre la zone sale");
        assert!(c.awaiting_ack, "l'image entière est en vol");
    }

    /// Le cœur de PR-1 : `[11, 1]`, puis l'anti-rebond écoulé, produit
    /// exactement une suspension, et la reprise exactement une reprise.
    #[test]
    fn un_bureau_masque_suspend_le_serveur_une_seule_fois_apres_l_anti_rebond() {
        let t0 = Instant::now();
        let mut c = Cadence::nouvelle(TOUT, false, t0);
        c.pause(true, t0);
        assert_eq!(c.tic(apres(t0, 500), true), None, "trop tôt : anti-rebond");
        assert_eq!(c.tic(apres(t0, 1000), true), Some(Ordre::Suspendre));
        assert_eq!(c.tic(apres(t0, 1100), true), None, "une seule fois");
        assert_eq!(c.tic(apres(t0, 5000), true), None);
        assert_eq!(c.pause(false, apres(t0, 6000)), Some(Ordre::Reprendre));
        assert_eq!(c.pause(false, apres(t0, 6100)), None, "reprise unique");
    }

    #[test]
    fn un_passage_bref_sur_un_autre_onglet_ne_suspend_rien() {
        let t0 = Instant::now();
        let mut c = Cadence::nouvelle(TOUT, false, t0);
        c.pause(true, t0);
        c.tic(apres(t0, 300), true);
        assert_eq!(c.pause(false, apres(t0, 400)), None);
        // Une nouvelle pause repart de zéro : l'ancienne ne compte pas.
        c.pause(true, apres(t0, 900));
        assert_eq!(c.tic(apres(t0, 1500), true), None);
        assert_eq!(c.tic(apres(t0, 1900), true), Some(Ordre::Suspendre));
    }

    /// MS-RDPBCGR interdit le PDU à un serveur qui ne l'annonce pas : on garde
    /// alors l'ancien comportement (envoi à la page retenu, rien de plus).
    #[test]
    fn un_serveur_qui_n_annonce_pas_la_capacite_n_est_jamais_suspendu() {
        let t0 = Instant::now();
        let mut c = Cadence::nouvelle(RIEN, false, t0);
        c.pause(true, t0);
        assert_eq!(c.tic(apres(t0, 5000), true), None);
        assert_eq!(c.pause(false, apres(t0, 6000)), None);
    }

    #[test]
    fn rien_n_est_suspendu_avant_la_premiere_image() {
        let t0 = Instant::now();
        let mut c = Cadence::nouvelle(TOUT, true, t0);
        assert_eq!(
            c.tic(apres(t0, 5000), false),
            None,
            "pas d'image : pas de suspension"
        );
        assert_eq!(c.tic(apres(t0, 5100), true), Some(Ordre::Suspendre));
    }

    #[test]
    fn un_refresh_reprend_un_serveur_suspendu() {
        let t0 = Instant::now();
        let mut c = Cadence::nouvelle(TOUT, true, t0);
        assert_eq!(c.tic(apres(t0, 1000), true), Some(Ordre::Suspendre));
        assert_eq!(c.rafraichir(apres(t0, 2000)), Some(Ordre::Reprendre));
    }

    #[test]
    fn une_reactivation_resuspend_un_serveur_suspendu_et_oublie_celui_qui_ne_l_annonce_plus() {
        let t0 = Instant::now();
        let mut c = Cadence::nouvelle(TOUT, true, t0);
        assert_eq!(c.reactivation(TOUT), None, "pas encore suspendu");
        assert_eq!(c.tic(apres(t0, 1000), true), Some(Ordre::Suspendre));
        assert_eq!(c.reactivation(TOUT), Some(Ordre::Suspendre));
        assert_eq!(c.reactivation(RIEN), None);
        assert_eq!(
            c.pause(false, apres(t0, 2000)),
            None,
            "plus rien à reprendre"
        );
    }

    #[test]
    fn les_pdus_disent_la_suspension_puis_la_reprise_et_le_rafraichissement() {
        let s = pdus_de(Ordre::Suspendre, 1920, 1080, TOUT);
        assert!(matches!(
            s.as_slice(),
            [ShareDataPdu::SuppressOutput(SuppressOutputPdu {
                desktop_rect: None
            })]
        ));
        let r = pdus_de(Ordre::Reprendre, 1920, 1080, TOUT);
        let plein = rect(0, 0, 1919, 1079);
        match r.as_slice() {
            [ShareDataPdu::SuppressOutput(SuppressOutputPdu {
                desktop_rect: Some(a),
            }), ShareDataPdu::RefreshRectangle(RefreshRectanglePdu { areas_to_refresh })] => {
                assert_eq!(a, &plein);
                assert_eq!(areas_to_refresh.as_slice(), [plein]);
            }
            autre => panic!("reprise inattendue : {autre:?}"),
        }
        // Sans Refresh Rect annoncé, la reprise seule.
        let sans = DisplayUpdateSupport {
            suppress_output: true,
            refresh_rect: false,
        };
        assert_eq!(pdus_de(Ordre::Reprendre, 1920, 1080, sans).len(), 1);
    }
}
