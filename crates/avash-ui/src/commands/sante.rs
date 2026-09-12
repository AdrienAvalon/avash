//! Santé des hôtes : sonde TCP sans session.

/// L'état d'un hôte de la liste, sondé sans ouvrir de session.
#[derive(Debug, serde::Serialize)]
pub struct SanteHote {
    /// La clé de la ligne : `ssh:<alias>` ou `rdp:<id>`.
    pub cle: String,
    pub sante: avash::sante::Sante,
}

/// Sonde tous les hôtes déclarés (SSH et RDP) en parallèle, chacun borné à
/// `DELAI_DEFAUT`. Un hôte derrière un rebond n'est pas sondé : ce n'est pas
/// lui qu'on joindrait en direct, et le résultat ne dirait rien de lui.
#[tauri::command]
pub async fn hosts_health() -> Result<Vec<SanteHote>, String> {
    // Configuration et bureaux se lisent hors des fils du runtime (C-SIL-7).
    let cibles = super::bloquant(cibles_de_sonde).await?;
    Ok(sonder_toutes(cibles, |hote, port| async move {
        avash::sante::sonder(&hote, port, avash::sante::DELAI_DEFAUT).await
    })
    .await)
}

/// Les cibles à sonder, lues sur le disque.
fn cibles_de_sonde() -> Result<Vec<(String, String, u16)>, String> {
    // Trouvé par l'audit du 7 septembre 2026 : un `unwrap_or_default()` ici
    // affichait une santé « aucun hôte » quand `~/.ssh/config` était illisible
    // (non UTF-8) au lieu de dire pourquoi. On propage l'erreur.
    let conf = avash::configuration_resolue().map_err(|e| format!("{e:#}"))?;
    let bureaux = avash::rdphost::load_hosts().unwrap_or_else(|e| {
        tracing::warn!("bureaux illisibles pour la sonde de santé : {e:#}");
        Vec::new()
    });
    Ok(cibles_dans(&conf, &bureaux))
}

/// Les cibles d'une sonde (clé de ligne, hôte, port), d'après une
/// configuration déjà lue. Pure depuis l'audit du 12 septembre 2026 (contrat
/// K3, C-perf-5) : chaque hôte relisait et réanalysait le fichier et ses
/// `Include` ; la pureté interdit la relecture par construction.
pub(crate) fn cibles_dans(
    conf: &str,
    bureaux: &[avash::rdphost::RdpHost],
) -> Vec<(String, String, u16)> {
    let mut cibles = Vec::new();
    for h in avash::parse_config_str(conf) {
        // Résolu (blocs à motif compris) : un `Host web.interne` qui hérite d'un
        // `ProxyJump` de `Host *.interne` ne doit pas être sondé en direct — ce
        // n'est pas lui qu'on joindrait. Trouvé par l'audit du 7 septembre 2026 :
        // sans résolution, il était contacté directement, donc dit injoignable.
        let h = avash::resoudre_hote_dans(conf, &h.alias).unwrap_or(h);
        if h.proxy_jump
            .as_deref()
            .is_some_and(|p| !p.trim().is_empty() && !p.eq_ignore_ascii_case("none"))
        {
            continue;
        }
        let hote = h.hostname.clone().unwrap_or_else(|| h.alias.clone());
        cibles.push((format!("ssh:{}", h.alias), hote, h.port.unwrap_or(22)));
    }
    for r in bureaux {
        cibles.push((format!("rdp:{}", r.id), r.host.clone(), r.port));
    }
    cibles
}

/// Sonde les cibles, seize à la fois au plus, avec `sonde` (injectée pour les
/// tests).
pub(crate) async fn sonder_toutes<F, Fut>(
    cibles: Vec<(String, String, u16)>,
    sonde: F,
) -> Vec<SanteHote>
where
    F: Fn(String, u16) -> Fut + Clone + Send + 'static,
    Fut: std::future::Future<Output = avash::sante::Sante> + Send + 'static,
{
    // Au plus seize sondes à la fois : une liste de deux cents hôtes ne doit
    // pas ouvrir deux cents connexions d'un coup.
    let verrou = std::sync::Arc::new(tokio::sync::Semaphore::new(16));
    let mut sondes = tokio::task::JoinSet::new();
    let mut cles = std::collections::HashMap::new();
    for (cle, hote, port) in cibles {
        let verrou = verrou.clone();
        let sonde = sonde.clone();
        let nom = cle.clone();
        let tache = sondes.spawn(async move {
            let _jeton = verrou.acquire().await;
            let sante = sonde(hote, port).await;
            SanteHote { cle, sante }
        });
        cles.insert(tache.id(), nom);
    }
    let mut resultats = Vec::new();
    while let Some(r) = sondes.join_next_with_id().await {
        match r {
            Ok((_, s)) => resultats.push(s),
            // Trouvé par l'audit du 12 septembre 2026 (C-SIL-13) : une sonde qui
            // paniquait disparaissait du résultat, et l'hôte perdait son voyant
            // sans raison. Elle rend désormais une ligne d'erreur, et le journal
            // garde la cause.
            Err(e) => {
                let cle = cles.remove(&e.id()).unwrap_or_default();
                tracing::error!("sonde de santé interrompue pour {cle} : {e}");
                resultats.push(SanteHote {
                    cle,
                    sante: avash::sante::Sante::Injoignable {
                        raison: format!("sonde interrompue : {e}"),
                    },
                });
            }
        }
    }
    resultats
}

#[cfg(test)]
mod tests_sante {
    use super::{cibles_dans, hosts_health, sonder_toutes};
    use crate::commands::tests::with_ssh_config;

    /// Audit du 12 septembre 2026 (C-SIL-13) : une sonde qui paniquait
    /// disparaissait du résultat (`if let Ok(s) = r`), et l'hôte perdait son
    /// voyant sans raison. Elle doit rendre une ligne d'erreur, nommée.
    #[tokio::test]
    async fn une_sonde_qui_panique_donne_un_resultat_d_erreur() {
        let cibles = vec![
            ("ssh:a".to_owned(), "h".to_owned(), 22),
            ("ssh:b".to_owned(), "h".to_owned(), 23),
        ];
        let mut r = sonder_toutes(cibles, |_hote, port| async move {
            assert!(port != 23, "sonde en panne");
            avash::sante::Sante::Joignable { latence_ms: 1 }
        })
        .await;
        r.sort_by(|a, b| a.cle.cmp(&b.cle));
        let cles: Vec<&str> = r.iter().map(|s| s.cle.as_str()).collect();
        assert_eq!(cles, ["ssh:a", "ssh:b"], "{r:?}");
        assert!(
            matches!(&r[1].sante, avash::sante::Sante::Injoignable { raison } if raison.contains("interrompue")),
            "{r:?}"
        );
    }

    /// Contrat K3 : les cibles se calculent sur la configuration déjà lue ;
    /// l'hôte qui hérite d'un rebond par motif n'y figure pas.
    #[test]
    fn les_cibles_se_calculent_sur_la_configuration_deja_lue() {
        let conf = "Host *.interne\n  ProxyJump bastion\n\nHost web.interne\n  HostName 10.0.0.2\n\nHost db\n  HostName 10.0.0.3\n  Port 2222\n";
        let mut bureau = avash::rdphost::RdpHost::new("b", "10.0.0.4", 3389, "u", 800, 600);
        bureau.id = "b1".into();
        let cibles = cibles_dans(conf, &[bureau]);
        assert_eq!(
            cibles,
            vec![
                ("ssh:db".to_owned(), "10.0.0.3".to_owned(), 2222),
                ("rdp:b1".to_owned(), "10.0.0.4".to_owned(), 3389),
            ]
        );
    }

    /// Un hôte qui écoute est joignable, un port muet ne l'est pas, un hôte
    /// derrière un rebond n'est pas sondé.
    #[tokio::test]
    async fn la_sante_des_hotes_declares_est_sondee_sauf_derriere_un_rebond() {
        let ecoute = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = ecoute.local_addr().unwrap().port();
        let ferme = {
            let e = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            e.local_addr().unwrap().port()
        };
        let _g = with_ssh_config(&format!(
            "Host vivant\n  HostName 127.0.0.1\n  Port {port}\n\nHost muet\n  HostName 127.0.0.1\n  Port {ferme}\n\nHost cache\n  HostName 10.0.0.9\n  ProxyJump vivant\n"
        ));
        let mut r = hosts_health().await.unwrap();
        r.sort_by(|a, b| a.cle.cmp(&b.cle));
        let cles: Vec<&str> = r.iter().map(|s| s.cle.as_str()).collect();
        assert_eq!(cles, vec!["ssh:muet", "ssh:vivant"], "{r:?}");
        assert!(
            matches!(r[0].sante, avash::sante::Sante::Injoignable { .. }),
            "{r:?}"
        );
        assert!(
            matches!(r[1].sante, avash::sante::Sante::Joignable { .. }),
            "{r:?}"
        );
    }

    /// Un hôte qui hérite d'un `ProxyJump` d'un bloc à motif (`Host *.interne`)
    /// n'est pas sondé en direct. Trouvé par l'audit du 7 septembre 2026 :
    /// `web.interne` n'a pas de `ProxyJump` littéral, la sonde le contactait
    /// donc directement — un hôte joignable seulement à travers le bastion.
    #[tokio::test]
    async fn un_hote_derriere_un_rebond_herite_du_motif_n_est_pas_sonde() {
        let _g = with_ssh_config(
            "Host *.interne\n  ProxyJump bastion\n\nHost web.interne\n  HostName 127.0.0.1\n",
        );
        let r = hosts_health().await.unwrap();
        let cles: Vec<&str> = r.iter().map(|s| s.cle.as_str()).collect();
        assert!(
            !cles.contains(&"ssh:web.interne"),
            "web.interne hérite d'un ProxyJump : ne pas le sonder en direct ({cles:?})"
        );
    }

    /// Trouvé par l'audit du 7 septembre 2026 : un `unwrap_or_default()` rendait
    /// une santé « aucun hôte » quand `~/.ssh/config` était illisible (non
    /// UTF-8) au lieu de dire pourquoi. La commande doit propager l'erreur.
    #[tokio::test]
    async fn un_config_non_lisible_donne_une_erreur_pas_une_liste_vide() {
        let _g = with_ssh_config("");
        let path = avash::repertoire_personnel()
            .unwrap()
            .join(".ssh")
            .join("config");
        std::fs::write(&path, b"# R\xe9seau\nHost a\n  HostName 10.0.0.1\n").unwrap();
        let e = hosts_health().await.unwrap_err();
        assert!(e.contains("Impossible de lire"), "message inattendu : {e}");
    }
}
