//! Mots de passe mémorisés, hôtes déclarés, verrous clavier, ouverture externe.

use super::{find_host, Target};
use avash::secrets::Zeroizing;
use avash::SshHost;
use std::sync::atomic::{AtomicBool, Ordering};

/// Mémorise un mot de passe dans le trousseau du système.
///
/// Jamais dans `~/.ssh/config` : ce fichier est en clair. Le trousseau
/// (`KWallet`, GNOME Keyring, Credential Manager, Trousseau macOS) gère le
/// chiffrement, le déverrouillage et la révocation.
/// Utilisateur effectif d'un hote pour la cle du trousseau.
///
/// Doit correspondre EXACTEMENT a ce que `Target::from_alias` utilise pour
/// *relire* le mot de passe : un hote sans directive `User` retombe sur
/// l'utilisateur courant. Sans cette resolution commune, un mot de passe
/// enregistre sous une cle et relu sous une autre ne serait jamais retrouve
/// (bug : « mémoriser » cassé pour tout hote sans `User`).
pub(crate) fn effective_user(user: Option<String>) -> String {
    user.map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
        .unwrap_or_else(avash::ssh::current_username)
}

/// Un alias autre que `alias_exclu` résout-il encore vers cet identifiant de
/// trousseau ?
///
/// L'identifiant dérive de `user@hôte:port`, jamais de l'alias : deux alias
/// vers le même serveur (motif courant, `web` et `web-via-bastion` avec un
/// `ProxyJump`) partagent l'entrée du trousseau, ce qui est voulu. La
/// résolution reproduit EXACTEMENT `Target::from_alias` (hostname sinon
/// l'alias, port 22 par défaut, utilisateur courant faute de `User`) : la
/// relire autrement retomberait sur le même décalage save/relit que corrige
/// `effective_user`.
/// Trouvé par l'audit du 7 septembre 2026 : supprimer ou déplacer un alias
/// partagé effaçait le mot de passe de l'autre, redemandé « sans explication ».
///
/// `conf` est la configuration déjà lue par l'appelant (contrat K3 de l'audit
/// du 12 septembre 2026, C-perf-5) : chaque alias la relisait et la
/// réanalysait, en O(n²) sur `host_delete` et `host_update`.
#[must_use]
pub(crate) fn identifiant_encore_utilise(
    conf: &str,
    hotes: &[SshHost],
    alias_exclu: &str,
    id: &str,
) -> bool {
    hotes.iter().filter(|h| h.alias != alias_exclu).any(|h| {
        // On résout chaque alias comme `Target::from_alias` (blocs à motif
        // compris) : depuis que `Host *` peut poser `User`, un alias sans `User`
        // littéral partage l'entrée `adrien@…` héritée, pas `courant@…`. S'en
        // tenir aux champs littéraux retomberait sur le décalage save/relit.
        let resolu = avash::resoudre_hote_dans(conf, &h.alias).unwrap_or_else(|| h.clone());
        let (addr, port, user) = super::cible_de(&resolu);
        avash::secrets::account_id(&user, &addr, port) == id
    })
}

/// Décide, quand l'identifiant de trousseau d'un hôte change, s'il faut copier
/// le secret vers le nouvel identifiant et/ou oublier l'ancien.
///
/// `partage_ancien` : un autre alias résout-il encore vers l'ancien identifiant ?
/// `nouveau_occupe` : un mot de passe est-il DÉJÀ mémorisé pour le nouveau ?
///
/// Le secret est indexé par `user@hôte:port`, jamais par alias : il appartient
/// à la cible. Trouvé par l'audit du 7 septembre 2026 (scénario 2 du constat) :
/// repointer `web` (10.0.0.1) vers 10.0.0.2 où `db` avait mémorisé son mot de
/// passe l'écrasait, et `db` ne se connectait plus. On ne copie donc que si la
/// cible est LIBRE (sinon on garde le secret de la cible, correct pour elle), et
/// on n'oublie l'ancien que si on a effectivement copié ailleurs ET qu'aucun
/// autre alias ne le partage (sinon on perdrait le secret d'un jumeau).
#[must_use]
pub(crate) fn plan_deplacement(partage_ancien: bool, nouveau_occupe: bool) -> (bool, bool) {
    let copier = !nouveau_occupe;
    let oublier = copier && !partage_ancien;
    (copier, oublier)
}

/// Exécute un travail bloquant (trousseau, disque, processus) sur le pool
/// bloquant de tokio et rend son résultat.
///
/// Trouvé par l'audit du 12 septembre 2026 (C-SIL-7, C-perf-2) : une commande
/// Tauri synchrone s'exécute sur le fil principal, celui qui peint la fenêtre.
/// Pendant que `KWallet` attendait son mot de passe, Avash ne répondait plus
/// (redimensionnement, croix, sortie des autres onglets). `spawn_blocking`
/// plutôt que `command(async)` seul : ce dernier déplace l'appel sur un fil de
/// travail tokio, qui resterait bloqué d'autant.
pub(crate) async fn bloquant<T: Send + 'static>(
    travail: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(travail)
        .await
        .map_err(|e| format!("Tâche interrompue : {e}"))?
}

/// Le trousseau a-t-il déjà été signalé en panne pendant ce lancement ?
/// Géré par l'application : un avertissement par lancement, pas un par
/// connexion (contrat K1 de l'audit du 12 septembre 2026).
#[derive(Default)]
pub struct TrousseauSignale(AtomicBool);

/// Prévient le front, une fois par lancement, que le trousseau ne répond pas :
/// les mots de passe mémorisés seront redemandés, et l'utilisateur sait
/// pourquoi. Trouvé par l'audit du 12 septembre 2026 (C-SIL-8) : toute erreur
/// du trousseau valait « pas de mot de passe », sans un mot.
pub(crate) fn signaler_trousseau_indisponible<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    erreur: &str,
) {
    use tauri::Manager as _;
    tracing::warn!("trousseau indisponible : {erreur}");
    let deja = app
        .try_state::<TrousseauSignale>()
        .is_some_and(|s| s.0.swap(true, Ordering::AcqRel));
    if !deja {
        super::emettre(
            app,
            "trousseau-indisponible",
            serde_json::json!({ "message": message_trousseau(erreur) }),
        );
    }
}

/// Ce que l'utilisateur lit quand le trousseau ne répond pas.
fn message_trousseau(erreur: &str) -> String {
    format!(
        "Le trousseau du système ne répond pas ({erreur}). \
         Les mots de passe mémorisés ne peuvent pas être relus : saisis-les."
    )
}

/// Relit un secret du trousseau hors des fils du runtime : un portefeuille
/// verrouillé fait attendre l'appel jusqu'à la réponse de l'utilisateur, ce
/// qui ne doit occuper ni le fil de la fenêtre ni un fil de travail tokio.
/// Audit du 12 septembre 2026 (C-SIL-7, C-perf-2).
///
/// `Ok(None)` : aucune entrée. `Err` : le trousseau est en panne ; l'événement
/// `trousseau-indisponible` est parti (une fois par lancement) et le message
/// demande la saisie (contrat K1).
pub(crate) async fn charger_secret<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    compte: String,
) -> Result<Option<Zeroizing<String>>, String> {
    let lu = bloquant(move || Ok(avash::secrets::charger(&compte))).await?;
    lu.map_err(|e| {
        let m = format!("{e:#}");
        signaler_trousseau_indisponible(app, &m);
        message_trousseau(&m)
    })
}

#[tauri::command]
pub async fn password_save(
    addr: String,
    port: Option<u16>,
    user: Option<String>,
    password: String,
) -> Result<(), String> {
    let password = Zeroizing::new(password);
    bloquant(move || {
        let id = avash::secrets::account_id(&effective_user(user), addr.trim(), port.unwrap_or(22));
        avash::secrets::save(&id, &password).map_err(|e| format!("{e:#}"))
    })
    .await
}

/// Oublie la clé d'hôte mémorisée (`known_hosts`) après un changement légitime.
/// Le prochain contact réapprend la nouvelle clé (TOFU).
#[tauri::command(async)]
pub fn known_hosts_forget(addr: String, port: Option<u16>) -> Result<usize, String> {
    avash::ssh::forget_host_key(addr.trim(), port.unwrap_or(22)).map_err(|e| format!("{e:#}"))
}

/// Oublie un mot de passe mémorisé.
#[tauri::command]
pub async fn password_forget(
    addr: String,
    port: Option<u16>,
    user: Option<String>,
) -> Result<(), String> {
    bloquant(move || {
        let id = avash::secrets::account_id(&effective_user(user), addr.trim(), port.unwrap_or(22));
        avash::secrets::forget(&id).map_err(|e| format!("{e:#}"))
    })
    .await
}

/// Un mot de passe est-il déjà mémorisé pour cet hôte ?
///
/// Un trousseau en panne répond « non » et le signale (contrat K1).
#[tauri::command]
pub async fn password_known<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    addr: String,
    port: Option<u16>,
    user: Option<String>,
) -> bool {
    let id = avash::secrets::account_id(&effective_user(user), addr.trim(), port.unwrap_or(22));
    matches!(charger_secret(&app, id).await, Ok(Some(_)))
}

/// Ouvre une URL dans le navigateur du système, jamais dans la webview.
///
/// Un lien cliquable du terminal ne doit pas naviguer dans la fenêtre Avash :
/// celle-ci a accès à `invoke`. On délègue au système, et on n'ouvre que des
/// schémas sûrs.
///
/// `open::that` attend le lanceur du système (`xdg-open`), qui peut bloquer :
/// hors du fil principal (audit du 12 septembre 2026, C-SIL-7).
#[tauri::command]
pub async fn open_external(url: String) -> Result<(), String> {
    bloquant(move || ouvrir_url(&url)).await
}

fn ouvrir_url(url: &str) -> Result<(), String> {
    let url = url.trim();
    // Whitelist stricte : ni file://, ni javascript:, ni schéma inconnu.
    let ok = ["http://", "https://", "mailto:", "ftp://"]
        .iter()
        .any(|p| url.starts_with(p));
    if !ok {
        return Err(format!("Schéma d'URL non autorisé : {url}"));
    }
    open::that(url).map_err(|e| format!("Ouverture impossible : {e}"))
}

/// Supprime un hôte de `~/.ssh/config` et oublie son mot de passe mémorisé.
#[tauri::command]
pub async fn host_delete(alias: String) -> Result<(), String> {
    bloquant(move || supprimer_hote(&alias)).await
}

fn supprimer_hote(alias: &str) -> Result<(), String> {
    // On résout la cible AVANT de supprimer (après, l'hôte n'existe plus et on
    // ne saurait plus quel identifiant du trousseau oublier)... mais on n'oublie
    // le secret qu'APRÈS le succès de la suppression. Dans l'autre ordre, un
    // hôte déclaré via `Include` — que remove_host ne sait pas retirer — faisait
    // perdre le mot de passe alors que l'hôte restait en place.
    let identifiant = avash::configuration_resolue()
        .ok()
        .and_then(|conf| Target::identifiant(&conf, alias));
    avash::remove_host(alias).map_err(|e| format!("{e:#}"))?;
    if let Some(id) = identifiant {
        // Un autre alias peut résoudre vers le même identifiant de trousseau
        // (deux alias vers user@hôte:port partagent l'entrée). On ne l'oublie
        // que si plus aucun alias ne le réclame. Trouvé par l'audit du
        // 7 septembre 2026 : supprimer `web-via-bastion` effaçait le mot de
        // passe de `web`, redemandé sans explication à la connexion suivante.
        let conf = avash::configuration_resolue().unwrap_or_default();
        let hotes = avash::parse_config_str(&conf);
        if !identifiant_encore_utilise(&conf, &hotes, alias, &id) {
            if let Err(e) = avash::secrets::forget(&id) {
                // L'hôte est supprimé ; le secret orphelin reste au trousseau.
                // Le journal le garde (audit du 12 septembre 2026, C-SIL-2).
                tracing::warn!("secret orphelin non oublié après suppression d'un hôte : {e:#}");
            }
        }
    }
    Ok(())
}

/// Renvoie les champs d'un hôte pour pré-remplir le formulaire d'édition.
#[tauri::command(async)]
pub fn host_get(alias: String) -> Result<SshHost, String> {
    find_host(&alias)
}

/// Modifie un hôte enregistré. Si l'alias change, le mot de passe mémorisé
/// est déplacé vers le nouvel identifiant.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn host_update(
    old_alias: String,
    alias: String,
    addr: String,
    port: Option<u16>,
    user: Option<String>,
    key_path: Option<String>,
    proxy_jump: Option<String>,
    tags: Option<String>,
    folder: Option<String>,
) -> Result<(), String> {
    let host = SshHost {
        alias: alias.trim().to_string(),
        hostname: Some(addr.trim().to_string()).filter(|a| !a.is_empty()),
        user: user.map(|u| u.trim().to_string()).filter(|u| !u.is_empty()),
        port,
        identity_file: key_path
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty()),
        proxy_jump: proxy_jump
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty()),
        tags: tags
            .unwrap_or_default()
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect(),
        folder: avash::folders::normalize(&folder.unwrap_or_default()),
    };
    bloquant(move || modifier_hote(old_alias.trim(), &host)).await
}

fn modifier_hote(old_alias: &str, host: &SshHost) -> Result<(), String> {
    // Identifiant du trousseau AVANT modification : il dérive de user@addr:port,
    // pas de l'alias. Changer l'adresse ou l'utilisateur d'un hôte laissait donc
    // le secret sous l'ancien identifiant — redemandé à chaque connexion, sans
    // explication, l'ancienne entrée restant orpheline dans le trousseau.
    let ancien = avash::configuration_resolue()
        .ok()
        .and_then(|conf| Target::identifiant(&conf, old_alias));

    avash::update_host(old_alias, host).map_err(|e| format!("{e:#}"))?;

    // Après le succès seulement : on déplace le secret vers le nouvel identifiant.
    let Some(ancien) = ancien else {
        return Ok(());
    };
    let conf = avash::configuration_resolue().unwrap_or_default();
    let Some(nouveau) = Target::identifiant(&conf, host.alias.trim()) else {
        return Ok(());
    };
    if nouveau == ancien {
        return Ok(());
    }
    // Contrat K1 : un trousseau en panne n'est plus confondu avec « rien à
    // déplacer ». L'hôte est modifié ; on dit que le secret n'a pas suivi.
    let pas_deplace = |e: &dyn std::fmt::Display| {
        format!("Hôte modifié, mais le mot de passe mémorisé n'a pas pu être déplacé : {e:#}")
    };
    let Some(secret) = avash::secrets::charger(&ancien).map_err(|e| pas_deplace(&e))? else {
        return Ok(());
    };
    // La cible peut DÉJÀ porter un mot de passe : repointer un hôte vers un
    // serveur où un autre hôte a mémorisé le sien ne doit pas l'écraser (le
    // secret est indexé par user@hôte:port, il appartient à la cible). Et
    // l'ancien ne s'oublie que si aucun autre alias ne le partage : deux alias
    // vers le même serveur le partagent, changer le port de l'un ne doit pas
    // l'effacer pour l'autre. Trouvé par l'audit du 7 septembre 2026
    // (scénarios 1 et 2 du constat).
    let nouveau_occupe = avash::secrets::charger(&nouveau)
        .map_err(|e| pas_deplace(&e))?
        .is_some();
    let hotes = avash::parse_config_str(&conf);
    let partage_ancien = identifiant_encore_utilise(&conf, &hotes, host.alias.trim(), &ancien);
    let (copier, oublier) = plan_deplacement(partage_ancien, nouveau_occupe);
    if copier {
        // L'oubli n'a lieu qu'après une écriture réussie. Sinon — trousseau
        // verrouillé, D-Bus absent — la nouvelle entrée n'existait pas,
        // l'ancienne était quand même effacée, et `host_update` renvoyait Ok :
        // le mot de passe était perdu sans un mot, pour un simple changement de
        // port.
        avash::secrets::save(&nouveau, &secret)
            .map_err(|e| format!("Le mot de passe mémorisé n'a pas pu être déplacé : {e:#}"))?;
        if oublier {
            if let Err(e) = avash::secrets::forget(&ancien) {
                tracing::warn!("ancien secret non oublié après déplacement : {e:#}");
            }
        }
    }
    Ok(())
}
