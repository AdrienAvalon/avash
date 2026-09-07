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
    let mut cibles: Vec<(String, String, u16)> = Vec::new();
    // Trouvé par l'audit du 7 septembre 2026 : un `unwrap_or_default()` ici
    // affichait une santé « aucun hôte » quand `~/.ssh/config` était illisible
    // (non UTF-8) au lieu de dire pourquoi. On propage l'erreur.
    for h in avash::parse_ssh_config().map_err(|e| format!("{e:#}"))? {
        // Résolu (blocs à motif compris) : un `Host web.interne` qui hérite d'un
        // `ProxyJump` de `Host *.interne` ne doit pas être sondé en direct — ce
        // n'est pas lui qu'on joindrait. Trouvé par l'audit du 7 septembre 2026 :
        // sans résolution, il était contacté directement, donc dit injoignable.
        let h = avash::resoudre_hote(&h.alias).unwrap_or(h);
        if h.proxy_jump
            .as_deref()
            .is_some_and(|p| !p.trim().is_empty() && !p.eq_ignore_ascii_case("none"))
        {
            continue;
        }
        let hote = h.hostname.clone().unwrap_or_else(|| h.alias.clone());
        cibles.push((format!("ssh:{}", h.alias), hote, h.port.unwrap_or(22)));
    }
    for r in avash::rdphost::load_hosts().unwrap_or_default() {
        cibles.push((format!("rdp:{}", r.id), r.host.clone(), r.port));
    }
    // Au plus seize sondes à la fois : une liste de deux cents hôtes ne doit
    // pas ouvrir deux cents connexions d'un coup.
    let verrou = std::sync::Arc::new(tokio::sync::Semaphore::new(16));
    let mut sondes = tokio::task::JoinSet::new();
    for (cle, hote, port) in cibles {
        let verrou = verrou.clone();
        sondes.spawn(async move {
            let _jeton = verrou.acquire().await;
            let sante = avash::sante::sonder(&hote, port, avash::sante::DELAI_DEFAUT).await;
            SanteHote { cle, sante }
        });
    }
    let mut resultats = Vec::new();
    while let Some(r) = sondes.join_next().await {
        if let Ok(s) = r {
            resultats.push(s);
        }
    }
    Ok(resultats)
}

#[cfg(test)]
mod tests_sante {
    use super::hosts_health;
    use crate::commands::tests::with_ssh_config;

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
