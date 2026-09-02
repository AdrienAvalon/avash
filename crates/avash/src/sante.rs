//! Santé des hôtes : un serveur est-il joignable, sans ouvrir de session ?
//!
//! Une connexion TCP jusqu'au port SSH ou RDP, bornée dans le temps, puis
//! refermée aussitôt : pas d'authentification, pas de bannière lue. C'est ce
//! que voit un `nc -z`, ni plus ni moins — mais depuis la liste, sans rien
//! taper. Un hôte derrière un rebond n'est pas sondé : ce n'est pas lui qu'on
//! joindrait en direct.

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
/// il est absent — et une liste de cinquante hôtes ne doit pas figer.
pub const DELAI_DEFAUT: Duration = Duration::from_millis(1500);

/// Sonde `hote:port`. Le nom est résolu ici ; une adresse littérale passe
/// telle quelle.
pub async fn sonder(hote: &str, port: u16, delai: Duration) -> Sante {
    let depart = Instant::now();
    let adresses = match tokio::net::lookup_host((hote, port)).await {
        Ok(a) => a.collect::<Vec<_>>(),
        Err(e) => {
            return Sante::Inconnu {
                raison: e.to_string(),
            }
        }
    };
    if adresses.is_empty() {
        return Sante::Inconnu {
            raison: "aucune adresse".into(),
        };
    }
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

    #[test]
    fn la_sante_se_serialise_avec_son_etat_en_clair() {
        let j = serde_json::to_string(&Sante::Joignable { latence_ms: 12 }).unwrap();
        assert_eq!(j, r#"{"etat":"joignable","latence_ms":12}"#);
    }
}
