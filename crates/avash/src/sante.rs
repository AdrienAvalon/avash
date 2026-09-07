//! Santé des hôtes : un serveur est-il joignable, sans ouvrir de session ?
//!
//! Une connexion TCP jusqu'au port SSH ou RDP, bornée dans le temps, puis
//! refermée aussitôt : pas d'authentification, pas de bannière lue. C'est ce
//! que voit un `nc -z`, ni plus ni moins — mais depuis la liste, sans rien
//! taper. Un hôte derrière un rebond n'est pas sondé : ce n'est pas lui qu'on
//! joindrait en direct.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// Ce qu'une sonde a vu.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "etat", rename_all = "lowercase")]
pub enum Sante {
    /// Le port a répondu, en autant de millisecondes.
    Joignable { latence_ms: u64 },
    /// Refus, délai dépassé, réseau absent : la raison en clair.
    Injoignable { raison: String },
    /// Le nom ne se résout pas.
    Inconnu { raison: String },
}

/// Délai par défaut : au-delà, un serveur qui ne répond pas n'est pas « lent »,
/// il est absent — et une liste de cinquante hôtes ne doit pas figer. Il borne
/// chaque phase (résolution du nom, puis connexion), soit un pire cas de deux
/// fois ce délai par hôte quand le DNS traîne sans jamais répondre.
pub const DELAI_DEFAUT: Duration = Duration::from_millis(1500);

/// Sonde `hote:port`. Le nom est résolu ici ; une adresse littérale passe
/// telle quelle.
pub async fn sonder(hote: &str, port: u16, delai: Duration) -> Sante {
    async fn resoudre(hote: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
        Ok(tokio::net::lookup_host((hote, port)).await?.collect())
    }
    sonder_avec(resoudre(hote, port), delai).await
}

/// Cœur testable de `sonder` : la résolution du nom est injectée, ce qui permet
/// de simuler un résolveur qui n'aboutit jamais (`lookup_host` prend un `&str`
/// et n'est pas simulable en l'état).
///
/// Trouvé par l'audit du 7 septembre 2026 : `sonder` bornait la connexion TCP
/// mais pas la résolution. `lookup_host` est `getaddrinfo` dans `spawn_blocking`,
/// et avec un résolveur injoignable (portail captif, VPN coupé) glibc attend
/// 5 s × 2 tentatives par serveur de noms (souvent 20 s ou plus) par hôte : la
/// liste figeait, aucun voyant ne s'allumait avant la dernière résolution. La
/// résolution est désormais bornée par `delai`. Elle a de plus son propre budget,
/// distinct de celui du `connect` (second chronomètre ci-dessous) : sinon une
/// résolution lente mangeait le budget de connexion et un hôte vivant derrière
/// un DNS lent sortait à tort « délai dépassé ». La latence affichée compte à
/// partir du `connect`, elle ne mélange plus le temps de résolution et le SYN/ACK.
///
/// Remarque : le `timeout` borne l'appelant mais n'annule pas le `getaddrinfo`
/// sous-jacent (le thread bloquant reste occupé jusqu'à la fin réelle) ; avec un
/// DNS mort et beaucoup d'hôtes déclarés par nom, le pool bloquant de tokio peut
/// se remplir. Un résolveur asynchrone (hickory) le règlerait ; ici on borne au
/// moins l'attente perçue.
async fn sonder_avec<R>(resolveur: R, delai: Duration) -> Sante
where
    R: std::future::Future<Output = std::io::Result<Vec<SocketAddr>>>,
{
    let adresses = match tokio::time::timeout(delai, resolveur).await {
        Err(_) => {
            return Sante::Inconnu {
                raison: "résolution du nom trop longue".into(),
            }
        }
        Ok(Err(e)) => {
            return Sante::Inconnu {
                raison: e.to_string(),
            }
        }
        Ok(Ok(a)) => a,
    };
    if adresses.is_empty() {
        return Sante::Inconnu {
            raison: "aucune adresse".into(),
        };
    }
    // Second chronomètre : la phase de connexion a son propre budget `delai` et
    // la latence se mesure à partir d'ici.
    let depart = Instant::now();
    let mut derniere = String::from("délai dépassé");
    for adresse in adresses {
        let restant = delai.saturating_sub(depart.elapsed());
        if restant.is_zero() {
            break;
        }
        match tokio::time::timeout(restant, tokio::net::TcpStream::connect(adresse)).await {
            Ok(Ok(_flux)) => {
                return Sante::Joignable {
                    latence_ms: u64::try_from(depart.elapsed().as_millis()).unwrap_or(u64::MAX),
                }
            }
            Ok(Err(e)) => derniere = e.to_string(),
            Err(_) => derniere = "délai dépassé".into(),
        }
    }
    Sante::Injoignable { raison: derniere }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn un_port_qui_ecoute_est_joignable_avec_sa_latence() {
        let ecoute = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = ecoute.local_addr().unwrap().port();
        let s = sonder("127.0.0.1", port, DELAI_DEFAUT).await;
        assert!(
            matches!(s, Sante::Joignable { latence_ms } if latence_ms < 1000),
            "{s:?}"
        );
    }

    #[tokio::test]
    async fn un_port_ferme_est_injoignable_avec_la_raison() {
        // Un port libéré à l'instant : personne n'écoute.
        let port = {
            let e = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            e.local_addr().unwrap().port()
        };
        let s = sonder("127.0.0.1", port, DELAI_DEFAUT).await;
        assert!(
            matches!(&s, Sante::Injoignable { raison } if !raison.is_empty()),
            "{s:?}"
        );
    }

    #[tokio::test]
    async fn un_nom_inconnu_est_dit_inconnu() {
        let s = sonder("hote-qui-n-existe-pas.invalid", 22, DELAI_DEFAUT).await;
        assert!(matches!(s, Sante::Inconnu { .. }), "{s:?}");
    }

    /// Une adresse non routable ne doit pas figer : le délai borne la sonde.
    #[tokio::test]
    async fn le_delai_borne_une_adresse_muette() {
        let depart = Instant::now();
        let s = sonder("192.0.2.1", 22, Duration::from_millis(300)).await;
        assert!(!matches!(s, Sante::Joignable { .. }), "{s:?}");
        assert!(
            depart.elapsed() < Duration::from_secs(3),
            "{:?}",
            depart.elapsed()
        );
    }

    /// Trouvé par l'audit du 7 septembre 2026 : la résolution du nom n'était pas
    /// bornée (`lookup_host` sans `timeout`). Un résolveur qui absorbe les paquets
    /// sans répondre (portail captif, VPN coupé) faisait attendre glibc 20 s ou
    /// plus par hôte et figeait la liste. Un résolveur qui n'aboutit jamais rend
    /// désormais `Inconnu` dans le budget imparti (bien avant deux fois le délai).
    #[tokio::test]
    async fn une_resolution_qui_ne_repond_pas_est_bornee() {
        let delai = Duration::from_millis(200);
        let depart = Instant::now();
        let s = sonder_avec(
            std::future::pending::<std::io::Result<Vec<SocketAddr>>>(),
            delai,
        )
        .await;
        assert!(matches!(s, Sante::Inconnu { .. }), "{s:?}");
        assert!(depart.elapsed() < 2 * delai, "{:?}", depart.elapsed());
    }

    /// Trouvé par l'audit du 7 septembre 2026 : la résolution mangeait le budget
    /// de connexion (chronomètre unique), si bien qu'un hôte vivant derrière un
    /// DNS lent — cas d'un réseau normal, sans portail captif — sortait à tort
    /// `Injoignable { "délai dépassé" }`. Les deux phases ayant chacune leur
    /// budget, une résolution qui prend presque tout le délai puis un port qui
    /// écoute rend bien `Joignable`.
    #[tokio::test]
    async fn une_resolution_lente_puis_un_hote_qui_ecoute_reste_joignable() {
        let ecoute = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let adresse = ecoute.local_addr().unwrap();
        let delai = Duration::from_millis(200);
        let resolveur = async move {
            // La résolution consomme presque tout le délai : avec l'ancien
            // chronomètre unique, il ne serait rien resté pour le `connect`.
            tokio::time::sleep(Duration::from_millis(190)).await;
            Ok(vec![adresse])
        };
        let s = sonder_avec(resolveur, delai).await;
        assert!(matches!(s, Sante::Joignable { .. }), "{s:?}");
    }

    #[test]
    fn la_sante_se_serialise_avec_son_etat_en_clair() {
        let j = serde_json::to_string(&Sante::Joignable { latence_ms: 12 }).unwrap();
        assert_eq!(j, r#"{"etat":"joignable","latence_ms":12}"#);
    }
}
