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
    for h in avash::parse_ssh_config().unwrap_or_default() {
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
}
