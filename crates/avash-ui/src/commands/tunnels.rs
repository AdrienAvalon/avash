//! Tunnels SSH : définitions, démarrage, arrêt, état.

use super::{find_host, Target};
use avash::ssh::AvashSession;
use avash::tunnel::{Tunnel, TunnelDef, TunnelKind, TunnelSnapshot};
use avash::Verrou as _;
use std::collections::HashMap;
use std::sync::Mutex;

/// Tunnels ouverts, par identifiant de definition. Independants des onglets.
#[derive(Default)]
pub struct TunnelStore {
    pub inner: Mutex<HashMap<String, Tunnel>>,
    /// Tunnels dont l'ouverture est en cours, et ceux qu'on a arrêtés pendant.
    ///
    /// Entre le retrait du précédent et l'insertion du nouveau, le magasin ne
    /// contenait rien pour cet identifiant — pendant plusieurs secondes de
    /// connexion. Deux clics sur « relancer » passaient donc tous deux, et le
    /// perdant était **écrasé sans `close()`** : sa socket d'écoute et sa tâche
    /// survivaient, invisibles de l'interface. Symétriquement, un arrêt demandé
    /// pendant la connexion ne trouvait rien à retirer, et le tunnel s'installait
    /// quand même. C'est le défaut déjà corrigé pour les sessions SSH.
    pub en_cours: Mutex<std::collections::HashSet<String>>,
    pub annules: Mutex<std::collections::HashSet<String>>,
}

/// Etat d'un tunnel ouvert, tel que l'interface l'affiche.
#[derive(serde::Serialize)]
pub struct TunnelStatus {
    pub id: String,
    #[serde(flatten)]
    pub snapshot: TunnelSnapshot,
}

pub(crate) fn parse_kind(kind: &str) -> Result<TunnelKind, String> {
    match kind {
        "local" => Ok(TunnelKind::Local),
        "remote" => Ok(TunnelKind::Remote),
        "dynamic" => Ok(TunnelKind::Dynamic),
        other => Err(format!("Type de tunnel inconnu : {other}")),
    }
}

/// Definitions enregistrees, tous hotes confondus.
#[tauri::command(async)]
pub fn tunnel_defs() -> Result<Vec<TunnelDef>, String> {
    avash::tunnel::load_defs().map_err(|e| e.to_string())
}

/// Cree (`id` absent) ou modifie une definition.
#[allow(clippy::too_many_arguments)]
#[tauri::command(async)]
pub fn tunnel_def_save(
    id: Option<String>,
    alias: String,
    kind: String,
    bind_port: u16,
    target_host: Option<String>,
    target_port: Option<u16>,
    name: Option<String>,
) -> Result<TunnelDef, String> {
    let kind = parse_kind(&kind)?;
    // L'hote doit exister : un tunnel vers un alias fantome echouerait plus
    // tard avec un message moins clair.
    find_host(&alias)?;
    let mut def = TunnelDef::new(
        &alias,
        kind,
        bind_port,
        target_host.as_deref().unwrap_or(""),
        target_port.unwrap_or(0),
        name.as_deref().unwrap_or(""),
    );
    if let Some(id) = id.filter(|i| !i.is_empty()) {
        def.id = id;
    }
    avash::tunnel::upsert_def_in(&avash::tunnel::defs_path(), def.clone())
        .map_err(|e| e.to_string())?;
    Ok(def)
}

/// Supprime une definition ; ferme d'abord le tunnel s'il tourne.
#[tauri::command]
pub async fn tunnel_def_delete(
    tunnels: tauri::State<'_, TunnelStore>,
    id: String,
) -> Result<(), String> {
    let running = tunnels.inner.verrou().remove(&id);
    if let Some(t) = running {
        t.close().await;
    }
    avash::tunnel::remove_def_in(&avash::tunnel::defs_path(), &id).map_err(|e| e.to_string())?;
    Ok(())
}

/// Ouvre un tunnel. `password` suit la meme convention que `pty_open` : le
/// marqueur `PASSWORD_REQUIRED` dans l'erreur invite l'interface a le
/// demander puis a reessayer.
#[tauri::command]
pub async fn tunnel_start<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    tunnels: tauri::State<'_, TunnelStore>,
    id: String,
    password: Option<String>,
) -> Result<TunnelStatus, String> {
    // Définitions (disque) et cible (trousseau) hors des fils du runtime.
    let cherche = id.clone();
    let (def, mut target, panne) = super::bloquant(move || {
        let def = avash::tunnel::load_defs()
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|d| d.id == cherche)
            .ok_or_else(|| format!("Tunnel inconnu : {cherche}"))?;
        let (target, panne) = Target::depuis_alias(&def.alias)?;
        Ok((def, target, panne))
    })
    .await?;
    if let Some(m) = panne {
        super::signaler_trousseau_indisponible(&app, &m);
    }
    target.override_password(password);
    // Un tunnel deja ouvert (ou mort) sous cet id est remplace : c'est le
    // geste « relancer » de l'interface.
    let previous = {
        let mut inner = tunnels.inner.verrou();
        tunnels.en_cours.verrou().insert(id.clone());
        tunnels.annules.verrou().remove(&id);
        inner.remove(&id)
    };
    if let Some(t) = previous {
        t.close().await;
    }
    // Par les rebonds, comme tous les autres chemins (open_on_target, run_command,
    // sftp_of) : un hôte en ProxyJump n'est pas joignable en direct, et l'erreur
    // renvoyée ne mentionnait même pas le bastion.
    let session =
        AvashSession::connect_via(&target.jumps, &target.addr, target.port, &target.auth())
            .await
            .map_err(|e| e.to_string())?;
    let tunnel = Tunnel::open(session, def)
        .await
        .map_err(|e| e.to_string())?;
    let snapshot = tunnel.snapshot();
    // Arrêt demandé pendant la connexion : on ferme ce qu'on vient d'ouvrir
    // plutôt que de l'installer contre la volonté de l'utilisateur. Et un
    // évincé — deux « relancer » simultanés — est fermé, pas seulement lâché.
    let evince = {
        let mut inner = tunnels.inner.verrou();
        tunnels.en_cours.verrou().remove(&id);
        if tunnels.annules.verrou().remove(&id) {
            Some(tunnel) // arrêté entre-temps : à fermer, pas à installer
        } else {
            inner.insert(id.clone(), tunnel)
        }
    };
    if let Some(t) = evince {
        t.close().await;
        if !tunnels.inner.verrou().contains_key(&id) {
            return Err("Tunnel arrêté pendant l'ouverture.".to_owned());
        }
    }
    Ok(TunnelStatus { id, snapshot })
}

#[tauri::command]
pub async fn tunnel_stop(tunnels: tauri::State<'_, TunnelStore>, id: String) -> Result<(), String> {
    let t = {
        let mut inner = tunnels.inner.verrou();
        let t = inner.remove(&id);
        // Rien à retirer alors qu'une ouverture est en cours : on note l'arrêt
        // pour que `tunnel_start` le voie en arrivant, au lieu d'installer un
        // tunnel que l'utilisateur vient d'arrêter.
        if t.is_none() && tunnels.en_cours.verrou().contains(&id) {
            tunnels.annules.verrou().insert(id.clone());
        }
        t
    };
    if let Some(t) = t {
        t.close().await;
    }
    Ok(())
}

/// Etat de tous les tunnels ouverts (les morts inclus, marques `alive: false`,
/// jusqu'a ce que l'utilisateur les relance ou les arrete).
#[tauri::command]
#[must_use]
pub fn tunnel_status(tunnels: tauri::State<'_, TunnelStore>) -> Vec<TunnelStatus> {
    tunnels
        .inner
        .verrou()
        .iter()
        .map(|(id, t)| TunnelStatus {
            id: id.clone(),
            snapshot: t.snapshot(),
        })
        .collect()
}
