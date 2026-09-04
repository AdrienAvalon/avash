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
            format!("{} RDP ({sans_nla} sans NLA), {vnc} VNC", hs.len() - vnc)
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
        if std::path::Path::new(r"\\.\pipe\openssh-ssh-agent").exists() {
            "agent OpenSSH de Windows présent".to_owned()
        } else {
            "aucun agent (tube openssh-ssh-agent absent)".to_owned()
        }
    }
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
    t
}

/// Écrit le diagnostic à `chemin` (choisi par l'utilisateur dans une boîte
/// d'enregistrement), en 0600 et d'un seul tenant, et rend le chemin écrit.
#[tauri::command]
pub fn diagnostic_exporter<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    rdp: tauri::State<'_, crate::rdp::RdpStore>,
    chemin: String,
) -> Result<String, String> {
    let chemin = std::path::PathBuf::from(chemin);
    // Un chemin relatif dépendrait du répertoire courant de l'application,
    // qui n'est pas celui que l'utilisateur voit dans la boîte de dialogue.
    if !chemin.is_absolute() {
        return Err("Le chemin du diagnostic doit être absolu.".to_owned());
    }
    let faits = collecter(
        &app.package_info().version.to_string(),
        tauri::webview_version().ok(),
        crate::rdp::journaux(&rdp),
    );
    avash::ecrire_atomiquement(&chemin, composer(&faits).as_bytes()).map_err(|e| e.to_string())?;
    Ok(chemin.display().to_string())
}

#[cfg(test)]
mod tests_diagnostic {
    use super::{collecter, composer, diagnostic_exporter, Faits};
    use crate::commands::tests::with_ssh_config;
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
        let f = collecter("1.2.3", None, Vec::new());
        assert_eq!(
            f.config_ssh,
            "3 hôte(s), 1 dossier(s), 1 derrière un rebond, 1 avec une clé déclarée"
        );
        assert_eq!(f.webview, "inconnue");
        let t = composer(&f);
        assert!(!t.contains("secret-prod"), "{t}");
        assert!(!t.contains("203.0.113"), "{t}");
    }

    /// La commande écrit le fichier d'un seul tenant, en 0600, et refuse un
    /// chemin relatif.
    #[test]
    fn la_commande_ecrit_le_fichier_en_0600_et_refuse_un_chemin_relatif() {
        let _g = with_ssh_config("Host a\n  HostName 10.0.0.1\n");
        let app = tauri::test::mock_builder()
            .manage(crate::rdp::RdpStore::default())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("application factice");
        let dir = std::env::temp_dir().join(format!("avash-diag-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let chemin = dir.join("diagnostic.txt");
        let rendu = diagnostic_exporter(
            app.handle().clone(),
            app.state::<crate::rdp::RdpStore>(),
            chemin.display().to_string(),
        )
        .unwrap();
        assert_eq!(rendu, chemin.display().to_string());
        let texte = std::fs::read_to_string(&chemin).unwrap();
        assert!(texte.starts_with("# Diagnostic Avash "), "{texte}");
        assert!(texte.contains("1 hôte(s)"), "{texte}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&chemin).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "mode {mode:o}");
        }
        assert!(diagnostic_exporter(
            app.handle().clone(),
            app.state::<crate::rdp::RdpStore>(),
            "relatif/diag.txt".to_owned()
        )
        .is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
