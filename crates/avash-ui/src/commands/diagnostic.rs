//! Export d'un diagnostic : ce qu'un ticket a besoin de savoir, et rien de ce
//! qu'il ne doit pas voir.
//!
//! Le texte rassemble des faits : versions, système, ce que la configuration
//! contient en nombre, l'état du trousseau et de l'agent, les dernières lignes
//! du processus de bureau distant de chaque session ouverte. Jamais un
//! secret : aucun mot de passe n'est lu, aucun nom d'hôte de `~/.ssh/config`
//! n'est copié. Les journaux du processus RDP peuvent citer l'adresse d'un
//! serveur : l'en-tête le dit, pour qu'on relise avant de partager.

use std::fmt::Write as _;

/// Les faits rassemblés, sans mise en forme : `collecter` les lit, `composer`
/// les écrit. Les deux sont séparés pour que le texte se teste sans machine.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Faits {
    pub version: String,
    pub webview: String,
    pub systeme: String,
    pub session_graphique: String,
    pub emballage: String,
    pub config_ssh: String,
    pub bureaux: String,
    pub tunnels: String,
    pub sidecar: String,
    pub trousseau: String,
    pub agent: String,
    /// Variables d'environnement qui changent le comportement, avec leur
    /// valeur : aucune ne porte de secret (voir `VARIABLES`).
    pub variables: Vec<(String, String)>,
    /// Identifiant de session et dernières lignes du processus de bureau distant.
    pub sessions_rdp: Vec<(u64, String)>,
    /// Le moteur de rendu du terminal que le front a retenu (`webgl` ou
    /// `dom`), contrat K8 : sans lui, un « le terminal rame » ne se tranchait
    /// pas (audit du 12 septembre 2026, C-front-14).
    pub rendu_terminal: String,
    /// Les dernières lignes du journal de l'application (C-SIL-2).
    pub journal: String,
}

/// Lignes du journal jointes au diagnostic : de quoi dater et situer un
/// défaut, sans en faire un fichier à relire pendant une heure.
const JOURNAL_LIGNES: usize = 60;

/// Le moteur de rendu du terminal, tel que le front l'a signalé.
#[derive(Default)]
pub struct RenduTerminal(std::sync::Mutex<Option<String>>);

/// Le front signale le moteur de rendu de son terminal : « webgl » quand
/// l'extension WebGL s'est chargée, « dom » après un repli (contexte perdu,
/// WebGL indisponible). Contrat K8 de l'audit du 12 septembre 2026.
///
/// Seules ces deux valeurs sont retenues : le texte finit dans un fichier que
/// l'utilisateur joint à un ticket, la page n'a pas à y écrire ce qu'elle veut.
/// Synchrone : une écriture en mémoire, sans disque ni trousseau.
#[tauri::command]
pub fn diagnostic_noter_rendu(
    etat: tauri::State<'_, RenduTerminal>,
    rendu: String,
) -> Result<(), String> {
    use avash::Verrou as _;
    match rendu.as_str() {
        "webgl" | "dom" => {
            *etat.0.verrou() = Some(rendu);
            Ok(())
        }
        _ => Err(format!("Moteur de rendu inconnu : {rendu}")),
    }
}

/// Les variables rapportées. `AVASH_*` sont les nôtres (aucune ne porte de
/// mot de passe ni de jeton) ; les autres expliquent les défauts d'affichage
/// les plus fréquents sous Linux.
const VARIABLES: &[&str] = &[
    "AVASH_HOME",
    "AVASH_LANGUE",
    "AVASH_RDP_BIN",
    "AVASH_RDP_TRACE",
    "GDK_BACKEND",
    "WEBKIT_DISABLE_DMABUF_RENDERER",
    "WEBKIT_DISABLE_COMPOSITING_MODE",
    "XDG_SESSION_TYPE",
    "XDG_CURRENT_DESKTOP",
];

/// Rassemble les faits. `version` et `webview` viennent de Tauri (l'appelant
/// les a) ; `sessions_rdp` du magasin RDP. Le reste se lit ici, sans jamais
/// échouer : un fait illisible devient une phrase qui le dit.
#[must_use]
pub fn collecter(
    version: &str,
    webview: Option<String>,
    sessions_rdp: Vec<(u64, String)>,
    rendu_terminal: Option<String>,
) -> Faits {
    Faits {
        version: version.to_owned(),
        webview: webview.unwrap_or_else(|| "inconnue".to_owned()),
        systeme: systeme(),
        session_graphique: session_graphique(),
        emballage: emballage(),
        config_ssh: config_ssh(),
        bureaux: bureaux(),
        tunnels: tunnels(),
        sidecar: sidecar(),
        trousseau: match avash::secrets::sonder() {
            Ok(()) => "répond".to_owned(),
            Err(e) => format!("ne répond pas : {e}"),
        },
        agent: agent(),
        variables: VARIABLES
            .iter()
            .filter_map(|v| std::env::var(v).ok().map(|val| ((*v).to_owned(), val)))
            .collect(),
        sessions_rdp,
        rendu_terminal: rendu_terminal
            .unwrap_or_else(|| "inconnu (aucun terminal ouvert depuis le lancement)".to_owned()),
        journal: crate::journal::repertoire().map_or_else(
            || "répertoire de configuration introuvable".to_owned(),
            |d| crate::journal::dernieres_lignes(&d, JOURNAL_LIGNES),
        ),
    }
}

fn systeme() -> String {
    let base = format!("{} {}", std::env::consts::OS, std::env::consts::ARCH);
    #[cfg(target_os = "linux")]
    {
        if let Ok(f) = std::fs::read_to_string("/etc/os-release") {
            if let Some(nom) = f
                .lines()
                .find_map(|l| l.strip_prefix("PRETTY_NAME="))
                .map(|v| v.trim_matches('"'))
            {
                return format!("{base}, {nom}");
            }
        }
    }
    base
}

fn session_graphique() -> String {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let x11 = std::env::var_os("DISPLAY").is_some();
    match (wayland, x11) {
        (true, true) => "Wayland (X11 disponible)".to_owned(),
        (true, false) => "Wayland".to_owned(),
        (false, true) => "X11".to_owned(),
        (false, false) => "aucune variable d'affichage".to_owned(),
    }
}

fn emballage() -> String {
    if std::env::var_os("FLATPAK_ID").is_some() {
        "Flatpak".to_owned()
    } else if std::env::var_os("APPIMAGE").is_some() {
        "AppImage".to_owned()
    } else {
        "binaire installé ou portable".to_owned()
    }
}

fn config_ssh() -> String {
    let chemin = avash::ssh_config_path();
    if !chemin.exists() {
        return format!("{} absent", chemin.display());
    }
    match avash::parse_ssh_config() {
        Ok(hotes) => {
            let dossiers = hotes
                .iter()
                .filter(|h| !h.folder.is_empty())
                .map(|h| h.folder.as_str())
                .collect::<std::collections::BTreeSet<_>>()
                .len();
            let rebonds = hotes.iter().filter(|h| h.proxy_jump.is_some()).count();
            let cles = hotes.iter().filter(|h| h.identity_file.is_some()).count();
            format!(
                "{} hôte(s), {dossiers} dossier(s), {rebonds} derrière un rebond, {cles} avec une clé déclarée",
                hotes.len()
            )
        }
        Err(e) => format!("illisible : {e}"),
    }
}

fn bureaux() -> String {
    match avash::rdphost::load_hosts() {
        Ok(hs) => {
            let vnc = hs
                .iter()
                .filter(|h| matches!(h.protocole, avash::rdphost::Protocole::Vnc))
                .count();
            let sans_nla = hs.iter().filter(|h| h.sans_nla).count();
            let tls_herite = hs.iter().filter(|h| h.tls_herite).count();
            format!(
                "{} RDP ({sans_nla} sans NLA, {tls_herite} en TLS hérité), {vnc} VNC",
                hs.len() - vnc
            )
        }
        Err(e) => format!("illisibles : {e}"),
    }
}

fn tunnels() -> String {
    match avash::tunnel::load_defs() {
        Ok(t) => format!("{} défini(s)", t.len()),
        Err(e) => format!("illisibles : {e}"),
    }
}

fn sidecar() -> String {
    match crate::rdp::sidecar_path() {
        Some(p) => {
            let taille = std::fs::metadata(&p).map_or(0, |m| m.len());
            format!("{} ({taille} octets)", p.display())
        }
        None => "introuvable".to_owned(),
    }
}

fn agent() -> String {
    #[cfg(unix)]
    {
        match std::env::var("SSH_AUTH_SOCK") {
            Ok(s) if std::path::Path::new(&s).exists() => "SSH_AUTH_SOCK présent".to_owned(),
            Ok(_) => "SSH_AUTH_SOCK défini mais le socket manque".to_owned(),
            Err(_) => "aucun agent (SSH_AUTH_SOCK absent)".to_owned(),
        }
    }
    #[cfg(windows)]
    {
        let tube_openssh = std::path::Path::new(r"\\.\pipe\openssh-ssh-agent").exists();
        agent_windows_texte(tube_openssh, pageant_present())
    }
}

/// Rend l'état de l'agent SSH sous Windows selon ce que répond chaque transport.
/// Séparé de la sonde pour que le test couvre les trois états sans poste Windows
/// ni agent vivant.
///
/// Trouvé par l'audit du 7 septembre 2026 : la branche Windows ne sondait que le
/// tube OpenSSH ; sur un poste où seul Pageant (l'agent de `PuTTY`) tourne, le
/// diagnostic annonçait « aucun agent » alors que l'authentification par agent
/// fonctionnait (ssh.rs sonde déjà les deux transports), ce qui envoyait le
/// mainteneur sur une fausse piste. Pageant classique n'expose pas le tube
/// OpenSSH, donc le faux négatif était réel.
#[cfg(any(windows, test))]
fn agent_windows_texte(tube_openssh: bool, pageant: bool) -> String {
    match (tube_openssh, pageant) {
        (true, _) => "agent OpenSSH de Windows présent".to_owned(),
        (false, true) => "Pageant présent".to_owned(),
        (false, false) => "aucun agent (ni tube openssh-ssh-agent ni Pageant)".to_owned(),
    }
}

/// Pageant répond-il ? Pageant classique ne passe pas par le tube OpenSSH mais
/// par sa fenêtre cachée (WM_COPYDATA) : on le sonde par le même transport que
/// l'auth (`pageant::PageantStream`, tel que `connect_pageant` l'ouvre). Sonde
/// brève sur un fil dédié avec son propre runtime, pour ne dépendre d'aucun
/// runtime tokio déjà actif dans le contexte de la commande de diagnostic.
#[cfg(windows)]
fn pageant_present() -> bool {
    std::thread::spawn(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .is_ok_and(|rt| rt.block_on(async { pageant::PageantStream::new().await.is_ok() }))
    })
    .join()
    .unwrap_or(false)
}

/// Le texte du diagnostic, prêt à coller dans un ticket.
#[must_use]
pub fn composer(f: &Faits) -> String {
    let mut t = String::new();
    let _ = writeln!(t, "# Diagnostic Avash {}", f.version);
    let _ = writeln!(
        t,
        "Généré par « Exporter un diagnostic ». Aucun mot de passe ni nom d'hôte de\n\
         ~/.ssh/config n'y figure ; les journaux du processus de bureau distant\n\
         peuvent citer l'adresse d'un serveur : relire avant de partager.\n"
    );
    let _ = writeln!(t, "## Application");
    let _ = writeln!(t, "- version : {}", f.version);
    let _ = writeln!(t, "- webview : {}", f.webview);
    let _ = writeln!(t, "- emballage : {}", f.emballage);
    let _ = writeln!(t, "- processus de bureau distant : {}", f.sidecar);
    let _ = writeln!(t, "- Rendu du terminal : {}", f.rendu_terminal);
    let _ = writeln!(t, "\n## Système");
    let _ = writeln!(t, "- système : {}", f.systeme);
    let _ = writeln!(t, "- session graphique : {}", f.session_graphique);
    let _ = writeln!(t, "- trousseau : {}", f.trousseau);
    let _ = writeln!(t, "- agent SSH : {}", f.agent);
    if f.variables.is_empty() {
        let _ = writeln!(
            t,
            "- variables : aucune des variables suivies n'est définie"
        );
    } else {
        let _ = writeln!(t, "- variables :");
        for (n, v) in &f.variables {
            let _ = writeln!(t, "    {n}={v}");
        }
    }
    let _ = writeln!(t, "\n## Configuration");
    let _ = writeln!(t, "- ~/.ssh/config : {}", f.config_ssh);
    let _ = writeln!(t, "- bureaux distants : {}", f.bureaux);
    let _ = writeln!(t, "- tunnels : {}", f.tunnels);
    let _ = writeln!(t, "\n## Sessions de bureau distant");
    if f.sessions_rdp.is_empty() {
        let _ = writeln!(t, "aucune session ouverte");
    }
    for (id, lignes) in &f.sessions_rdp {
        let _ = writeln!(t, "### session {id}");
        if lignes.is_empty() {
            let _ = writeln!(t, "(rien écrit)");
        } else {
            let _ = writeln!(t, "{lignes}");
        }
    }
    let _ = writeln!(t, "\n## Journal");
    let _ = writeln!(t, "{}", f.journal);
    t
}

/// Ouvre la boîte « Enregistrer sous » native, écrit le diagnostic à l'endroit
/// choisi et rend le chemin écrit ; `None` si l'utilisateur a annulé.
///
/// Contrat K13 (audit de sécurité du 12 septembre 2026, C-ipc-3) : la commande
/// prenait un chemin venu de la page et remplaçait la cible par `rename`. Un
/// script de la webview écrasait ainsi `~/.bashrc` par le texte du
/// diagnostic. Le chemin vient désormais de la boîte native, ouverte ici, où
/// c'est l'utilisateur qui confirme le remplacement d'un fichier existant.
/// Appel du front : `invoke("diagnostic_exporter")`, sans argument.
#[tauri::command]
pub async fn diagnostic_exporter<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt as _;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_file_name("avash-diagnostic.txt")
        .add_filter("Texte", &["txt"])
        .save_file(move |c| {
            let _ = tx.send(c);
        });
    let choisi = rx
        .await
        .map_err(|_| "La boîte d'enregistrement s'est fermée sans répondre.".to_owned())?;
    let Some(chemin) = choisi.and_then(|f| f.into_path().ok()) else {
        return Ok(None);
    };
    // Collecte (fichiers, trousseau, processus) et écriture hors des fils du
    // runtime (C-SIL-7).
    super::bloquant(move || ecrire_diagnostic(&app, chemin))
        .await
        .map(Some)
}

/// Écrit le diagnostic à `chemin`, en 0600 et d'un seul tenant, et rend le
/// chemin écrit. Séparé de la boîte de dialogue pour être testé.
pub(crate) fn ecrire_diagnostic<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    chemin: std::path::PathBuf,
) -> Result<String, String> {
    use avash::Verrou as _;
    use tauri::Manager as _;
    // Un chemin relatif dépendrait du répertoire courant de l'application,
    // qui n'est pas celui que l'utilisateur voit dans la boîte de dialogue.
    if !chemin.is_absolute() {
        return Err("Le chemin du diagnostic doit être absolu.".to_owned());
    }
    let rendu = app
        .try_state::<RenduTerminal>()
        .and_then(|r| r.0.verrou().clone());
    let sessions = app
        .try_state::<crate::rdp::RdpStore>()
        .map(|r| crate::rdp::journaux(&r))
        .unwrap_or_default();
    let faits = collecter(
        &app.package_info().version.to_string(),
        tauri::webview_version().ok(),
        sessions,
        rendu,
    );
    avash::ecrire_atomiquement(&chemin, composer(&faits).as_bytes()).map_err(|e| e.to_string())?;
    Ok(chemin.display().to_string())
}

#[cfg(test)]
mod tests_diagnostic {
    use super::{collecter, composer, diagnostic_noter_rendu, ecrire_diagnostic, Faits};
    use crate::commands::tests::{app_de_test, with_ssh_config};
    use tauri::Manager as _;

    fn faits() -> Faits {
        Faits {
            version: "9.9.9".into(),
            webview: "WebKitGTK 2.50".into(),
            systeme: "linux x86_64".into(),
            session_graphique: "Wayland".into(),
            emballage: "AppImage".into(),
            config_ssh: "2 hôte(s)".into(),
            bureaux: "1 RDP (0 sans NLA), 0 VNC".into(),
            tunnels: "0 défini(s)".into(),
            sidecar: "/opt/avash-rdp (12 octets)".into(),
            trousseau: "répond".into(),
            agent: "SSH_AUTH_SOCK présent".into(),
            variables: vec![("AVASH_LANGUE".into(), "fr".into())],
            sessions_rdp: vec![(3, "connecté\nfermé par le serveur".into())],
            rendu_terminal: "webgl".into(),
            journal: "WARN trousseau indisponible".into(),
        }
    }

    /// Le texte porte chaque fait, une fois, sous son titre, et l'avertissement
    /// sur ce qu'il peut contenir.
    #[test]
    fn le_texte_reprend_chaque_fait_et_previent() {
        let t = composer(&faits());
        assert!(t.starts_with("# Diagnostic Avash 9.9.9\n"), "{t}");
        assert!(t.contains("relire avant de partager"), "{t}");
        for attendu in [
            "- webview : WebKitGTK 2.50",
            "- système : linux x86_64",
            "- emballage : AppImage",
            "- trousseau : répond",
            "    AVASH_LANGUE=fr",
            "- ~/.ssh/config : 2 hôte(s)",
            "### session 3\nconnecté\nfermé par le serveur",
            "- Rendu du terminal : webgl",
            "## Journal\nWARN trousseau indisponible",
        ] {
            assert!(t.contains(attendu), "manque « {attendu} » dans :\n{t}");
        }
    }

    /// Sans session ouverte, le diagnostic le dit au lieu de laisser un titre vide.
    #[test]
    fn sans_session_le_texte_le_dit() {
        let mut f = faits();
        f.sessions_rdp.clear();
        f.variables.clear();
        let t = composer(&f);
        assert!(t.contains("aucune session ouverte"), "{t}");
        assert!(t.contains("aucune des variables suivies"), "{t}");
    }

    /// La collecte compte la configuration sans en recopier les noms d'hôte :
    /// l'alias et l'adresse d'un hôte ne doivent apparaître nulle part.
    #[test]
    fn la_collecte_compte_sans_recopier_les_hotes() {
        let _g = with_ssh_config(
            "Host secret-prod\n  HostName 203.0.113.9\n  IdentityFile ~/.ssh/k\n  #Folder: prod\n\nHost bastion\n  HostName 203.0.113.1\n\nHost cache\n  HostName 10.0.0.9\n  ProxyJump bastion\n",
        );
        let f = collecter("1.2.3", None, Vec::new(), None);
        assert_eq!(
            f.config_ssh,
            "3 hôte(s), 1 dossier(s), 1 derrière un rebond, 1 avec une clé déclarée"
        );
        assert_eq!(f.webview, "inconnue");
        let t = composer(&f);
        assert!(!t.contains("secret-prod"), "{t}");
        assert!(!t.contains("203.0.113"), "{t}");
    }

    /// Sous Windows, le diagnostic doit reconnaître Pageant même quand le tube
    /// OpenSSH est absent. Trouvé par l'audit du 7 septembre 2026 : sur un poste
    /// `PuTTY` où seul Pageant tourne, l'authentification par agent réussissait
    /// mais le diagnostic exporté disait « aucun agent », fausse piste pour le
    /// mainteneur. On éprouve la seule logique de décision (les trois états),
    /// la sonde des transports demandant un poste Windows et un agent vivant.
    #[test]
    fn le_diagnostic_windows_reconnait_pageant_seul() {
        use super::agent_windows_texte;
        assert_eq!(
            agent_windows_texte(true, false),
            "agent OpenSSH de Windows présent"
        );
        assert_eq!(
            agent_windows_texte(true, true),
            "agent OpenSSH de Windows présent"
        );
        assert_eq!(agent_windows_texte(false, true), "Pageant présent");
        assert_eq!(
            agent_windows_texte(false, false),
            "aucun agent (ni tube openssh-ssh-agent ni Pageant)"
        );
    }

    /// L'écriture se fait d'un seul tenant, en 0600, et refuse un chemin
    /// relatif ; le moteur de rendu signalé par le front y figure.
    #[test]
    fn la_commande_ecrit_le_fichier_en_0600_et_refuse_un_chemin_relatif() {
        let _g = with_ssh_config("Host a\n  HostName 10.0.0.1\n");
        let app = app_de_test();
        diagnostic_noter_rendu(app.state::<super::RenduTerminal>(), "dom".into()).unwrap();
        let dir = avash::repertoire_personnel().unwrap().join("export");
        std::fs::create_dir_all(&dir).unwrap();
        let chemin = dir.join("diagnostic.txt");
        let rendu = ecrire_diagnostic(app.handle(), chemin.clone()).unwrap();
        assert_eq!(rendu, chemin.display().to_string());
        let texte = std::fs::read_to_string(&chemin).unwrap();
        assert!(texte.starts_with("# Diagnostic Avash "), "{texte}");
        assert!(texte.contains("1 hôte(s)"), "{texte}");
        assert!(texte.contains("- Rendu du terminal : dom"), "{texte}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&chemin).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "mode {mode:o}");
        }
        assert!(ecrire_diagnostic(app.handle(), "relatif/diag.txt".into()).is_err());
    }

    /// Contrat K8 : seules « webgl » et « dom » sont retenues ; la page n'écrit
    /// pas un texte libre dans un fichier joint à un ticket.
    #[test]
    fn le_rendu_du_terminal_n_accepte_que_webgl_ou_dom() {
        let app = app_de_test();
        let etat = || app.state::<super::RenduTerminal>();
        assert!(diagnostic_noter_rendu(etat(), "webgl".into()).is_ok());
        assert!(diagnostic_noter_rendu(etat(), "<script>".into()).is_err());
        let f = collecter("1", None, Vec::new(), None);
        assert!(
            f.rendu_terminal.starts_with("inconnu"),
            "{}",
            f.rendu_terminal
        );
    }

    /// Audit du 12 septembre 2026 (C-SIL-2) : après un avertissement, le
    /// diagnostic exporté porte la ligne du journal.
    #[test]
    fn le_diagnostic_contient_le_journal() {
        let _g = with_ssh_config("");
        let dir = crate::journal::repertoire().unwrap();
        tracing::subscriber::with_default(
            crate::journal::abonne(
                dir,
                crate::journal::TAILLE_MAX,
                crate::journal::niveau(None),
            ),
            || tracing::warn!("témoin du diagnostic"),
        );
        let t = composer(&collecter("1", None, Vec::new(), None));
        let journal = t.split("## Journal").nth(1).unwrap_or_default();
        assert!(journal.contains("témoin du diagnostic"), "{t}");
    }
}
