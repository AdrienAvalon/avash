//! Commandes réseau de l'interface éprouvées contre une vraie session SSH : le
//! serveur SSH+SFTP en mémoire du cœur (`avash::testutil::serveur_ssh`,
//! fonctionnalité `outils-de-test`, en dépendance de développement seulement).
//!
//! Audit du 12 septembre 2026 (C-couv-4 et C-couv-3) : le panneau SFTP, les
//! tunnels et le dépôt de clé n'étaient exercés par aucun test automatisé, faute
//! de session : `SftpHandle` est un type concret sur un canal russh, que le
//! moteur factice de Tauri ne sait pas simuler. Le serveur des tests
//! d'intégration du cœur, désormais partagé, fournit cette session.

use super::tests::{app_de_test, with_ssh_config};
use super::*;
use avash::testutil::serveur_ssh::{
    fs_est_dossier, fs_lire, fs_poser, lancer_serveur, temp_key_path,
};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tauri::Manager as _;

type AppDeTest = tauri::App<tauri::test::MockRuntime>;

/// Un onglet branché sur une vraie session du serveur de test, construit comme
/// `open_on_target` le fait (canal SFTP et exécution sur la session de
/// l'onglet). Rend la session et le PTY : les garder vivants garde l'onglet.
async fn onglet_reel(
    app: &AppDeTest,
    id: u64,
    port: u16,
) -> (SessionPartagee, avash::ssh::PtyChannel) {
    let auth = avash::ssh::ClientAuth {
        user: "testuser".into(),
        key_path: Some(temp_key_path()),
        password: None,
    };
    let mut session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion au serveur de test");
    let pty = session.open_pty(80, 24, "xterm-256color").await.unwrap();
    let partagee: SessionPartagee = std::sync::Arc::new(tokio::sync::Mutex::new(session));
    let handle = SessionHandle {
        epoch: SESSION_EPOCH.fetch_add(1, Ordering::Relaxed),
        input: pty.in_tx.clone(),
        resize: pty.resize_tx.clone(),
        sftp: std::sync::Mutex::new(None),
        ouvrir_sftp: ouvreur_sftp(&partagee),
        executer: executeur(&partagee),
        label: "serveur de test".into(),
        cible: ("127.0.0.1".into(), port, "testuser".into()),
        enregistreur: std::sync::Arc::new(std::sync::Mutex::new(None)),
    };
    enregistrer_session(&app.state::<SessionStore>(), id, handle).unwrap();
    (partagee, pty)
}

/// Attend une condition sans dormir : on rend la main au runtime (un seul fil)
/// à chaque tour, ce qui laisse avancer l'autre moitié d'un `join!` dès qu'elle
/// le peut. Bornée dans le temps, avec un message qui dit ce qu'on attendait.
async fn attendre(quoi: &str, mut condition: impl FnMut() -> bool) {
    let echeance = tokio::time::Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(
            tokio::time::Instant::now() < echeance,
            "jamais arrivé : {quoi}"
        );
        tokio::task::yield_now().await;
    }
}

/// Un port local libre au moment de l'appel.
fn port_libre() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// `~/.ssh/config` qui déclare `srv`, le serveur de test, par clé.
fn config_du_serveur(port: u16) -> String {
    format!(
        "Host srv\n  HostName 127.0.0.1\n  Port {port}\n  User testuser\n  IdentityFile {}\n",
        temp_key_path().display()
    )
}

/// C-couv-4.1 : le panneau canonise et liste sur la session de l'onglet, sans
/// jamais ouvrir de seconde connexion (la promesse de `sftp_of`), et un seul
/// canal SFTP sert les deux commandes.
#[tokio::test]
async fn le_panneau_liste_et_canonise_sur_la_session_de_l_onglet() {
    let serveur = lancer_serveur().await;
    let _g = with_ssh_config("");
    let app = app_de_test();
    let _onglet = onglet_reel(&app, 1, serveur.port).await;
    let racine = sftp_realpath(app.state::<SessionStore>(), 1, ".".into())
        .await
        .unwrap();
    assert_eq!(
        racine, "/home/testuser",
        "le « . » devient un chemin absolu"
    );
    let noms: Vec<String> = sftp_list(app.state::<SessionStore>(), 1, racine)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert!(noms.iter().any(|n| n == "rapport.md"), "{noms:?}");
    assert_eq!(
        serveur.connexions.load(Ordering::SeqCst),
        1,
        "jamais une seconde connexion"
    );
    assert_eq!(
        serveur.sous_systemes_sftp.load(Ordering::SeqCst),
        1,
        "un seul canal SFTP, réutilisé"
    );
}

/// C-couv-4.4 : créer, renommer et supprimer passent par le canal de l'onglet
/// et atteignent le serveur ; un refus du serveur remonte.
#[tokio::test]
async fn mkdir_remove_rename_passent_par_le_canal_de_l_onglet() {
    let serveur = lancer_serveur().await;
    let _g = with_ssh_config("");
    let app = app_de_test();
    let _onglet = onglet_reel(&app, 1, serveur.port).await;
    let st = || app.state::<SessionStore>();
    sftp_mkdir(st(), 1, "/fs/ui-panneau".into()).await.unwrap();
    assert!(fs_est_dossier("/fs/ui-panneau"));
    fs_poser("/fs/ui-panneau/a.txt", b"alpha");
    sftp_rename(
        st(),
        1,
        "/fs/ui-panneau/a.txt".into(),
        "/fs/ui-panneau/b.txt".into(),
    )
    .await
    .unwrap();
    assert!(fs_lire("/fs/ui-panneau/a.txt").is_none());
    assert_eq!(
        fs_lire("/fs/ui-panneau/b.txt").as_deref(),
        Some(&b"alpha"[..])
    );
    sftp_remove(st(), 1, "/fs/ui-panneau/b.txt".into(), false)
        .await
        .unwrap();
    assert!(fs_lire("/fs/ui-panneau/b.txt").is_none());
    sftp_remove(st(), 1, "/fs/ui-panneau".into(), true)
        .await
        .unwrap();
    assert!(!fs_est_dossier("/fs/ui-panneau"));
    assert!(
        sftp_mkdir(st(), 1, "/fs".into()).await.is_err(),
        "un dossier déjà là : le refus du serveur remonte"
    );
}

/// C-couv-4.5 : deux commandes SFTP concurrentes sur un onglet sans canal
/// ouvert n'en gardent qu'un ; le canal perdu de la course est refermé chez le
/// serveur, pas laissé ouvert.
#[tokio::test]
async fn deux_ouvertures_concurrentes_du_canal_n_en_gardent_qu_une() {
    let serveur = lancer_serveur().await;
    let _g = with_ssh_config("");
    let app = app_de_test();
    let _onglet = onglet_reel(&app, 1, serveur.port).await;
    let state = app.state::<SessionStore>();
    let (a, b) = tokio::join!(sftp_of(&state, 1), sftp_of(&state, 1));
    let (a, b) = (a.unwrap(), b.unwrap());
    assert!(
        std::sync::Arc::ptr_eq(&a, &b),
        "un seul canal pour l'onglet"
    );
    let ouverts = serveur.sous_systemes_sftp.load(Ordering::SeqCst);
    assert!((1..=2).contains(&ouverts), "{ouverts} canaux ouverts");
    if ouverts == 2 {
        attendre("la fermeture du canal perdant", || {
            serveur.fermetures.load(Ordering::SeqCst) >= 1
        })
        .await;
    }
    assert_eq!(
        a.realpath(".").await,
        "/home/testuser",
        "le canal gardé sert"
    );
}

/// C-couv-4.6 : fermer l'onglet ferme aussi son canal SFTP chez le serveur
/// (le terminal, lui, reste ouvert ici : le test garde son PTY).
#[tokio::test]
async fn fermer_l_onglet_ferme_le_canal_sftp() {
    let serveur = lancer_serveur().await;
    let _g = with_ssh_config("");
    let app = app_de_test();
    let _onglet = onglet_reel(&app, 1, serveur.port).await;
    // Le magasin garde la seule référence au canal : c'est elle que la
    // fermeture de l'onglet referme.
    drop(sftp_of(&app.state::<SessionStore>(), 1).await.unwrap());
    let avant = serveur.fermetures.load(Ordering::SeqCst);
    pty_close(app.handle().clone(), app.state::<SessionStore>(), 1)
        .await
        .unwrap();
    attendre("le serveur voit le canal SFTP se fermer", || {
        serveur.fermetures.load(Ordering::SeqCst) > avant
    })
    .await;
    assert!(app.state::<SessionStore>().inner.lock().unwrap().is_empty());
}

/// C-couv-4.7 : un tunnel arrêté pendant qu'il s'ouvre ne s'installe pas, et
/// sa socket d'écoute est refermée.
#[tokio::test]
async fn un_tunnel_arrete_pendant_son_ouverture_n_est_pas_installe() {
    let serveur = lancer_serveur().await;
    let _g = with_ssh_config(&config_du_serveur(serveur.port));
    let app = app_de_test();
    let port_local = port_libre();
    let def = tunnel_def_save(
        None,
        "srv".into(),
        "local".into(),
        port_local,
        Some("cible".into()),
        Some(80),
        None,
    )
    .unwrap();
    let tunnels = || app.state::<TunnelStore>();
    let (issue, ()) = tokio::join!(
        tunnel_start(app.handle().clone(), tunnels(), def.id.clone(), None),
        async {
            attendre("l'ouverture en cours", || {
                tunnels().en_cours.lock().unwrap().contains(&def.id)
            })
            .await;
            tunnel_stop(tunnels(), def.id.clone()).await.unwrap();
        }
    );
    assert_eq!(
        issue.err().as_deref(),
        Some("Tunnel arrêté pendant l'ouverture.")
    );
    assert!(tunnel_status(tunnels()).is_empty(), "rien d'installé");
    std::net::TcpListener::bind(("127.0.0.1", port_local))
        .expect("la socket d'écoute du tunnel arrêté est refermée");
}

/// C-couv-4.8 : relancer un tunnel ferme l'ancien avant d'ouvrir le nouveau
/// (sans quoi la seconde écoute sur le même port échouerait), et le magasin
/// n'en garde qu'un. Le relancer deux fois à la fois n'est pas jouable ici :
/// les deux ouvertures se disputeraient le port local fixe (un port à 0 est
/// refusé par `TunnelDef::validate`).
#[tokio::test]
async fn relancer_un_tunnel_ferme_l_ancien_et_n_en_garde_qu_un() {
    let serveur = lancer_serveur().await;
    let _g = with_ssh_config(&config_du_serveur(serveur.port));
    let app = app_de_test();
    let port_local = port_libre();
    let def = tunnel_def_save(
        None,
        "srv".into(),
        "local".into(),
        port_local,
        Some("cible".into()),
        Some(80),
        None,
    )
    .unwrap();
    let tunnels = || app.state::<TunnelStore>();
    tunnel_start(app.handle().clone(), tunnels(), def.id.clone(), None)
        .await
        .unwrap();
    tunnel_start(app.handle().clone(), tunnels(), def.id.clone(), None)
        .await
        .expect("la relance reprend le port de l'ancien, fermé d'abord");
    assert_eq!(tunnel_status(tunnels()).len(), 1);
    tunnel_stop(tunnels(), def.id.clone()).await.unwrap();
    assert!(tunnel_status(tunnels()).is_empty());
    std::net::TcpListener::bind(("127.0.0.1", port_local))
        .expect("l'arrêt referme la socket d'écoute");
}

/// Une ligne publique OpenSSH quelconque : le serveur de test ne l'analyse pas.
const CLE_PUBLIQUE: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHRlc3Q avash@test";

/// C-couv-3.4 : le dépôt de clé installe, puis dit « déjà présente » au second
/// passage. Le serveur de test répond le seul marqueur, SANS écho de la
/// commande : l'écho contiendrait les deux marqueurs, et `interpret_deploy`
/// conclurait « installée » pour la mauvaise raison.
#[tokio::test]
async fn key_deploy_installe_puis_dit_deja_presente() {
    let serveur = lancer_serveur().await;
    let _g = with_ssh_config("");
    let deposer = || {
        key_deploy(
            "127.0.0.1".into(),
            Some(serveur.port),
            "deploy".into(),
            "mdp".into(),
            CLE_PUBLIQUE.into(),
        )
    };
    assert_eq!(deposer().await.unwrap(), "Clé installée sur le serveur.");
    assert_eq!(
        deposer().await.unwrap(),
        "La clé était déjà autorisée sur ce serveur."
    );
}

/// C-couv-3.3 : un refus du shell distant (code non nul) remonte avec son code
/// et la sortie du serveur, au lieu d'un « installée » de complaisance.
#[tokio::test]
async fn key_deploy_signale_un_code_non_nul_avec_la_sortie_du_serveur() {
    let serveur = lancer_serveur().await;
    let _g = with_ssh_config("");
    let e = key_deploy(
        "127.0.0.1".into(),
        Some(serveur.port),
        "sans-droit".into(),
        "mdp".into(),
        CLE_PUBLIQUE.into(),
    )
    .await
    .unwrap_err();
    assert!(e.contains("code 3"), "{e}");
    assert!(e.contains("Permission denied"), "{e}");
}
