//! Tests d'intégration avash : serveur SSH+SFTP embarqué (russh server),
//! client avash réel dessus. Valide connect/auth/exec/PTY/SFTP bout-en-bout.
//!
//! Le serveur vit dans `avash::testutil::serveur_ssh` (fonctionnalité
//! `outils-de-test`, posée par l'auto-dépendance de développement du crate),
//! partagé avec les tests d'`avash-ui` depuis l'audit du 12 septembre 2026
//! (C-couv-4).

// Les aides de ce fichier (hors `#[test]`) déroulent leur décor par `unwrap`,
// ce que `allow-unwrap-in-tests` ne couvre pas : un décor qui échoue doit
// faire échouer le test sur place.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use avash::testutil::serveur_ssh::*;
use russh::keys::PrivateKey;
use std::sync::Arc;

/// Huit fils demandent la clé en même temps : un seul chemin, une clé
/// lisible. Avec l'ancien « si le fichier n'existe pas », l'un d'eux lisait
/// un fichier en cours d'écriture.
#[test]
fn la_cle_de_test_est_generee_une_fois_meme_en_parallele() {
    // Lancer les huit fils AVANT d'en joindre aucun : un `.map(spawn).map(join)`
    // paresseux joignait chaque fil avant de lancer le suivant, donc rien de
    // parallèle. Trouvé par l'audit du 8 septembre 2026.
    let fils: Vec<_> = (0..8).map(|_| std::thread::spawn(temp_key_path)).collect();
    let chemins: Vec<_> = fils.into_iter().map(|h| h.join().unwrap()).collect();
    assert!(
        chemins.iter().all(|c| c == &chemins[0]),
        "chemins : {chemins:?}"
    );
    let pem = std::fs::read_to_string(&chemins[0]).unwrap();
    assert!(
        russh::keys::decode_secret_key(&pem, None).is_ok(),
        "la clé de test doit se relire"
    );
}

/// `HOME` n'est posé qu'une fois, et toujours sur la même valeur.
///
/// `set_var` porte sur tout le processus : l'appeler depuis chaque test — ils
/// s'exécutent en parallèle — est une mutation concurrente, même quand la
/// valeur ne change pas. Le crate a un verrou pour cela (`testutil`), mais un
/// garde ne se tient pas à travers un `.await` ; poser la variable une seule
/// fois, avant tout test, règle la question sans verrou.
static HOME_POSE: std::sync::LazyLock<()> = std::sync::LazyLock::new(|| {
    let home = virtual_home();
    // Par l'unique écrivain de l'environnement des tests, sous son verrou
    // (audit du 12 septembre 2026, C-unsafe-6).
    let verrou = avash::testutil::verrou_environnement();
    avash::testutil::poser_variable(&verrou, "HOME", Some(home.as_os_str()));
    avash::testutil::poser_variable(&verrou, "AVASH_HOME", Some(home.as_os_str()));
    // Aucun agent SSH joignable pendant les tests : `ouvrir_agent_local` rend
    // alors None, et la garde d'agent, quand le drapeau est levé, refuse en
    // `ConnectFailed` — verdict déterministe qui ne dépend pas de l'agent réel
    // du poste. Posé ici une seule fois, pour ne pas muter l'environnement
    // depuis un test parallèle (cf. `un_canal_d_agent_hors_commande...`).
    avash::testutil::poser_variable(
        &verrou,
        "SSH_AUTH_SOCK",
        Some(home.join("agent-inexistant.sock").as_os_str()),
    );
});

fn test_auth() -> avash::ssh::ClientAuth {
    std::sync::LazyLock::force(&HOME_POSE);
    avash::ssh::ClientAuth {
        user: "testuser".into(),
        key_path: Some(temp_key_path()),
        password: None,
    }
}

/// Attend que les compteurs d'un tunnel atteignent l'état visé.
///
/// Les tests dormaient 100 ms avant de lire un instantané : sous charge, les
/// compteurs atomiques n'étaient pas encore à jour et la suite rougissait sans
/// la moindre régression. On attend un état, avec une échéance — et le message
/// d'échec dit ce qu'on attendait.
async fn attendre_compteurs(
    tunnel: &avash::tunnel::Tunnel,
    vise: impl Fn(&avash::tunnel::TunnelSnapshot) -> bool,
    quoi: &str,
) -> avash::tunnel::TunnelSnapshot {
    let echeance = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let snap = tunnel.snapshot();
        if vise(&snap) {
            return snap;
        }
        assert!(
            tokio::time::Instant::now() < echeance,
            "compteurs jamais arrivés à l'état attendu ({quoi}) : {snap:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

// ---------- Les tests ----------

#[tokio::test]
async fn connect_exec_roundtrip() {
    let port = spawn_test_sshd().await;
    let auth = test_auth();
    let mut session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion échouée");
    let (stdout, code) = session.run("uname -a").await.unwrap();
    assert_eq!(code, 0);
    assert!(
        stdout.contains("CMD:uname -a"),
        "stdout inattendu : {stdout}"
    );
    assert!(stdout.contains("stderr-ok"), "stderr manquant : {stdout}");
    session.disconnect().await.unwrap();
}

#[tokio::test]
async fn exec_rapporte_le_code_de_sortie() {
    // Non-regression : run() cassait sur Eof, or exit-status arrive APRES.
    // Le code etait donc toujours 0. Verifie ici sur un code non nul.
    let port = spawn_test_sshd().await;
    let auth = test_auth();
    let mut session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion echouee");
    let (_out, code) = session.run("sh -c exit 42").await.unwrap();
    assert_eq!(code, 42, "le code de sortie non nul doit remonter");

    let (_out, zero) = session.run("true").await.unwrap();
    assert_eq!(zero, 0);
    session.disconnect().await.unwrap();
}

/// Attend que le serveur signale `CHANNEL_CLOSE`, avec une échéance : sinon le
/// test rougirait par un `assert` clair plutôt que de pendre.
async fn attendre_close(etat: &EtatInondation, quoi: &str) {
    let echeance = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
    while !etat.close_recu.load(std::sync::atomic::Ordering::SeqCst) {
        assert!(
            tokio::time::Instant::now() < echeance,
            "le canal n'a pas été fermé ({quoi}) : CHANNEL_CLOSE jamais reçu par le serveur"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

/// Le flux doit s'être tari : on laisse d'abord la dernière itération en vol se
/// poser, puis on vérifie que le compteur d'octets servis ne bouge plus.
async fn affirmer_flux_tari(etat: &EtatInondation) {
    use std::sync::atomic::Ordering;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let avant = etat.octets.load(Ordering::SeqCst);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let apres = etat.octets.load(Ordering::SeqCst);
    assert_eq!(
        avant, apres,
        "le serveur inonde encore après la fermeture du canal ({avant} -> {apres} octets)"
    );
}

/// Trouvé par l'audit du 7 septembre 2026 : `run` sortait au plafond de 1 Mio
/// sans fermer le canal. `russh::Channel` n'envoie pas de `CHANNEL_CLOSE` à sa
/// chute et le client réalimente la fenêtre : un serveur qui répond
/// `cat /dev/zero` continuait de nous inonder toute la vie de la session. On
/// vérifie que `run` ferme le canal (le serveur reçoit `CHANNEL_CLOSE`), que le
/// flux se tarit, et que la session reste utilisable derrière.
#[tokio::test]
async fn le_plafond_de_sortie_ferme_le_canal() {
    let (port, etat) = spawn_test_sshd_inondation().await;
    let auth = test_auth();
    let mut session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion échouée");

    let (stdout, _code) = session.run("INONDE").await.unwrap();
    assert!(
        stdout.contains("plafond de 1 Mio atteint"),
        "la sortie aurait dû être tronquée au plafond : {}",
        &stdout[..stdout.len().min(120)]
    );

    attendre_close(&etat, "plafond").await;
    affirmer_flux_tari(&etat).await;

    // La même session reste utilisable après la fermeture du canal inondé.
    let (ok, code) = session.run("echo ok").await.unwrap();
    assert_eq!(code, 0);
    assert!(ok.contains("CMD:echo ok"), "session inutilisable : {ok}");
    session.disconnect().await.unwrap();
}

/// Trouvé par l'audit du 7 septembre 2026 : la sonde d'OS bornait l'attente par
/// un `timeout` posé par-dessus `run`, mais lâchait son canal à l'échéance sans
/// le fermer. Un serveur qui débite lentement (jamais le plafond) continuait
/// donc d'inonder la session longue de l'onglet. `run_borne` ferme le canal à
/// l'échéance : on vérifie qu'elle rend une erreur, que le serveur reçoit
/// `CHANNEL_CLOSE`, que le flux se tarit et que la session reste utilisable.
#[tokio::test]
async fn la_sonde_d_os_ferme_son_canal_a_l_echeance() {
    let (port, etat) = spawn_test_sshd_inondation().await;
    let auth = test_auth();
    let mut session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion échouée");

    // Débit lent : le plafond n'est jamais atteint, seule l'échéance tranche.
    let probe = session
        .run_borne("GOUTTE", std::time::Duration::from_millis(300))
        .await;
    assert!(
        probe.is_err(),
        "la sonde bornée aurait dû rendre une erreur d'échéance : {probe:?}"
    );

    attendre_close(&etat, "échéance").await;
    affirmer_flux_tari(&etat).await;

    let (ok, code) = session.run("echo ok").await.unwrap();
    assert_eq!(code, 0);
    assert!(ok.contains("CMD:echo ok"), "session inutilisable : {ok}");
    session.disconnect().await.unwrap();
}

#[tokio::test]
async fn pty_write_and_resize_roundtrip() {
    let port = spawn_test_sshd().await;
    let auth = test_auth();
    let mut session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion échouée");

    let mut pty = session.open_pty(80, 24, "xterm-256color").await.unwrap();

    // Le banner PTY doit arriver
    let first = tokio::time::timeout(std::time::Duration::from_secs(5), pty.out_rx.recv())
        .await
        .expect("timeout banner PTY")
        .expect("canal fermé");
    let banner = String::from_utf8_lossy(&first);
    assert!(
        banner.contains("PTY(xterm-256color 80x24)"),
        "banner : {banner}"
    );

    // Écrire au stdin doit revenir en ECHO:
    pty.in_tx.send(b"bonjour\r".to_vec()).await.unwrap();
    let echoed = tokio::time::timeout(std::time::Duration::from_secs(5), pty.out_rx.recv())
        .await
        .expect("timeout echo")
        .expect("canal fermé");
    let echo = String::from_utf8_lossy(&echoed);
    assert!(echo.contains("ECHO:bonjour"), "echo : {echo}");

    // Resize doit déclencher RESIZED:w×h
    pty.resize_tx.send((120, 40)).await.unwrap();
    let resized = tokio::time::timeout(std::time::Duration::from_secs(5), pty.out_rx.recv())
        .await
        .expect("timeout resize")
        .expect("canal fermé");
    let r = String::from_utf8_lossy(&resized);
    assert!(r.contains("RESIZED:120x40"), "resize : {r}");

    session.disconnect().await.unwrap();
}

#[tokio::test]
async fn sftp_list_download_upload() {
    let port = spawn_test_sshd().await;
    let auth = test_auth();
    let session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion échouée");
    let sftp = avash::sftp::SftpHandle::open(session)
        .await
        .expect("SFTP open");

    // list
    let entries = sftp.list("/").await.unwrap();
    assert!(
        entries.iter().any(|e| e.name == "rapport.md"),
        "entries : {entries:?}"
    );

    // download → fichier local temporaire
    let local = std::env::temp_dir().join(format!("avash-dl-{}.txt", std::process::id()));
    let n = sftp.download("/rapport.md", &local).await.unwrap();
    assert!(n > 0);
    let content = std::fs::read_to_string(&local).unwrap();
    assert_eq!(content, "CONTENU-FICHIER-TEST");

    // upload (le serveur factice accepte tout write)
    let up = std::env::temp_dir().join(format!("avash-up-{}.txt", std::process::id()));
    let payload: &[u8] = b"donnees-locales";
    std::fs::write(&up, payload).unwrap();
    let n = sftp.upload(&up, "/envoye.txt").await.unwrap();
    // Compare a la taille reelle : une constante en dur derive des qu'on
    // touche au contenu (c'etait 14 pour 15 octets).
    assert_eq!(n as usize, payload.len());

    sftp.close().await.unwrap();
}

/// Non-regression : une cle d'hote qui CHANGE doit faire echouer la connexion.
///
/// C'est le scenario d'interception (MITM) : l'hote est deja connu, mais la cle
/// presentee ne correspond plus. OpenSSH refuse et affiche
/// REMOTE HOST IDENTIFICATION HAS CHANGED ; avash doit faire de meme.
///
/// Regression corrigee : le `match` sur `check_known_hosts` confondait
/// "hote inconnu" (Ok(false)) et "cle changee" (Err(KeyChanged)) dans un bras
/// `_` commun, et reapprenait la cle dans les deux cas.
/// Le panneau SFTP d'un onglet ouvre un canal sur la session du terminal, pas
/// une seconde connexion : le serveur ne doit voir qu'une session, et le
/// terminal doit rester utilisable pendant que le panneau travaille.
#[tokio::test]
async fn sftp_sur_la_session_du_terminal_ne_rouvre_pas_de_connexion() {
    use std::sync::atomic::Ordering;
    let (port, connexions) = spawn_test_sshd_compte().await;
    let auth = test_auth();
    let mut session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion échouée");
    let mut pty = session.open_pty(80, 24, "xterm").await.expect("PTY");
    let _ = pty.out_rx.recv().await.expect("bannière du PTY");

    let sftp = avash::sftp::SftpHandle::open_on(&mut session)
        .await
        .expect("SFTP sur la session existante");
    let entries = sftp.list("/").await.unwrap();
    assert!(
        entries.iter().any(|e| e.name == "rapport.md"),
        "{entries:?}"
    );

    // Le terminal vit toujours sur la même session : l'écho du serveur revient.
    pty.in_tx.send(b"ping\n".to_vec()).await.unwrap();
    let echo = tokio::time::timeout(std::time::Duration::from_secs(5), pty.out_rx.recv())
        .await
        .expect("le PTY n'a plus répondu après l'ouverture du SFTP")
        .expect("PTY fermé");
    assert!(!echo.is_empty());

    assert_eq!(
        connexions.load(Ordering::SeqCst),
        1,
        "le serveur a vu plus d'une connexion pour un seul onglet"
    );
    sftp.close().await.unwrap();
    // La session mère n'est pas emportée par la fermeture du canal SFTP.
    let (out, code) = session.run("echo encore").await.unwrap();
    assert_eq!(code, 0, "{out}");
}

#[tokio::test]
async fn changed_host_key_is_refused() {
    let port = spawn_test_sshd().await;
    let auth = test_auth(); // positionne HOME sur le home virtuel

    // On inscrit volontairement une cle qui n'est PAS celle du serveur.
    let decoy = PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
    let decoy_pub = decoy.public_key().clone();
    // On vise EXPLICITEMENT le fichier que le code consultera. La résolution
    // implicite de russh passe par `std::env::home_dir()`, qui sous Windows
    // consulte `USERPROFILE` et ignore notre home de test : le leurre atterrissait
    // dans le vrai profil, la vérification ne le voyait pas, et la connexion
    // était acceptée comme un premier contact.
    let known_hosts = avash::ssh::chemin_known_hosts().expect("chemin known_hosts");
    russh::keys::known_hosts::learn_known_hosts_path("127.0.0.1", port, &decoy_pub, &known_hosts)
        .expect("ecriture known_hosts");

    // Le serveur presente sa vraie cle : elle differe de celle memorisee.
    let res = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth).await;
    let err = res
        .err()
        .expect("une cle d'hote modifiee doit etre refusee");

    // Trouvé par l'audit du 8 septembre 2026 : on nettoyait par
    // `forget_host_key_at`, une réécriture complète du `known_hosts` PARTAGÉ qui
    // court avec les `learn_known_hosts_path` (ajouts) des tests parallèles et
    // pouvait en perdre. On APPREND plutôt la vraie clé du serveur pour ce port
    // (un simple ajout) : `juger_cle_hote` accepte une clé parmi plusieurs, donc
    // un futur serveur de test qui hérite du port est reconnu, et le leurre peut
    // rester sans nuire.
    russh::keys::known_hosts::learn_known_hosts_path(
        "127.0.0.1",
        port,
        CLE_HOTE.public_key(),
        &known_hosts,
    )
    .expect("apprentissage de la vraie clé");

    // Le message doit etre exploitable tel quel dans l'interface : un
    // "Unknown key" opaque ne dit pas a l'utilisateur ce qui se passe ni quoi
    // faire. Il part sinon sur stderr, que personne ne lit dans une GUI.
    let msg = format!("{err:#}");
    assert!(
        msg.contains("CLÉ D'HÔTE A CHANGÉ"),
        "le message doit nommer le probleme : {msg}"
    );
    assert!(
        msg.contains("known_hosts"),
        "le message doit dire comment s'en sortir : {msg}"
    );
}

/// TOFU : un hote inconnu est appris au premier contact, la connexion passe.
#[tokio::test]
async fn unknown_host_is_learned_on_first_contact() {
    let port = spawn_test_sshd().await;
    let auth = test_auth();

    let session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("premier contact : la connexion doit passer (TOFU)");
    drop(session);

    // La clé doit désormais être mémorisée. Le vérifier DANS LE FICHIER : se
    // contenter d'une seconde connexion réussie ne prouve rien, puisqu'un
    // second « premier contact » passerait tout aussi bien.
    let known_hosts = avash::ssh::chemin_known_hosts().expect("chemin known_hosts");
    let apprises =
        russh::keys::known_hosts::known_host_keys_path("127.0.0.1", port, &known_hosts).unwrap();
    assert_eq!(apprises.len(), 1, "la clé d'hôte n'a pas été mémorisée");

    // Et la reconnexion passe, cette fois en « hôte connu ».
    avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("reconnexion sur hote connu : doit passer");
}

/// Non-regression : abandonner le canal de resize ne doit ni tuer la session,
/// ni faire tourner le pump a vide.
///
/// Regression corrigee : le bras `resize_rx.recv()` du select! traitait le
/// None par un no-op. Un canal ferme rendant Ready(None) immediatement et sans
/// fin, la boucle tournait a 100 % de CPU tant que la session vivait.
#[tokio::test]
async fn dropping_resize_channel_keeps_pty_alive_and_idle() {
    let port = spawn_test_sshd().await;
    let auth = test_auth();
    let mut session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion echouee");

    let mut pty = session.open_pty(80, 24, "xterm-256color").await.unwrap();

    // Banner initial
    tokio::time::timeout(std::time::Duration::from_secs(5), pty.out_rx.recv())
        .await
        .expect("timeout banner")
        .expect("canal ferme");

    // On abandonne le resize : le front peut le lacher sans fermer l'onglet.
    let resize = std::mem::replace(&mut pty.resize_tx, {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        tx
    });
    drop(resize);

    // La session doit rester utilisable : le clavier passe toujours.
    pty.in_tx
        .send(b"bonjour".to_vec())
        .await
        .expect("stdin ferme");
    let echoed = tokio::time::timeout(std::time::Duration::from_secs(5), pty.out_rx.recv())
        .await
        .expect("timeout apres abandon du resize")
        .expect("canal ferme apres abandon du resize");
    assert!(
        String::from_utf8_lossy(&echoed).contains("bonjour"),
        "le PTY doit rester vivant : {:?}",
        String::from_utf8_lossy(&echoed)
    );
}

// ---------- Tunnels ----------

use avash::tunnel::{Tunnel, TunnelDef, TunnelKind};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn connect_for_tunnel(port: u16) -> avash::ssh::AvashSession {
    avash::ssh::AvashSession::connect("127.0.0.1", port, &test_auth())
        .await
        .expect("connexion")
}

#[tokio::test]
async fn tunnel_local_relaie_les_octets_dans_les_deux_sens() {
    let port = spawn_test_sshd().await;
    let session = connect_for_tunnel(port).await;
    // Port 0 : on laisse l'OS choisir, puis on lit le port lie.
    let mut def = TunnelDef::new("test", TunnelKind::Local, 1, "db.interne", 5432, "");
    def.bind_port = 0;
    // validate() refuse 0 pour un humain ; ici on contourne pour le test en
    // ouvrant sur un port libre trouve nous-memes.
    let free = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    def.bind_port = free.local_addr().unwrap().port();
    drop(free);
    let tunnel = Tunnel::open(session, def)
        .await
        .expect("ouverture tunnel -L");
    assert_eq!(tunnel.bound_port(), tunnel.def().bind_port);

    let mut client = tokio::net::TcpStream::connect(("127.0.0.1", tunnel.bound_port()))
        .await
        .expect("le port local doit accepter");
    client.write_all(b"ping").await.unwrap();
    let mut buf = vec![0u8; 64];
    let n = tokio::time::timeout(std::time::Duration::from_secs(3), client.read(&mut buf))
        .await
        .expect("reponse dans les temps")
        .unwrap();
    assert_eq!(
        &buf[..n],
        b"ECHO:ping",
        "les octets doivent traverser le tunnel"
    );
    drop(client);

    // Les compteurs refletent la connexion.
    let snap = attendre_compteurs(
        &tunnel,
        |s| s.total == 1 && s.active == 0 && s.bytes_up == 4,
        "une connexion terminée, 4 octets montants",
    )
    .await;
    assert!(snap.alive);
    assert_eq!(snap.total, 1);
    assert_eq!(snap.active, 0, "connexion terminee");
    assert_eq!(snap.bytes_up, 4);
    assert_eq!(snap.bytes_down, "ECHO:ping".len() as u64);

    tunnel.close().await;
}

#[tokio::test]
async fn tunnel_local_signale_une_destination_injoignable() {
    let port = spawn_test_sshd().await;
    let session = connect_for_tunnel(port).await;
    let free = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let bind = free.local_addr().unwrap().port();
    drop(free);
    let def = TunnelDef::new("test", TunnelKind::Local, bind, "injoignable", 1, "");
    let tunnel = Tunnel::open(session, def).await.unwrap();

    let mut client = tokio::net::TcpStream::connect(("127.0.0.1", bind))
        .await
        .unwrap();
    // Le serveur refuse le canal : notre cote ferme la connexion locale.
    let mut buf = [0u8; 8];
    let n = tokio::time::timeout(std::time::Duration::from_secs(3), client.read(&mut buf))
        .await
        .expect("fermeture dans les temps")
        .unwrap();
    assert_eq!(n, 0, "connexion fermee sans donnees");
    let snap = attendre_compteurs(
        &tunnel,
        |s| s.last_error.is_some(),
        "une erreur de destination injoignable",
    )
    .await;
    assert!(snap.alive, "le tunnel lui-meme reste debout");
    assert!(
        snap.last_error
            .as_deref()
            .unwrap_or("")
            .contains("injoignable:1"),
        "l'erreur doit nommer la destination : {:?}",
        snap.last_error
    );
    tunnel.close().await;
}

#[tokio::test]
async fn tunnel_dynamique_negocie_socks5_puis_relaie() {
    let port = spawn_test_sshd().await;
    let session = connect_for_tunnel(port).await;
    let free = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let bind = free.local_addr().unwrap().port();
    drop(free);
    let def = TunnelDef::new("test", TunnelKind::Dynamic, bind, "", 0, "");
    let tunnel = Tunnel::open(session, def).await.unwrap();

    let mut c = tokio::net::TcpStream::connect(("127.0.0.1", bind))
        .await
        .unwrap();
    c.write_all(&[5, 1, 0]).await.unwrap();
    let mut rep = [0u8; 2];
    c.read_exact(&mut rep).await.unwrap();
    assert_eq!(rep, [5, 0]);
    let mut req = vec![5, 1, 0, 3, 9];
    req.extend_from_slice(b"intranet.");
    req.extend_from_slice(&80u16.to_be_bytes());
    c.write_all(&req).await.unwrap();
    let mut ok = [0u8; 10];
    c.read_exact(&mut ok).await.unwrap();
    assert_eq!(ok[1], 0, "CONNECT accepte");

    c.write_all(b"GET /").await.unwrap();
    let mut buf = vec![0u8; 64];
    let n = tokio::time::timeout(std::time::Duration::from_secs(3), c.read(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buf[..n], b"ECHO:GET /");
    tunnel.close().await;
}

#[tokio::test]
async fn tunnel_distant_relaie_vers_un_service_local() {
    // Service local que le serveur doit atteindre a travers nous.
    let local = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let local_port = local.local_addr().unwrap().port();
    // Réponse propre à CE test. Trouvé par l'audit du 7 septembre 2026 : ce
    // test et `une_redirection_distante_en_port_zero_prend_le_port_du_serveur`
    // tournent en parallèle et poussent tous deux « HELLO » dans le statique
    // partagé REMOTE_REPLY ; avec une charge utile identique, la boucle de l'un
    // pouvait être satisfaite par la réponse de l'autre. On marque la réponse
    // par le port local (unique au test) et on filtre dessus.
    let attendu = format!("HELLO-{local_port}");
    let reponse = attendu.clone();
    tokio::spawn(async move {
        let (mut s, _) = local.accept().await.unwrap();
        let mut buf = [0u8; 16];
        let _ = s.read(&mut buf).await.unwrap();
        s.write_all(reponse.as_bytes()).await.unwrap();
    });

    let port = spawn_test_sshd().await;
    let session = connect_for_tunnel(port).await;
    let def = TunnelDef::new(
        "test",
        TunnelKind::Remote,
        40_000,
        "127.0.0.1",
        local_port,
        "",
    );
    let tunnel = Tunnel::open(session, def).await.expect("ouverture -R");
    assert_eq!(tunnel.bound_port(), 40_000);

    // Le serveur de test ouvre le canal de lui-meme et attend la reponse.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if REMOTE_REPLY
            .lock()
            .await
            .iter()
            .any(|r| r == attendu.as_bytes())
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "aucune reponse marquée recue via -R"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    // `bytes_up` compte le sens serveur -> service local (le « hello » de 5
    // octets), `bytes_down` le retour service local -> serveur (notre réponse
    // marquée) : cf. ForwardCounters::relay(a = canal serveur, b = local).
    let attendu_len = attendu.len() as u64;
    let snap = attendre_compteurs(
        &tunnel,
        |s| s.total == 1 && s.bytes_up == 5 && s.bytes_down == attendu_len,
        "un aller de 5 octets et le retour marqué de ce test",
    )
    .await;
    assert_eq!(snap.total, 1);
    assert_eq!(snap.bytes_up, 5, "« hello » vers le service local");
    assert_eq!(
        snap.bytes_down, attendu_len,
        "la réponse marquée vers le serveur"
    );
    tunnel.close().await;
}

// ---------- SFTP : dossiers, renommage, suppression, progression ----------

#[tokio::test]
async fn sftp_mkdir_rename_remove_atteignent_le_serveur() {
    let port = spawn_test_sshd().await;
    let session = connect_for_tunnel(port).await;
    let sftp = avash::sftp::SftpHandle::open(session).await.unwrap();
    sftp.mkdir("/srv/nouveau").await.unwrap();
    sftp.rename("/srv/a.txt", "/srv/b.txt").await.unwrap();
    sftp.remove("/srv/b.txt", false).await.unwrap();
    sftp.remove("/srv/vide", true).await.unwrap();
    let err = sftp.remove("/srv/plein", true).await.unwrap_err();
    assert!(
        err.to_string().contains("doit être vide"),
        "le message doit expliquer pourquoi : {err:#}"
    );
    let ops = SFTP_OPS.lock().await.clone();
    for expected in [
        "mkdir /srv/nouveau",
        "rename /srv/a.txt /srv/b.txt",
        "remove /srv/b.txt",
        "rmdir /srv/vide",
    ] {
        assert!(
            ops.iter().any(|o| o == expected),
            "{expected} absent de {ops:?}"
        );
    }
    sftp.close().await.unwrap();
}

#[tokio::test]
async fn sftp_realpath_resout_le_point_en_chemin_absolu() {
    let port = spawn_test_sshd().await;
    let session = connect_for_tunnel(port).await;
    let sftp = avash::sftp::SftpHandle::open(session).await.unwrap();
    assert_eq!(sftp.realpath(".").await, "/home/testuser");
    // Un chemin deja absolu revient tel quel.
    assert_eq!(sftp.realpath("/srv").await, "/srv");
    sftp.close().await.unwrap();
}

#[tokio::test]
async fn sftp_download_rapporte_sa_progression() {
    let port = spawn_test_sshd().await;
    let session = connect_for_tunnel(port).await;
    let sftp = avash::sftp::SftpHandle::open(session).await.unwrap();
    let local = std::env::temp_dir().join(format!("avash-progress-{}.bin", std::process::id()));
    let mut seen = Vec::new();
    let n = sftp
        .download_with("/srv/fichier.txt", &local, |done, _total| seen.push(done))
        .await
        .unwrap();
    assert_eq!(n, "CONTENU-FICHIER-TEST".len() as u64);
    assert_eq!(
        seen.last().copied(),
        Some(n),
        "la derniere progression = total transfere"
    );
    assert_eq!(std::fs::read(&local).unwrap(), b"CONTENU-FICHIER-TEST");
    let _ = std::fs::remove_file(&local);
    sftp.close().await.unwrap();
}

/// La cible ne doit pas être touchée tant que le transfert n'a pas abouti.
///
/// `File::create` tronquait d'emblée : un double-clic sur un fichier déjà
/// présent dans ~/Téléchargements l'écrasait, et une coupure laissait à sa
/// place un fichier tronqué portant le bon nom. On vérifie ici qu'un
/// téléchargement voué à l'échec — chemin distant inexistant — laisse le
/// fichier local intact et ne sème pas de `.part`.
#[tokio::test]
async fn un_telechargement_qui_echoue_ne_touche_pas_le_fichier_local() {
    let port = spawn_test_sshd().await;
    let session = connect_for_tunnel(port).await;
    let sftp = avash::sftp::SftpHandle::open(session).await.unwrap();
    let local = std::env::temp_dir().join(format!("avash-intact-{}.bin", std::process::id()));
    std::fs::write(&local, b"PRECIEUX").unwrap();

    // Coupure APRÈS le premier bloc : c'est ce cas-là qui laissait un fichier
    // tronqué portant le bon nom. Un échec à l'ouverture, lui, n'a jamais rien
    // écrit — le tester ne prouverait rien.
    let echec = sftp
        .download_with("/srv/coupure.bin", &local, |_, _| {})
        .await;

    assert!(echec.is_err(), "le téléchargement aurait dû échouer");
    assert_eq!(
        std::fs::read(&local).unwrap(),
        b"PRECIEUX",
        "le fichier local a été touché alors que le transfert a échoué"
    );
    let partiel = local.with_extension("bin.part");
    assert!(
        !partiel.exists(),
        "un .part orphelin est resté : {}",
        partiel.display()
    );

    let _ = std::fs::remove_file(&local);
    sftp.close().await.unwrap();
}

/// Quand le renommage final échoue, l'erreur dit où est le fichier complet et
/// le `.part` reste.
///
/// Trouvé par l'audit du 7 septembre 2026 : le transfert termine par
/// `rename(.part, cible)`. Sous Windows, ce remplacement échoue par violation de
/// partage si la cible est ouverte ailleurs sans `FILE_SHARE_DELETE` (Acrobat,
/// Excel, un aperçu de l'Explorateur) ou porte l'attribut lecture seule : le
/// transfert est pourtant COMPLET (tout entier dans le `.part`), mais l'échec
/// était annoncé sans un mot sur le `.part` laissé à côté, transfert à 100 %
/// affiché en erreur. On force ici le même échec sous Unix — cible finale qui
/// est un répertoire, `rename` d'un fichier dessus rend EISDIR — et l'on exige
/// que l'erreur pointe le `.part` et que ce `.part` complet survive.
#[tokio::test]
async fn un_renommage_final_impossible_pointe_le_part_complet() {
    let port = spawn_test_sshd().await;
    let session = connect_for_tunnel(port).await;
    let sftp = avash::sftp::SftpHandle::open(session).await.unwrap();
    // `/srv/fichier.txt` est petit : chemin séquentiel de `download_with`.
    let local = std::env::temp_dir().join(format!("avash-verrou-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&local);
    let _ = std::fs::remove_file(&local);
    std::fs::create_dir(&local).unwrap();

    let issue = sftp
        .download_with("/srv/fichier.txt", &local, |_, _| {})
        .await;

    let e = issue.expect_err("le renommage vers un répertoire aurait dû échouer");
    let msg = format!("{e:#}");
    assert!(
        msg.contains("le fichier complet est dans"),
        "l'erreur ne dit pas où est le fichier complet : {msg}"
    );
    // Le `.part` porte le transfert complet et n'a pas été supprimé.
    let partiel = std::path::PathBuf::from(format!("{}.part", local.display()));
    assert_eq!(
        std::fs::read(&partiel).unwrap(),
        b"CONTENU-FICHIER-TEST",
        "le .part complet doit rester à côté de la cible"
    );

    let _ = std::fs::remove_file(&partiel);
    let _ = std::fs::remove_dir_all(&local);
    sftp.close().await.unwrap();
}

/// Le téléchargement en bandes parallèles doit rendre EXACTEMENT le fichier.
///
/// `File` de russh-sftp n'émet qu'une requête de lecture à la fois : le débit
/// descendant plafonnait à un bloc par aller-retour, huit fois moins que la
/// montée, déjà pipelinée. On lit désormais par bandes, à décalages distincts —
/// ce qui n'a de valeur que si le réassemblage est juste. Le serveur de test
/// honore décalage et longueur, et chaque octet du fichier dépend de sa
/// position : une bande mal placée se verrait immédiatement.
#[tokio::test]
async fn un_telechargement_en_bandes_rend_le_fichier_a_l_octet_pres() {
    let port = spawn_test_sshd().await;
    let session = connect_for_tunnel(port).await;
    let sftp = avash::sftp::SftpHandle::open(session).await.unwrap();
    let local = std::env::temp_dir().join(format!("avash-bandes-{}.bin", std::process::id()));

    let mut progression = Vec::new();
    let n = sftp
        .download_with("/srv/gros.bin", &local, |fait, total| {
            progression.push((fait, total));
        })
        .await
        .unwrap();

    let attendu: &[u8] = &GROS_FICHIER;
    assert_eq!(n as usize, attendu.len(), "taille annoncée");
    assert_eq!(
        std::fs::read(&local).unwrap(),
        attendu,
        "le réassemblage des bandes ne rend pas le fichier d'origine"
    );
    // La progression reste croissante et finit sur le total, malgré des bandes
    // qui avancent en parallèle.
    assert_eq!(progression.last().map(|(f, _)| *f), Some(n));
    assert!(
        progression.windows(2).all(|p| p[0].0 <= p[1].0),
        "progression non monotone"
    );

    let _ = std::fs::remove_file(&local);
    sftp.close().await.unwrap();
}

/// Sans mot de passe, un refus doit porter le marqueur que l'interface guette.
///
/// C'est lui qui déclenche la demande de saisie puis la nouvelle tentative :
/// sans marqueur, l'utilisateur voit un échec sec et sans recours. Le serveur
/// de test acceptait jusqu'ici n'importe qui, si bien qu'aucun test Rust ne
/// produisait ce marqueur.
#[tokio::test]
async fn un_refus_sans_mot_de_passe_porte_le_marqueur_attendu() {
    let port = spawn_test_sshd().await;
    let _home = virtual_home();
    let mut auth = test_auth();
    auth.user = "refuse".into();
    auth.password = None;

    let issue = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth).await;
    let Err(e) = issue else {
        panic!("un refus doit remonter")
    };
    let e = e.to_string();
    assert!(
        e.contains(avash::ssh::PASSWORD_REQUIRED),
        "l'interface ne saura pas qu'il faut demander un mot de passe : {e}"
    );
}

/// Avec un mauvais mot de passe, le message doit dire l'échec — et surtout PAS
/// porter le marqueur, sans quoi l'interface redemanderait indéfiniment.
#[tokio::test]
async fn un_mauvais_mot_de_passe_ne_porte_pas_le_marqueur() {
    let port = spawn_test_sshd().await;
    let _home = virtual_home();
    let mut auth = test_auth();
    auth.user = "refuse".into();
    auth.password = Some(avash::secrets::Zeroizing::new("mauvais".into()));

    let issue = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth).await;
    let Err(e) = issue else {
        panic!("un mauvais mot de passe doit remonter")
    };
    let e = e.to_string();
    assert!(
        !e.contains(avash::ssh::PASSWORD_REQUIRED),
        "marqueur en trop : {e}"
    );
    assert!(
        e.contains("Authentification échouée"),
        "message inattendu : {e}"
    );
    // Le message doit dire ce que le serveur accepte encore : sans cela,
    // l'utilisateur ne peut pas distinguer « mauvais mot de passe » de
    // « cette méthode n'est pas proposée », qui appellent des gestes opposés.
    assert!(
        e.contains("Le serveur propose encore"),
        "l'échec ne nomme pas les méthodes restantes : {e}"
    );
}

/// Le bon mot de passe passe : sans ce cas, les deux tests ci-dessus
/// pourraient passer sur un serveur qui refuse tout, quoi qu'on lui envoie.
#[tokio::test]
async fn le_bon_mot_de_passe_est_accepte() {
    let port = spawn_test_sshd().await;
    let _home = virtual_home();
    let mut auth = test_auth();
    auth.user = "refuse".into();
    auth.password = Some(avash::secrets::Zeroizing::new("le-bon".into()));

    assert!(avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .is_ok());
}

/// Un serveur qui annonce plus qu'il ne sert ne doit pas produire un « succès ».
///
/// Les bandes écrivent à des décalages disjoints : si les dernières tombent sur
/// une fin de fichier prématurée, le `.part` contient les premières données,
/// **des zéros au milieu**, et se voyait promu sur la cible, transfert annoncé
/// réussi. Le chemin séquentiel, lui, ne pouvait que tronquer — jamais trouer.
#[tokio::test]
async fn un_fichier_plus_court_que_promis_ne_passe_pas_pour_un_succes() {
    let port = spawn_test_sshd().await;
    let session = connect_for_tunnel(port).await;
    let sftp = avash::sftp::SftpHandle::open(session).await.unwrap();
    let local = std::env::temp_dir().join(format!("avash-troue-{}.bin", std::process::id()));
    let _ = std::fs::remove_file(&local);

    let issue = sftp
        .download_with("/srv/gros-tronque.bin", &local, |_, _| {})
        .await;

    let Err(e) = issue else {
        panic!("un fichier incomplet ne doit pas être un succès")
    };
    assert!(
        e.to_string().contains("Transfert incomplet"),
        "message inattendu : {e}"
    );
    assert!(
        !local.exists(),
        "la cible ne doit pas exister : {}",
        local.display()
    );
    let partiel = local.with_extension("bin.part");
    assert!(!partiel.exists(), "un .part orphelin est resté");

    sftp.close().await.unwrap();
}

/// Jumeau séquentiel du test précédent : un PETIT fichier (<= 128 Kio, donc
/// chemin séquentiel de `download_with`) tronqué en cours de lecture ne doit
/// pas non plus passer pour un succès.
///
/// Trouvé par l'audit du 7 septembre 2026 : le chemin séquentiel renommait le
/// `.part` sur la cible sans vérifier `done == total`. Un journal distant
/// rotationné/tronqué pendant la lecture atteint un EOF propre plus tôt
/// (`stat` annonce 80, `read` sert 20 puis Eof) : sans garde, le préfixe de
/// 20 octets était promu sur la cible, transfert annoncé réussi.
#[tokio::test]
async fn un_petit_fichier_tronque_ne_passe_pas_pour_un_succes() {
    let port = spawn_test_sshd().await;
    let session = connect_for_tunnel(port).await;
    let sftp = avash::sftp::SftpHandle::open(session).await.unwrap();
    let local =
        std::env::temp_dir().join(format!("avash-petit-tronque-{}.bin", std::process::id()));
    let _ = std::fs::remove_file(&local);

    let issue = sftp
        .download_with("/srv/petit-tronque.bin", &local, |_, _| {})
        .await;

    let Err(e) = issue else {
        panic!("un petit fichier incomplet ne doit pas être un succès")
    };
    assert!(
        e.to_string().contains("Transfert incomplet"),
        "message inattendu : {e}"
    );
    assert!(
        !local.exists(),
        "la cible ne doit pas exister : {}",
        local.display()
    );
    let partiel = local.with_extension("bin.part");
    assert!(!partiel.exists(), "un .part orphelin est resté");

    sftp.close().await.unwrap();
}

/// Miroir du précédent : un PETIT fichier qui a GROSSI pendant la lecture
/// (`stat` annonce 20, `read` en sert 40 puis Eof) reste un transfert complet
/// et doit RÉUSSIR — c'est ce qui impose une garde `done < total`, et non
/// `done != total` qui rejetterait à tort un journal en cours d'écriture.
#[tokio::test]
async fn un_petit_fichier_qui_a_grossi_reste_un_succes() {
    let port = spawn_test_sshd().await;
    let session = connect_for_tunnel(port).await;
    let sftp = avash::sftp::SftpHandle::open(session).await.unwrap();
    let local =
        std::env::temp_dir().join(format!("avash-petit-agrandi-{}.bin", std::process::id()));
    let _ = std::fs::remove_file(&local);

    let n = sftp
        .download_with("/srv/petit-agrandi.bin", &local, |_, _| {})
        .await
        .expect("un fichier complet, même plus gros qu'annoncé, doit réussir");

    assert_eq!(n, 40, "les 40 octets réellement servis sont écrits");
    assert_eq!(
        std::fs::read(&local).unwrap(),
        (0..40u8).collect::<Vec<u8>>(),
        "la cible porte les octets servis, pas le préfixe annoncé"
    );
    let partiel = local.with_extension("bin.part");
    assert!(!partiel.exists(), "un .part orphelin est resté");

    let _ = std::fs::remove_file(&local);
    sftp.close().await.unwrap();
}

/// Un compte de domaine `DOMAINE\utilisateur` doit arriver INTACT au serveur.
///
/// C'est la forme qu'impose un hôte Linux joint à un annuaire Active Directory.
/// La contre-oblique traverse la saisie, l'IPC de Tauri (donc du JSON, où elle
/// s'échappe) et la requête d'authentification SSH : si l'une de ces étapes la
/// mangeait ou la doublait, le serveur verrait un autre compte et refuserait,
/// sans que rien n'indique pourquoi.
#[tokio::test]
async fn un_compte_de_domaine_arrive_intact_au_serveur() {
    let (port, dernier) = spawn_test_sshd_observe().await;
    let _home = virtual_home();

    let auth = avash::ssh::ClientAuth {
        user: "TEST\\Adrien".into(),
        key_path: None,
        password: Some(avash::secrets::Zeroizing::new("secret".into())),
    };
    let session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion");

    assert_eq!(
        dernier.lock().unwrap().as_deref(),
        Some("TEST\\Adrien"),
        "le nom de domaine n'est pas arrivé intact"
    );
    session.disconnect().await.unwrap();
}

/// Un serveur qui n'accepte QUE `keyboard-interactive` doit être joignable.
///
/// C'est la configuration courante d'un hôte Linux joint à un annuaire :
/// `PasswordAuthentication` désactivé, la conversation confiée à PAM. OpenSSH
/// bascule tout seul ; Avash ne savait pas, et rendait « authentification
/// échouée » avec un mot de passe pourtant juste.
#[tokio::test]
async fn un_serveur_qui_n_accepte_que_pam_est_joignable() {
    let port = spawn_test_sshd().await;
    let _home = virtual_home();
    let auth = avash::ssh::ClientAuth {
        user: "pam-seul".into(),
        key_path: None,
        password: Some(avash::secrets::Zeroizing::new("le-bon".into())),
    };
    let session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("un serveur en keyboard-interactive doit être joignable");
    session.disconnect().await.unwrap();
}

/// Plusieurs invites masquées d'affilée : chacune reçoit le mot de passe.
#[tokio::test]
async fn plusieurs_invites_masquees_sont_toutes_honorees() {
    let port = spawn_test_sshd().await;
    let _home = virtual_home();
    let auth = avash::ssh::ClientAuth {
        user: "pam-double".into(),
        key_path: None,
        password: Some(avash::secrets::Zeroizing::new("le-bon".into())),
    };
    assert!(avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .is_ok());
}

/// Une invite EN CLAIR n'est pas un mot de passe — code à usage unique,
/// question de sécurité. Y envoyer le mot de passe le livrerait à l'écran du
/// serveur sans aboutir. On renonce en nommant ce qui était demandé.
#[tokio::test]
async fn une_invite_en_clair_n_est_pas_remplie_avec_le_mot_de_passe() {
    let port = spawn_test_sshd().await;
    let _home = virtual_home();
    let auth = avash::ssh::ClientAuth {
        user: "pam-otp".into(),
        key_path: None,
        password: Some(avash::secrets::Zeroizing::new("le-bon".into())),
    };
    let issue = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth).await;
    let Err(e) = issue else {
        panic!("une invite en clair ne doit pas être remplie à l'aveugle")
    };
    let msg = e.to_string();
    assert!(
        msg.contains("Code à usage unique"),
        "l'invite doit être citée : {msg}"
    );
    assert!(
        !msg.contains("le-bon"),
        "le mot de passe ne doit pas fuiter dans le message"
    );
}

/// Trouvé par l'audit du 7 septembre 2026 : `authenticate` faisait `?` sur le
/// chargement de la clé. Une clé chiffrée par phrase de passe (chargée dans
/// l'agent, donc `ssh` fonctionne) faisait échouer la connexion AVANT même
/// d'essayer l'agent ou le mot de passe. On utilise l'utilisateur « refuse »,
/// qui rejette la clé publique : un éventuel agent SSH réel de la machine ne
/// peut donc pas aboutir, et l'on éprouve bien le repli par mot de passe.
#[tokio::test]
async fn une_cle_chiffree_ne_bloque_pas_le_repli_mot_de_passe() {
    let port = spawn_test_sshd().await;
    std::sync::LazyLock::force(&HOME_POSE); // pose HOME
    let cle = PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
    let chiffree = cle.encrypt(&mut rand::rng(), "phrase-de-passe").unwrap();
    let pem = chiffree
        .to_openssh(russh::keys::ssh_key::LineEnding::LF)
        .unwrap();
    let path = virtual_home().join(format!("id_chiffree-{}", std::process::id()));
    std::fs::write(&path, pem.as_bytes()).unwrap();
    // Garde-fou : la clé ne doit PAS se charger sans phrase, sinon le test ne
    // prouverait rien.
    assert!(
        russh::keys::load_secret_key(&path, None).is_err(),
        "la clé de test doit être chiffrée"
    );

    // Clé chiffrée + mot de passe accepté : la connexion aboutit par le mot de
    // passe, au lieu d'échouer au chargement de la clé.
    let auth = avash::ssh::ClientAuth {
        user: "refuse".into(),
        key_path: Some(path.clone()),
        password: Some(avash::secrets::Zeroizing::new("le-bon".into())),
    };
    let mut session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("le mot de passe doit prendre le relais d'une clé chiffrée");
    let (out, _) = session.run("echo ok").await.unwrap();
    assert!(out.contains("CMD:echo ok"), "{out:?}");
    session.disconnect().await.unwrap();

    // Sans mot de passe : l'échec porte le marqueur (l'interface demandera un
    // mot de passe) et nomme la clé inutilisable.
    let auth_sans = avash::ssh::ClientAuth {
        user: "refuse".into(),
        key_path: Some(path.clone()),
        password: None,
    };
    let err = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth_sans)
        .await
        .err()
        .expect("sans mot de passe, la connexion doit échouer");
    let msg = format!("{err:#}");
    assert!(
        msg.contains(avash::ssh::PASSWORD_REQUIRED),
        "le marqueur d'invite de mot de passe est attendu : {msg}"
    );
    assert!(
        msg.contains("id_chiffree"),
        "la clé inutilisable doit être nommée : {msg}"
    );
    let _ = std::fs::remove_file(&path);
}

// ---------- ProxyJump ----------

#[tokio::test]
async fn proxy_jump_connecte_la_cible_a_travers_un_rebond() {
    // Deux serveurs : un rebond (jump) et la cible. On se connecte a la cible
    // UNIQUEMENT via le rebond, comme `ssh -J jump cible`.
    let jump_port = spawn_test_sshd().await;
    let target_port = spawn_test_sshd().await;
    let hop = avash::ssh::Hop {
        addr: "127.0.0.1".into(),
        port: jump_port,
        auth: test_auth(),
    };
    let mut session =
        avash::ssh::AvashSession::connect_via(&[hop], "127.0.0.1", target_port, &test_auth())
            .await
            .expect("connexion via rebond");
    // On atteint bien la cible : elle repond a l'exec.
    let (out, code) = session.run("echo via-jump").await.unwrap();
    assert!(
        out.contains("CMD:echo via-jump"),
        "réponse de la cible : {out:?}"
    );
    assert_eq!(code, 0);
    session.disconnect().await.unwrap();
}

#[tokio::test]
async fn proxy_jump_a_deux_rebonds() {
    // Chaine de deux rebonds avant la cible.
    let j1 = spawn_test_sshd().await;
    let j2 = spawn_test_sshd().await;
    let target = spawn_test_sshd().await;
    let hops = vec![
        avash::ssh::Hop {
            addr: "127.0.0.1".into(),
            port: j1,
            auth: test_auth(),
        },
        avash::ssh::Hop {
            addr: "127.0.0.1".into(),
            port: j2,
            auth: test_auth(),
        },
    ];
    let mut session =
        avash::ssh::AvashSession::connect_via(&hops, "127.0.0.1", target, &test_auth())
            .await
            .expect("connexion via 2 rebonds");
    let (out, _) = session.run("echo deux-rebonds").await.unwrap();
    assert!(out.contains("CMD:echo deux-rebonds"), "{out:?}");
    session.disconnect().await.unwrap();
}

#[tokio::test]
async fn un_rebond_a_cle_changee_garde_le_marqueur_sous_le_format_alterne() {
    // Trouvé par l'audit du 7 septembre 2026 : `connect_via` enrobe l'échec d'un
    // rebond d'un « Rebond hôte:port », ce qui enterre le marqueur
    // `[AVASH_HOST_KEY_CHANGED]`. Le `Display` d'anyhow (`to_string()`, ce que
    // faisait `etablir`) n'affiche que ce contexte externe et perd le marqueur ;
    // l'interface, qui le repère par inclusion, ne proposait alors pas d'oublier
    // la clé changée d'un rebond. `{e:#}` déroule toute la chaîne, marqueur
    // compris.
    let jump_port = spawn_test_sshd().await;
    let _auth = test_auth(); // pose HOME
                             // Fausse clé mémorisée pour le REBOND : sa clé d'hôte « a changé ».
    let decoy = PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
    let known_hosts = avash::ssh::chemin_known_hosts().unwrap();
    russh::keys::known_hosts::learn_known_hosts_path(
        "127.0.0.1",
        jump_port,
        decoy.public_key(),
        &known_hosts,
    )
    .unwrap();

    let hop = avash::ssh::Hop {
        addr: "127.0.0.1".into(),
        port: jump_port,
        auth: test_auth(),
    };
    let err = avash::ssh::AvashSession::connect_via(&[hop], "127.0.0.1", jump_port, &test_auth())
        .await
        .err()
        .expect("une clé de rebond changée doit être refusée");

    let _ = avash::ssh::forget_host_key_at("127.0.0.1", jump_port, &known_hosts);

    let alterne = format!("{err:#}");
    assert!(
        alterne.contains("Rebond"),
        "le contexte du rebond est attendu : {alterne}"
    );
    assert!(
        alterne.contains(avash::ssh::HOST_KEY_CHANGED),
        "le marqueur doit survivre au format alterné : {alterne}"
    );
    assert!(
        !err.to_string().contains(avash::ssh::HOST_KEY_CHANGED),
        "to_string() n'affiche que le contexte externe et perd le marqueur : {err}"
    );
}

#[tokio::test]
async fn connect_via_sans_rebond_equivaut_a_connect() {
    let port = spawn_test_sshd().await;
    let mut session = avash::ssh::AvashSession::connect_via(&[], "127.0.0.1", port, &test_auth())
        .await
        .expect("connexion directe via liste vide");
    let (out, _) = session.run("echo direct").await.unwrap();
    assert!(out.contains("CMD:echo direct"), "{out:?}");
    session.disconnect().await.unwrap();
}

// ---------- known_hosts : oubli d'une clé ----------

#[tokio::test]
async fn forget_host_key_retire_la_cle_apprise() {
    // Fichier known_hosts dedie : aucune dependance a HOME, donc aucune course
    // avec les autres tests (HOME est global au processus).
    let path = std::env::temp_dir().join(format!(
        "avash-kh-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_file(&path);
    let key =
        russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
    // Apprend une cle pour un hote fictif dans CE fichier.
    russh::keys::known_hosts::learn_known_hosts_path("10.9.8.7", 2222, key.public_key(), &path)
        .unwrap();

    let before = russh::keys::known_hosts::known_host_keys_path("10.9.8.7", 2222, &path).unwrap();
    assert_eq!(before.len(), 1, "cle apprise");

    let removed = avash::ssh::forget_host_key_at("10.9.8.7", 2222, &path).unwrap();
    assert_eq!(removed, 1);
    let after = russh::keys::known_hosts::known_host_keys_path("10.9.8.7", 2222, &path).unwrap();
    assert!(after.is_empty(), "cle oubliee");

    // Oublier une cle absente ne casse rien.
    assert_eq!(
        avash::ssh::forget_host_key_at("10.9.8.7", 2222, &path).unwrap(),
        0
    );
    let _ = std::fs::remove_file(&path);
}

/// Trouvé par l'audit du 7 septembre 2026 : un commentaire en tête du fichier
/// décalait la numérotation. russh ne compte pas les lignes `# …` (son
/// `continue` saute l'incrément), mais l'oubli les comptait via `enumerate()` :
/// oublier `hôteB` retirait en réalité la clé de `hôteA`, dont le TOFU repartait
/// alors de zéro, et laissait celle de `hôteB` en place.
#[tokio::test]
async fn forget_host_key_ne_touche_pas_a_un_autre_hote_apres_un_commentaire() {
    let path = std::env::temp_dir().join(format!(
        "avash-kh-cmt-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_file(&path);
    let cle_a =
        russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
    let cle_b =
        russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
    // Un commentaire, puis deux hôtes distincts. La forme d'une entrée
    // known_hosts est « hôte type base64 » : c'est ce qu'écrit `to_openssh`.
    let ligne = |hote: &str, k: &russh::keys::PrivateKey| {
        format!("{hote} {}\n", k.public_key().to_openssh().unwrap())
    };
    let contenu = format!(
        "# mes serveurs\n{}{}",
        ligne("hoteA", &cle_a),
        ligne("hoteB", &cle_b)
    );
    std::fs::write(&path, &contenu).unwrap();

    let retirees = avash::ssh::forget_host_key_at("hoteB", 22, &path).unwrap();
    assert_eq!(retirees, 1, "une seule clé retirée");
    let reste = std::fs::read_to_string(&path).unwrap();
    assert!(
        reste.contains("hoteA "),
        "la clé de hoteA doit rester :\n{reste}"
    );
    assert!(
        !reste.contains("hoteB "),
        "la clé de hoteB doit partir :\n{reste}"
    );
    assert!(
        reste.contains("# mes serveurs"),
        "le commentaire doit rester"
    );
    // hoteA reste effectivement reconnu par russh après l'opération.
    let a = russh::keys::known_hosts::known_host_keys_path("hoteA", 22, &path).unwrap();
    assert_eq!(a.len(), 1, "hoteA toujours connu");
    let _ = std::fs::remove_file(&path);
}

// ---------- Transferts : dossiers, reprise, bandes montantes, annulation, relais ----------

/// Un dossier temporaire à ce test, vide.
fn dossier_temp(nom: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("avash-transferts-{}-{nom}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Un contenu où chaque octet dépend de sa position : un réassemblage faux se voit.
fn motif(n: usize, graine: u32) -> Vec<u8> {
    (0..n)
        .map(|i| ((i as u32).wrapping_mul(31).wrapping_add(graine) % 251) as u8)
        .collect()
}

async fn sftp_de_test() -> avash::sftp::SftpHandle {
    let port = spawn_test_sshd().await;
    let session = connect_for_tunnel(port).await;
    avash::sftp::SftpHandle::open(session).await.unwrap()
}

/// Un dossier distant entier arrive avec ses sous-dossiers, ses fichiers
/// (dont un assez gros pour les bandes) et ses dossiers vides.
#[tokio::test]
async fn un_dossier_distant_se_telecharge_entier() {
    let gros = motif(300 * 1024, 1);
    fs_poser("/fs/dl/a.txt", b"alpha");
    fs_poser("/fs/dl/sous/b.bin", &gros);
    fs_dossiers()
        .as_mut()
        .unwrap()
        .insert("/fs/dl/vide".to_owned());
    let sftp = sftp_de_test().await;
    let local = dossier_temp("dl");
    let mut dernier = avash::sftp::Avancement::default();
    let n = sftp
        .download_dir_with("/fs/dl", &local, None, |a| {
            dernier = a;
        })
        .await
        .unwrap();
    assert_eq!(n as usize, 5 + gros.len());
    assert_eq!(std::fs::read(local.join("a.txt")).unwrap(), b"alpha");
    assert_eq!(
        std::fs::read(local.join("sous").join("b.bin")).unwrap(),
        gros
    );
    assert!(local.join("vide").is_dir(), "le dossier vide est recréé");
    assert_eq!((dernier.termines, dernier.nombre, dernier.fait), (2, 2, n));
    let _ = std::fs::remove_dir_all(&local);
    sftp.close().await.unwrap();
}

/// Un dossier local entier part avec son arborescence ; le gros fichier
/// arrive à l'octet près.
#[tokio::test]
async fn un_dossier_local_se_televerse_entier() {
    let gros = motif(300 * 1024 + 11, 2);
    let local = dossier_temp("up");
    std::fs::create_dir_all(local.join("sous").join("creux")).unwrap();
    std::fs::write(local.join("a.txt"), b"alpha").unwrap();
    std::fs::write(local.join("sous").join("b.bin"), &gros).unwrap();
    let sftp = sftp_de_test().await;
    let mut dernier = avash::sftp::Avancement::default();
    let n = sftp
        .upload_dir_with(&local, "/fs/up/arbre", None, |a| {
            dernier = a;
        })
        .await
        .unwrap();
    assert_eq!(n as usize, 5 + gros.len());
    assert_eq!(fs_lire("/fs/up/arbre/a.txt").unwrap(), b"alpha");
    assert_eq!(fs_lire("/fs/up/arbre/sous/b.bin").unwrap(), gros);
    assert!(fs_est_dossier("/fs/up/arbre/sous/creux"));
    assert_eq!((dernier.termines, dernier.nombre), (2, 2));
    let _ = std::fs::remove_dir_all(&local);
    sftp.close().await.unwrap();
}

/// Annulé en route, un téléchargement garde son `.part` et sa carte ; relancé,
/// il rend le fichier entier et ne laisse aucune trace.
///
/// Huit mégaoctets : huit bandes de seize blocs. Avec 400 Kio (deux blocs par
/// bande), les bandes finissaient leurs lectures avant que la boucle de
/// progression, qui pose le drapeau, ne les ait toutes vues : sur l'exécuteur
/// macOS de la chaîne, le transfert rendait Ok(409600) au lieu de
/// « annulé » (régression vue en CI, 04/09/2026). Le drapeau est vérifié par
/// chaque bande avant chaque bloc : avec seize blocs par bande, il est vu.
#[tokio::test]
async fn un_telechargement_annule_reprend_et_rend_le_fichier_entier() {
    let gros = motif(8 * 1024 * 1024, 3);
    fs_poser("/fs/reprise/gros.bin", &gros);
    let sftp = sftp_de_test().await;
    let local = dossier_temp("reprise").join("gros.bin");
    let annulation: avash::sftp::Annulation = std::sync::Arc::default();
    let a = annulation.clone();
    let issue = sftp
        .download_reprise(
            "/fs/reprise/gros.bin",
            &local,
            Some(&annulation),
            move |fait, _| {
                if fait >= 1024 * 1024 {
                    a.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            },
        )
        .await;
    let e = issue.expect_err("l'annulation doit se voir");
    assert!(e.to_string().contains(avash::sftp::ANNULE), "{e:#}");
    let partiel = local.with_file_name("gros.bin.part");
    let carte = local.with_file_name("gros.bin.part.reprise");
    // La carte n'existe que si une bande a fini avant l'annulation, ce que
    // huit bandes menées de front ne garantissent pas ; le .part, lui, reste.
    assert!(partiel.exists(), "le .part reste pour la reprise");
    let n = sftp
        .download_reprise("/fs/reprise/gros.bin", &local, None, |_, _| {})
        .await
        .unwrap();
    assert_eq!(n as usize, gros.len());
    assert_eq!(
        std::fs::read(&local).unwrap(),
        gros,
        "le fichier repris est faux"
    );
    assert!(
        !partiel.exists() && !carte.exists(),
        "plus de trace après la reprise"
    );
    let _ = std::fs::remove_dir_all(local.parent().unwrap());
    sftp.close().await.unwrap();
}

/// La reprise ne redemande pas les bandes que la carte dit complètes : avec
/// une carte posée à la main (première et dernière bandes faites, leurs
/// octets déjà dans le `.part`), aucune lecture ne tombe dans ces bandes, et
/// le fichier rendu est entier. Déterministe, là où l'annulation en cours de
/// route ne garantit pas qu'une bande ait fini.
#[tokio::test]
async fn la_reprise_ne_relit_pas_les_bandes_que_la_carte_dit_faites() {
    let gros = motif(2 * 1024 * 1024, 5);
    fs_poser("/fs/reprise2/gros.bin", &gros);
    let sftp = sftp_de_test().await;
    let local = dossier_temp("reprise2").join("gros.bin");
    let partiel = local.with_file_name("gros.bin.part");
    let carte = local.with_file_name("gros.bin.part.reprise");
    // Huit bandes de 256 Kio : la première et la dernière sont « faites ».
    let bande = 256 * 1024u64;
    let faites = vec![(0, bande), (7 * bande, 8 * bande)];
    let mut part = vec![0u8; gros.len()];
    for (d, f) in &faites {
        let (d, f) = (*d as usize, *f as usize);
        part[d..f].copy_from_slice(&gros[d..f]);
    }
    std::fs::write(&partiel, &part).unwrap();
    // Même taille et même date que le serveur de test : la carte vaut.
    std::fs::write(
        &carte,
        serde_json::json!({ "taille": gros.len(), "mtime": 1_700_000_000u64, "faites": faites })
            .to_string(),
    )
    .unwrap();

    fs_oublier_lectures();
    let n = sftp
        .download_reprise("/fs/reprise2/gros.bin", &local, None, |_, _| {})
        .await
        .unwrap();
    assert_eq!(n as usize, gros.len());
    assert_eq!(
        std::fs::read(&local).unwrap(),
        gros,
        "le fichier repris est faux"
    );
    assert!(
        !partiel.exists() && !carte.exists(),
        "plus de trace après la reprise"
    );
    let lectures = fs_lectures_de("/fs/reprise2/gros.bin");
    assert!(!lectures.is_empty(), "les six bandes manquantes se lisent");
    for o in lectures {
        assert!(
            !faites.iter().any(|(d, f)| *d <= o && o < *f),
            "la reprise a relu le décalage {o}, dans une bande déjà faite {faites:?}"
        );
    }
    let _ = std::fs::remove_dir_all(local.parent().unwrap());
    sftp.close().await.unwrap();
}

/// Une carte disant une bande « faite » alors que le `.part` est plus court
/// que cette bande ne doit pas promouvoir un fichier faux.
///
/// Trouvé par l'audit du 7 septembre 2026 : la carte `.part.reprise` est écrite
/// avec un `fsync` (`ecrire_atomiquement`), mais les octets d'une bande
/// n'avaient qu'un `flush` (pas de `fsync`) au moment de l'annonce « bande
/// faite ». Après une coupure de courant, la carte durable pouvait promettre
/// une bande que le `.part` n'avait pas encore reçue, plus courte sur le disque.
/// À la reprise, la bande dite faite n'était pas relue, `fait == total` comptait
/// `deja_fait` sans regarder le fichier, et le `rename` promouvait un fichier
/// troué (zéros là où l'écriture d'une bande suivante avait étendu le `.part`
/// en sparse). Ici, la carte annonce `(0, bande)` faite mais le `.part` s'arrête
/// à `bande / 2` : la reprise doit refuser cette carte et repartir de zéro, donc
/// relire la bande 0, et rendre le fichier entier.
#[tokio::test]
async fn une_carte_de_reprise_survit_a_un_part_tronque() {
    let gros = motif(2 * 1024 * 1024, 6);
    fs_poser("/fs/reprise3/gros.bin", &gros);
    let sftp = sftp_de_test().await;
    let local = dossier_temp("reprise3").join("gros.bin");
    let partiel = local.with_file_name("gros.bin.part");
    let carte = local.with_file_name("gros.bin.part.reprise");
    // Huit bandes de 256 Kio : la carte prétend la première faite, mais le
    // `.part` s'arrête au milieu de cette bande (crash entre la donnée non
    // durable et la carte durable).
    let bande = 256 * 1024u64;
    let faites = vec![(0u64, bande)];
    let tronque = (bande / 2) as usize;
    std::fs::write(&partiel, &gros[..tronque]).unwrap();
    std::fs::write(
        &carte,
        serde_json::json!({ "taille": gros.len(), "mtime": 1_700_000_000u64, "faites": faites })
            .to_string(),
    )
    .unwrap();

    let n = sftp
        .download_reprise("/fs/reprise3/gros.bin", &local, None, |_, _| {})
        .await
        .unwrap();
    assert_eq!(n as usize, gros.len());
    assert_eq!(
        std::fs::read(&local).unwrap(),
        gros,
        "un .part tronqué sous une carte optimiste ne doit pas être promu tel quel"
    );
    assert!(
        !partiel.exists() && !carte.exists(),
        "plus de trace après la reprise"
    );
    let _ = std::fs::remove_dir_all(local.parent().unwrap());
    sftp.close().await.unwrap();
}

/// Même chose en montée : la carte `.envoi.reprise` note ce qui est
/// sûrement écrit, la reprise repart de là et le fichier distant finit entier.
#[tokio::test]
async fn un_envoi_annule_reprend_sans_renvoyer_les_bandes_faites() {
    // Douze mégaoctets : au moins deux points de contrôle (tous les 4 Mio)
    // avant l'annulation, pour que la reprise ait quelque chose à reprendre.
    let gros = motif(12 * 1024 * 1024 + 5, 4);
    let local = dossier_temp("envoi").join("gros.bin");
    std::fs::write(&local, &gros).unwrap();
    let sftp = sftp_de_test().await;
    let annulation: avash::sftp::Annulation = std::sync::Arc::default();
    let a = annulation.clone();
    let issue = sftp
        .upload_reprise(
            &local,
            "/fs/envoi/gros.bin",
            true,
            Some(&annulation),
            move |fait, _| {
                if fait >= 9 * 1024 * 1024 {
                    a.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            },
        )
        .await;
    assert!(issue.is_err());
    let carte = local.with_file_name("gros.bin.envoi.reprise");
    assert!(carte.exists(), "la carte d'envoi reste");
    let mut premiere = None;
    let n = sftp
        .upload_reprise(&local, "/fs/envoi/gros.bin", true, None, |fait, _| {
            premiere.get_or_insert(fait);
        })
        .await
        .unwrap();
    assert_eq!(n as usize, gros.len());
    assert!(
        premiere.unwrap_or(0) > 0,
        "la reprise repart de ce qui était fait"
    );
    assert_eq!(fs_lire("/fs/envoi/gros.bin").unwrap(), gros);
    assert!(!carte.exists());
    let _ = std::fs::remove_dir_all(local.parent().unwrap());
    sftp.close().await.unwrap();
}

/// Trouvé par l'audit du 7 septembre 2026 : un envoi de fichier unitaire faisait
/// `create(remote)`, qui tronque une cible du même nom sans un mot. Avec
/// `refuser_ecrasement`, une cible distante déjà présente fait refuser l'envoi et
/// reste intacte ; vers un chemin libre, le même envoi passe.
#[tokio::test]
async fn un_envoi_unitaire_ne_remplace_pas_une_cible_existante() {
    let sftp = sftp_de_test().await;
    let local = dossier_temp("envoi_ecrase").join("note.txt");
    std::fs::create_dir_all(local.parent().unwrap()).unwrap();
    std::fs::write(&local, b"nouveau contenu").unwrap();
    fs_poser("/fs/ecrase/note.txt", b"precieux, a garder");

    let issue = sftp
        .upload_reprise(&local, "/fs/ecrase/note.txt", true, None, |_, _| {})
        .await;
    assert!(
        issue.is_err(),
        "un envoi ne doit pas écraser une cible existante"
    );
    assert_eq!(
        fs_lire("/fs/ecrase/note.txt").as_deref(),
        Some(&b"precieux, a garder"[..]),
        "la cible existante doit rester intacte"
    );

    // Vers un chemin libre, le même envoi passe.
    let n = sftp
        .upload_reprise(&local, "/fs/ecrase/neuf.txt", true, None, |_, _| {})
        .await
        .unwrap();
    assert_eq!(n, b"nouveau contenu".len() as u64);
    assert_eq!(fs_lire("/fs/ecrase/neuf.txt").unwrap(), b"nouveau contenu");

    let _ = std::fs::remove_dir_all(local.parent().unwrap());
    sftp.close().await.unwrap();
}

/// Trouvé par l'audit du 7 septembre 2026 : `relayer_vers` lisait la taille par
/// `metadata(...).map_or(0, ...)`. Une source illisible (lien symbolique vers un
/// fichier supprimé) donnait `total = 0`, la cible était alors créée vide et la
/// copie annoncée « réussie » ; une cible préexistante était tronquée avant que
/// l'échec ne survienne. Une source illisible doit échouer SANS créer ni
/// tronquer la cible.
#[tokio::test]
async fn relayer_vers_echoue_sur_une_source_illisible_sans_toucher_la_cible() {
    let source = sftp_de_test().await;
    let cible = sftp_de_test().await;
    // Cible préexistante non vide : elle ne doit être ni tronquée ni écrasée.
    fs_poser("/fs/relaisko/garde.txt", b"contenu-precieux");

    let issue = source
        .relayer_vers(
            "/fs/relaisko/absente.log",
            &cible,
            "/fs/relaisko/garde.txt",
            false,
            None,
            |_, _| {},
        )
        .await;
    assert!(issue.is_err(), "une source illisible doit échouer");
    assert_eq!(
        fs_lire("/fs/relaisko/garde.txt").as_deref(),
        Some(&b"contenu-precieux"[..]),
        "la cible existante doit rester intacte"
    );

    // Vers une cible neuve : aucun fichier vide ne doit être créé.
    let issue2 = source
        .relayer_vers(
            "/fs/relaisko/absente2.log",
            &cible,
            "/fs/relaisko/neuf.bin",
            false,
            None,
            |_, _| {},
        )
        .await;
    assert!(issue2.is_err(), "une source illisible doit échouer");
    assert!(
        fs_lire("/fs/relaisko/neuf.bin").is_none(),
        "aucune cible vide ne doit être créée"
    );
    source.close().await.unwrap();
    cible.close().await.unwrap();
}

/// Trouvé par l'audit du 7 septembre 2026 : `relayer_vers` faisait
/// `create(remote_cible)`, qui tronque un fichier du même nom chez la cible — une
/// copie de fichier vers un autre hôte écrasait sans un mot. Avec
/// `refuser_ecrasement`, une cible existante fait refuser la copie et reste
/// intacte (le serveur en mémoire honore `OpenFlags::TRUNCATE`, la troncature
/// serait donc visible sinon).
#[tokio::test]
async fn relayer_vers_ne_remplace_pas_une_cible_existante() {
    let source = sftp_de_test().await;
    let cible = sftp_de_test().await;
    fs_poser("/fs/copie/src.bin", &motif(200 * 1024, 7));
    fs_poser("/fs/copie/deja.bin", b"a garder chez la cible");

    let issue = source
        .relayer_vers(
            "/fs/copie/src.bin",
            &cible,
            "/fs/copie/deja.bin",
            true,
            None,
            |_, _| {},
        )
        .await;
    assert!(
        issue.is_err(),
        "la copie ne doit pas écraser une cible existante"
    );
    assert_eq!(
        fs_lire("/fs/copie/deja.bin").as_deref(),
        Some(&b"a garder chez la cible"[..]),
        "la cible existante doit rester intacte"
    );
    source.close().await.unwrap();
    cible.close().await.unwrap();
}

/// Trouvé par l'audit du 9 septembre 2026 : le relais créait la cible par
/// `create()`, donc la tronquait, AVANT de lancer ses bandes, sans jamais
/// rien nettoyer ensuite. Annulé en route (bouton « Annuler » d'une ligne de
/// transfert, coupure réseau), il laissait chez le second serveur un fichier
/// tronqué et troué (les bandes écrivent à des décalages disjoints) à la place
/// de l'homologue valide, sans carte de reprise ni le moindre avertissement.
/// C'est le mode fusion (`refuser_ecrasement = false`) de la copie de DOSSIER
/// qui rendait la perte réelle : une resynchro annulée détruisait le fichier
/// déjà là. Le relais doit écrire dans un partiel chez la cible et ne toucher
/// le fichier final qu'une fois tous les octets arrivés.
#[tokio::test]
async fn le_relais_annule_ne_laisse_pas_de_fichier_tronque() {
    // Huit mégaoctets : huit bandes de seize blocs, chaque bande relit le
    // drapeau avant chaque bloc, l'annulation est vue (voir le même dosage
    // pour le téléchargement annulé, régression CI du 04/09/2026).
    let gros = motif(8 * 1024 * 1024, 9);
    fs_poser("/fs/relaispart/src.bin", &gros);
    fs_poser("/fs/relaispart/deja.bin", b"la version deja chez la cible");
    let source = sftp_de_test().await;
    let cible = sftp_de_test().await;
    let annulation: avash::sftp::Annulation = std::sync::Arc::default();
    let a = annulation.clone();
    let issue = source
        .relayer_vers(
            "/fs/relaispart/src.bin",
            &cible,
            "/fs/relaispart/deja.bin",
            // Fusion, comme la copie de dossier : c'est ce mode-là qui perdait
            // le fichier homologue déjà présent chez la cible.
            false,
            Some(&annulation),
            move |fait, _| {
                if fait >= 1024 * 1024 {
                    a.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            },
        )
        .await;
    let e = issue.expect_err("l'annulation doit se voir");
    assert!(e.to_string().contains(avash::sftp::ANNULE), "{e:#}");
    // Comparaison sans `assert_eq!` : la cible fautive porte des mégaoctets,
    // que le vidage d'un `assert_eq!` déverserait dans le journal de la chaîne.
    let apres = fs_lire("/fs/relaispart/deja.bin");
    assert!(
        apres.as_deref() == Some(&b"la version deja chez la cible"[..]),
        "un relais annulé ne doit pas laisser la cible tronquée (elle porte {} octets)",
        apres.map_or(0, |c| c.len())
    );
    assert!(
        fs_lire("/fs/relaispart/deja.bin.part").is_none(),
        "un relais annulé ne doit pas laisser de partiel chez la cible"
    );

    // Relancé sans annulation, le même relais remplace bien la cible, entière.
    let n = source
        .relayer_vers(
            "/fs/relaispart/src.bin",
            &cible,
            "/fs/relaispart/deja.bin",
            false,
            None,
            |_, _| {},
        )
        .await
        .unwrap();
    assert_eq!(n as usize, gros.len());
    assert_eq!(fs_lire("/fs/relaispart/deja.bin").unwrap(), gros);
    assert!(
        fs_lire("/fs/relaispart/deja.bin.part").is_none(),
        "le partiel disparaît quand le relais aboutit"
    );
    source.close().await.unwrap();
    cible.close().await.unwrap();
}

/// Relecture de l'audit du 9 septembre 2026 : la promotion du partiel chez la
/// cible a deux branches et le serveur en mémoire n'en empruntait qu'une. Il
/// renommait à la POSIX (`remove` puis `insert`, la cible existante écrasée
/// sans un mot) alors qu'OpenSSH refuse `SSH_FXP_RENAME` dès que la cible
/// existe. Chez un vrai serveur, la promotion passe donc TOUJOURS par « écarter
/// la cible, puis renommer », branche qui restait verte quoi qu'elle contienne
/// (une cible supprimée puis un renommage qui échoue, et l'utilisateur se
/// retrouve sans fichier). Le serveur de test refuse désormais comme OpenSSH :
/// on le vérifie d'abord, puis on exige que le relais promeuve quand même son
/// partiel par-dessus l'homologue déjà présent.
#[tokio::test]
async fn le_relais_promeut_son_partiel_meme_quand_le_serveur_refuse_le_renommage() {
    let gros = motif(300 * 1024 + 7, 11);
    fs_poser("/fs/relaisrenom/src.bin", &gros);
    fs_poser("/fs/relaisrenom/deja.bin", b"la version deja chez la cible");
    fs_poser("/fs/relaisrenom/temoin.bin", b"temoin");
    let source = sftp_de_test().await;
    let cible = sftp_de_test().await;

    // Sans ce refus, le relais sortirait par la première branche et la seconde
    // (la seule qu'emprunte un vrai serveur) resterait sans épreuve.
    assert!(
        cible
            .rename("/fs/relaisrenom/temoin.bin", "/fs/relaisrenom/deja.bin")
            .await
            .is_err(),
        "le serveur de test doit refuser de renommer sur une cible existante, comme OpenSSH"
    );

    let n = source
        .relayer_vers(
            "/fs/relaisrenom/src.bin",
            &cible,
            "/fs/relaisrenom/deja.bin",
            false,
            None,
            |_, _| {},
        )
        .await
        .expect("le relais doit promouvoir son partiel malgré le refus de renommage");
    assert_eq!(n as usize, gros.len());
    assert_eq!(fs_lire("/fs/relaisrenom/deja.bin").unwrap(), gros);
    assert!(
        fs_lire("/fs/relaisrenom/deja.bin.part").is_none(),
        "le partiel disparaît quand le relais aboutit"
    );
    source.close().await.unwrap();
    cible.close().await.unwrap();
}

/// La promotion du partiel a deux branches : un `rename` direct, atomique, quand
/// le serveur remplace une cible existante à la POSIX, sinon `remove` puis
/// `rename`. Depuis que le serveur de test refuse le renommage sur une cible
/// existante comme OpenSSH (relecture du 9 septembre 2026), la première branche
/// n'était plus éprouvée. Un segment « posix » du chemin rend ce serveur
/// complaisant, et le journal des suppressions prouve que la cible n'a jamais
/// été retirée : pas un instant sans fichier chez la cible.
#[tokio::test]
async fn le_relais_promeut_par_renommage_direct_quand_le_serveur_le_permet() {
    let gros = motif(64 * 1024 + 3, 17);
    fs_poser("/fs/posix/src.bin", &gros);
    fs_poser("/fs/posix/deja.bin", b"la version deja chez la cible");
    let source = sftp_de_test().await;
    let cible = sftp_de_test().await;
    let n = source
        .relayer_vers(
            "/fs/posix/src.bin",
            &cible,
            "/fs/posix/deja.bin",
            false,
            None,
            |_, _| {},
        )
        .await
        .expect("le relais doit aboutir sur un serveur qui renomme à la POSIX");
    assert_eq!(n as usize, gros.len());
    assert_eq!(fs_lire("/fs/posix/deja.bin").unwrap(), gros);
    assert!(
        fs_lire("/fs/posix/deja.bin.part").is_none(),
        "le partiel disparaît quand le relais aboutit"
    );
    let supprimes = FS_SUPPRESSIONS.lock().await.clone();
    assert!(
        !supprimes.iter().any(|c| c == "/fs/posix/deja.bin"),
        "sur un serveur qui renomme à la POSIX, la cible ne doit jamais être retirée : {supprimes:?}"
    );
    source.close().await.unwrap();
    cible.close().await.unwrap();
}

/// Relecture de l'audit du 9 septembre 2026 : le passage par un partiel chez la
/// cible a élargi l'ensemble des fichiers que le relais détruit sans étendre le
/// garde-fou. `refuser_ecrasement` (le mode de la copie unitaire lancée depuis
/// l'interface) ne regardait que le nom final, puis `create` du partiel le
/// TRONQUAIT : copier « rapport.pdf » effaçait sans un mot un
/// « rapport.pdf.part » présent chez la cible, là où ce mode promet précisément
/// de ne rien écraser. Et un partiel n'est pas un déchet : quand la promotion
/// échoue, on le conserve exprès parce qu'il porte les seuls octets complets.
#[tokio::test]
async fn le_relais_qui_refuse_d_ecraser_epargne_aussi_un_partiel_deja_present() {
    fs_poser("/fs/relaisgarde/src.bin", &motif(200 * 1024, 13));
    // Le partiel d'une copie précédente dont la promotion a échoué : ces octets
    // sont la seule copie complète du fichier.
    fs_poser(
        "/fs/relaisgarde/deja.bin.part",
        b"les seuls octets complets",
    );
    let source = sftp_de_test().await;
    let cible = sftp_de_test().await;

    let issue = source
        .relayer_vers(
            "/fs/relaisgarde/src.bin",
            &cible,
            "/fs/relaisgarde/deja.bin",
            true,
            None,
            |_, _| {},
        )
        .await;
    let e = issue.expect_err("la copie ne doit pas écraser un partiel déjà présent");
    assert!(
        e.to_string().contains("deja.bin.part"),
        "le refus doit nommer le fichier épargné : {e:#}"
    );
    assert_eq!(
        fs_lire("/fs/relaisgarde/deja.bin.part").as_deref(),
        Some(&b"les seuls octets complets"[..]),
        "le partiel déjà présent doit rester intact"
    );
    assert!(
        fs_lire("/fs/relaisgarde/deja.bin").is_none(),
        "aucun fichier final ne doit être créé quand la copie est refusée"
    );
    source.close().await.unwrap();
    cible.close().await.unwrap();
}

/// Un dossier passe d'un serveur à un autre par le poste sans rien y écrire :
/// deux serveurs, deux sessions, et les octets identiques à l'arrivée.
#[tokio::test]
async fn un_dossier_se_relaie_d_un_serveur_a_l_autre() {
    let gros = motif(300 * 1024 + 3, 5);
    fs_poser("/fs/relais/src/x.bin", &gros);
    fs_poser("/fs/relais/src/sous/y.txt", b"y");
    let source = sftp_de_test().await;
    let cible = sftp_de_test().await;
    let temp = std::env::temp_dir();
    let mut dernier = avash::sftp::Avancement::default();
    // Trouvé par l'audit du 8 septembre 2026 : on ne vérifiait l'absence de
    // `.part` local qu'APRÈS le transfert — un relais qui passerait par le
    // disque puis nettoierait resterait vert. On OBSERVE donc pendant le
    // transfert : à chaque progression, aucun `*.part` du relais ne doit
    // apparaître dans le répertoire temporaire (le fichier fait 300 Kio, donc
    // plusieurs bandes et plusieurs rappels).
    let mut vu_part_en_vol = false;
    let n = source
        .relayer_dir_vers("/fs/relais/src", &cible, "/fs/relais/dst", None, |a| {
            dernier = a;
            if let Ok(entrees) = std::fs::read_dir(&temp) {
                if entrees.flatten().any(|e| {
                    let nom = e.file_name();
                    let nom = nom.to_string_lossy();
                    nom.contains("x.bin.part") || nom.contains("y.txt.part")
                }) {
                    vu_part_en_vol = true;
                }
            }
        })
        .await
        .unwrap();
    assert_eq!(n as usize, gros.len() + 1);
    assert_eq!(fs_lire("/fs/relais/dst/x.bin").unwrap(), gros);
    assert_eq!(fs_lire("/fs/relais/dst/sous/y.txt").unwrap(), b"y");
    assert_eq!((dernier.termines, dernier.nombre), (2, 2));
    assert!(
        !vu_part_en_vol,
        "le relais a écrit un .part sur le disque du poste pendant le transfert"
    );
    // Et rien ne subsiste après coup non plus.
    assert!(
        !std::fs::read_dir(&temp)
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().contains("x.bin.part")),
        "le relais a laissé un .part sur le disque du poste"
    );
    source.close().await.unwrap();
    cible.close().await.unwrap();
}

/// Trouvé par l'audit du 7 septembre 2026 : un dossier distant qui contient un
/// lien symbolique (ici cassé, comme un `current -> releases/1.2.3` dont la
/// cible a disparu) faisait échouer TOUT le téléchargement. `parcourir` rangeait
/// le lien en fichier (son `file_type` n'est ni dossier ni fichier régulier),
/// puis `download_reprise` tentait d'`open`/`read` dessus, l'échec remontait par
/// `?` et les fichiers listés après le lien n'étaient jamais reçus. Le lien doit
/// désormais être ignoré (comme `scp -r` sans `-L`) et le reste du dossier arriver.
#[tokio::test]
async fn un_lien_symbolique_dans_un_dossier_ne_casse_pas_le_telechargement() {
    fs_poser("/fs/liens/a.txt", b"alpha");
    fs_poser("/fs/liens/b.txt", b"bravo");
    // Lien cassé « courant » : listé, mais ni ouvrable ni statable.
    fs_poser_lien("/fs/liens/courant", 14);
    let sftp = sftp_de_test().await;
    let local = dossier_temp("liens");
    let mut dernier = avash::sftp::Avancement::default();
    let n = sftp
        .download_dir_with("/fs/liens", &local, None, |a| {
            dernier = a;
        })
        .await
        .expect("le lien doit être ignoré, pas faire échouer le dossier");
    assert_eq!(n, b"alpha".len() as u64 + b"bravo".len() as u64);
    assert_eq!(std::fs::read(local.join("a.txt")).unwrap(), b"alpha");
    assert_eq!(std::fs::read(local.join("b.txt")).unwrap(), b"bravo");
    assert!(
        !local.join("courant").exists(),
        "le lien n'a pas à être matérialisé en local"
    );
    // Seuls les deux fichiers réguliers comptent, la progression ne compte pas le lien.
    assert_eq!((dernier.termines, dernier.nombre), (2, 2));
    let _ = std::fs::remove_dir_all(&local);
    sftp.close().await.unwrap();
}

/// Même défaut sur le relais serveur à serveur : `relayer_dir_vers` réutilise
/// `parcourir`, donc un lien cassé dans la source faisait tout échouer et, pire,
/// `relayer_vers` créait un fichier VIDE à la place du lien (total nul → `Ok`).
/// Le lien doit être ignoré et rien de faux créé chez la cible.
#[tokio::test]
async fn un_lien_symbolique_dans_un_dossier_ne_casse_pas_le_relais() {
    fs_poser("/fs/lienrelais/src/x.bin", b"donnees");
    fs_poser_lien("/fs/lienrelais/src/courant", 14);
    let source = sftp_de_test().await;
    let cible = sftp_de_test().await;
    let mut dernier = avash::sftp::Avancement::default();
    let n = source
        .relayer_dir_vers(
            "/fs/lienrelais/src",
            &cible,
            "/fs/lienrelais/dst",
            None,
            |a| {
                dernier = a;
            },
        )
        .await
        .expect("le lien doit être ignoré, pas faire échouer le relais");
    assert_eq!(n, b"donnees".len() as u64);
    assert_eq!(fs_lire("/fs/lienrelais/dst/x.bin").unwrap(), b"donnees");
    assert!(
        fs_lire("/fs/lienrelais/dst/courant").is_none(),
        "aucun fichier vide ne doit être créé à la place du lien"
    );
    assert_eq!((dernier.termines, dernier.nombre), (1, 1));
    source.close().await.unwrap();
    cible.close().await.unwrap();
}

/// Une annulation posée avant le départ arrête tout, en le disant.
#[tokio::test]
async fn un_transfert_annule_avant_de_partir_le_dit() {
    fs_poser("/fs/annule/f.txt", b"abc");
    let sftp = sftp_de_test().await;
    let annulation: avash::sftp::Annulation =
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let local = dossier_temp("annule");
    let e = sftp
        .download_dir_with("/fs/annule", &local, Some(&annulation), |_| {})
        .await
        .expect_err("annulé");
    assert!(e.to_string().contains(avash::sftp::ANNULE), "{e:#}");
    let _ = std::fs::remove_dir_all(&local);
    sftp.close().await.unwrap();
}

/// Trouvé par la revue de sécurité du commit : un serveur qui nomme une
/// entrée « ../evasion.txt » ne doit rien faire écrire hors du dossier local.
#[tokio::test]
async fn un_serveur_qui_nomme_une_entree_hors_du_dossier_est_refuse() {
    fs_dossiers()
        .as_mut()
        .unwrap()
        .insert("/fs/hostile".to_owned());
    let sftp = sftp_de_test().await;
    let local = dossier_temp("hostile").join("recu");
    let e = sftp
        .download_dir_with("/fs/hostile", &local, None, |_| {})
        .await
        .expect_err("le nom doit être refusé");
    assert!(e.to_string().contains("nom interdit"), "{e:#}");
    assert!(
        !local.parent().unwrap().join("evasion.txt").exists(),
        "un fichier a été écrit hors du dossier de réception"
    );
    let _ = std::fs::remove_dir_all(local.parent().unwrap());
    sftp.close().await.unwrap();
}

/// Sous `AVASH_HOME`, les téléchargements restent sous ce toit : le vrai dossier
/// Téléchargements de l'utilisateur ne doit pas recevoir les fichiers d'un bac à
/// sable (cinquième passage Windows de la suite bout en bout, 05/09/2026, où
/// `dirs::download_dir()` ignorait la variable). Le sous-dossier des
/// téléchargements est pris dès qu'il existe.
#[test]
fn sous_avash_home_les_telechargements_restent_sous_le_foyer() {
    std::sync::LazyLock::force(&HOME_POSE);
    let foyer = std::path::PathBuf::from(std::env::var_os("AVASH_HOME").unwrap());
    assert!(
        avash::sftp::default_local_dir().starts_with(&foyer),
        "{:?} hors de {foyer:?}",
        avash::sftp::default_local_dir()
    );
    let telechargements = foyer.join("Téléchargements");
    std::fs::create_dir_all(&telechargements).unwrap();
    assert_eq!(avash::sftp::default_local_dir(), telechargements);
}

/// Le plafond de sortie d'une commande est d'un mébioctet, pas de deux
/// kibioctets : trois kilo-octets reviennent entiers, par `run` comme par
/// `run_avec_agent`, qui rend aussi la sortie et le code. Mutants survivants
/// du premier passage de cargo-mutants (05/09/2026) : `run_avec_agent`
/// remplacé par `Ok((String::new(), 0))`, `1024 * 1024` par `1024 + 1024`,
/// `>=` par `<` sur le plafond.
#[tokio::test]
async fn les_deux_executions_rendent_toute_une_sortie_de_trois_kilo_octets() {
    let port = spawn_test_sshd().await;
    let auth = test_auth();
    let mut session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion");
    let long = "x".repeat(3000);
    let (out, code) = session.run(&format!("echo {long}")).await.unwrap();
    assert_eq!(code, 0);
    assert!(
        out.contains(&long) && !out.contains("tronquée"),
        "sortie coupée : {} octets",
        out.len()
    );
    let (out, code) = session
        .run_avec_agent(&format!("echo {long} ; exit 7"), None)
        .await
        .unwrap();
    assert_eq!(code, 7, "le code de sortie de run_avec_agent");
    assert!(
        out.contains("CMD:echo") && out.contains(&long) && !out.contains("tronquée"),
        "sortie de run_avec_agent : {} octets",
        out.len()
    );
    session.disconnect().await.unwrap();
}

/// Trouvé par l'audit du 7 septembre 2026 : `run_avec_agent` initialisait le
/// code de sortie à 0 et ne le changeait que sur `ExitStatus`. Une copie
/// directe (scp) interrompue — canal fermé sans statut — rendait donc 0 et se
/// voyait « réussie ». Sans statut de sortie, la commande doit échouer.
#[tokio::test]
async fn run_avec_agent_echoue_quand_le_canal_ferme_sans_statut() {
    let port = spawn_test_sshd().await;
    let auth = test_auth();
    let session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion");
    // Le serveur de test ferme le canal sans exit-status sur ce marqueur.
    let err = session
        .run_avec_agent("echo SANS_STATUT", None)
        .await
        .expect_err("une fermeture sans statut doit échouer");
    assert!(
        err.to_string().contains("sans code de sortie"),
        "message inattendu : {err}"
    );
    // Une commande normale, elle, rend toujours son code.
    let (_out, code) = session
        .run_avec_agent("echo ok ; exit 3", None)
        .await
        .unwrap();
    assert_eq!(code, 3);
}

/// Trouvé par l'audit du 9 septembre 2026 : dans la boucle de `run_avec_agent`,
/// tous les chemins de sortie (plafond de 1 Mio, `Close`, annulation) sortaient
/// par le bas, là où `channel.close().await` referme le canal : le commentaire
/// voisin promettait d'ailleurs une fermeture « sur TOUTE sortie de la boucle ».
/// Tous sauf un : le bras `ExitSignal` faisait un `return Err(...)` direct, qui
/// sautait cette fermeture. Or `russh::Channel` n'envoie PAS de `CHANNEL_CLOSE` à
/// sa chute et le client réalimente sa fenêtre de réception quoi qu'il arrive :
/// une copie directe (scp lancé chez la source) tuée par un signal laissait
/// derrière elle un canal exec que le serveur pouvait continuer d'alimenter à
/// plein débit toute la vie de la session. Exactement le défaut déjà corrigé le
/// 7 septembre pour les autres sorties de cette même boucle, resté ici.
///
/// Le serveur de test émet `exit-signal` PUIS garde le canal ouvert en
/// continuant d'émettre : seule la fermeture par le client peut l'arrêter, et
/// `close_recu` en est la preuve (il n'est levé que par un vrai `CHANNEL_CLOSE`
/// venu du client, pas par la chute silencieuse du canal).
#[tokio::test]
async fn run_avec_agent_ferme_le_canal_quand_la_commande_est_tuee_par_un_signal() {
    use std::sync::atomic::Ordering;
    let (port, etat) = spawn_test_sshd_inondation().await;
    let auth = test_auth();
    let session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion");
    // On repart de zéro : seule la fermeture du canal de CETTE commande compte.
    etat.close_recu.store(false, Ordering::SeqCst);
    let err = session
        .run_avec_agent("echo SIGNAL_SANS_CLOTURE", None)
        .await
        .expect_err("une commande tuée par un signal ne doit pas passer pour un succès");
    assert!(
        err.to_string().contains("interrompue par un signal"),
        "le message doit nommer le signal : {err}"
    );
    // La fermeture part juste avant que l'erreur ne remonte ; on laisse au
    // serveur le temps de la constater plutôt que de courir après l'ordonnanceur.
    for _ in 0..100 {
        if etat.close_recu.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        etat.close_recu.load(Ordering::SeqCst),
        "le canal exec doit être refermé même quand la commande meurt d'un signal : \
         sans CHANNEL_CLOSE, le serveur continue d'alimenter la fenêtre ({} octets déjà remis)",
        etat.octets.load(Ordering::SeqCst)
    );
}

/// Trouvé par l'audit du 7 septembre 2026 : `run_avec_agent` faisait
/// `agent_forward(true)` sans consommer le verdict du serveur, et la boucle exec
/// ignorait `ChannelMsg::Failure` (`_ => {}`). Contre un serveur durci
/// `AllowAgentForwarding no` (`CHANNEL_FAILURE`), le refus était avalé :
/// Avash
/// lançait quand même `scp … autre:`, qui échouait « Permission denied » en
/// accusant la clé, jamais le refus de redirection. Le verdict est désormais
/// consommé AVANT `exec` : un refus rend une erreur qui nomme la vraie cause,
/// une acceptation laisse la commande se dérouler normalement.
///
/// Le simulacre émet un vrai `CHANNEL_SUCCESS`/`CHANNEL_FAILURE` dans
/// `agent_request`
/// (« sans-agent » = refus) : le défaut de russh 0.63 répondrait par un message
/// GLOBAL invisible du client, jamais par un verdict de canal (cf. le
/// commentaire du handler).
#[tokio::test]
async fn run_avec_agent_signale_le_refus_de_redirection_d_agent() {
    let port = spawn_test_sshd().await;
    // Compte refusé pour la redirection : le serveur répond CHANNEL_FAILURE.
    let auth_refuse = avash::ssh::ClientAuth {
        user: "sans-agent".into(),
        key_path: Some(temp_key_path()),
        password: None,
    };
    let session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth_refuse)
        .await
        .expect("connexion");
    let err = session
        .run_avec_agent("true", None)
        .await
        .expect_err("un refus de redirection d'agent doit échouer");
    assert!(
        err.to_string().contains("refuse la redirection d'agent"),
        "le message doit nommer le refus de redirection, pas la clé : {err}"
    );

    // Le chemin passant, lui, se déroule : le serveur accepte la redirection
    // (CHANNEL_SUCCESS) et la commande rend sa sortie et son code.
    let auth_ok = test_auth();
    let session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth_ok)
        .await
        .expect("connexion");
    let (out, code) = session
        .run_avec_agent("echo ok ; exit 5", None)
        .await
        .expect("une redirection acceptée laisse la commande aboutir");
    assert_eq!(code, 5);
    assert!(out.contains("CMD:echo"), "sortie inattendue : {out}");
}

/// Se déconnecter ferme la session, et `is_closed` le dit.
#[tokio::test]
async fn la_deconnexion_ferme_la_session() {
    let port = spawn_test_sshd().await;
    let auth = test_auth();
    let session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion");
    assert!(!session.is_closed(), "ouverte tant qu'on ne l'a pas fermée");
    session.disconnect().await.unwrap();
    let echeance = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    while !session.is_closed() {
        assert!(
            tokio::time::Instant::now() < echeance,
            "la session ne se ferme pas après disconnect"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// Un marqueur `@revoked` sur l'hôte dans `known_hosts` bloque la connexion
/// avant toute clé, par la vraie lecture du fichier et pas seulement par la
/// fonction pure (mutant survivant : `marqueur_bloquant` remplacé par `None`).
/// L'hôte visé est « localhost », résolu vers le serveur de test : les autres
/// tests parlent à 127.0.0.1 et ne voient pas la ligne ; russh, lui, lit
/// « @revoked » comme un hôte qui ne correspond à rien.
#[tokio::test]
async fn un_hote_marque_revoked_dans_known_hosts_est_refuse() {
    std::sync::LazyLock::force(&HOME_POSE);
    let port = spawn_test_sshd().await;
    let chemin = avash::ssh::chemin_known_hosts().unwrap();
    std::fs::create_dir_all(chemin.parent().unwrap()).unwrap();
    let cle =
        russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
    let ligne = format!(
        "@revoked localhost {}\n",
        cle.public_key().to_openssh().unwrap()
    );
    {
        use std::io::Write as _;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&chemin)
            .unwrap()
            .write_all(ligne.as_bytes())
            .unwrap();
    }
    let auth = test_auth();
    let issue = avash::ssh::AvashSession::connect("localhost", port, &auth).await;
    let Err(e) = issue else {
        panic!("un hôte révoqué a été accepté");
    };
    assert!(format!("{e:#}").contains("@revoked"), "{e:#}");
}

/// Une redirection distante demandée sur le port 0 prend le port que le
/// serveur choisit (40 000 chez le serveur de test), et c'est celui-là qui est
/// rendu et qui relaie (mutants survivants sur `port == 0 && bound != 0`).
#[tokio::test]
async fn une_redirection_distante_en_port_zero_prend_le_port_du_serveur() {
    let local = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let local_port = local.local_addr().unwrap().port();
    // Réponse marquée par le port local : le statique REMOTE_REPLY est partagé
    // avec `tunnel_distant_relaie_vers_un_service_local` (audit du 7 septembre
    // 2026), qui poussait le même « HELLO » ; la moitié « relaie » de ce test
    // était alors satisfaite par la réponse de l'AUTRE test, sans jamais lire
    // les compteurs de son propre tunnel.
    let attendu = format!("HELLO-{local_port}");
    let reponse = attendu.clone();
    tokio::spawn(async move {
        let (mut s, _) = local.accept().await.unwrap();
        let mut buf = [0u8; 16];
        let _ = s.read(&mut buf).await.unwrap();
        s.write_all(reponse.as_bytes()).await.unwrap();
    });
    let port = spawn_test_sshd().await;
    let session = connect_for_tunnel(port).await;
    // Directement par la session : la définition d'un tunnel refuse le port 0
    // (validation_refuse_un_port_d_ecoute_a_zero), c'est la couche SSH qui
    // sait laisser le serveur choisir.
    // Compteurs de CE tunnel : c'est le vrai garde-fou. Sous un mutant qui
    // garderait le calcul de `bound` mais supprimerait la ré-indexation
    // `f.insert(bound, dest)` (ssh.rs), la table reste clée sur 0, le canal
    // forwarded-tcpip sur 40 000 est refusé et ces compteurs restent à zéro,
    // quel que soit l'état de REMOTE_REPLY.
    let compteurs = Arc::new(avash::ssh::ForwardCounters::default());
    let bound = session
        .remote_forward("localhost", 0, "127.0.0.1", local_port, compteurs.clone())
        .await
        .expect("redirection distante en port 0");
    assert_eq!(bound, 40_000, "le port choisi par le serveur");
    // Le relais est asynchrone : attente bornée sur l'Arc<ForwardCounters>
    // (attendre_compteurs ne prend qu'un &Tunnel). `bytes_up` = « hello » du
    // serveur vers le service local (5), `bytes_down` = notre réponse marquée
    // vers le serveur ; cf. ForwardCounters::relay(a = canal serveur, b = local).
    let attendu_len = attendu.len() as u64;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        use std::sync::atomic::Ordering::Relaxed;
        let compteurs_ok = compteurs.total.load(Relaxed) == 1
            && compteurs.bytes_up.load(Relaxed) == 5
            && compteurs.bytes_down.load(Relaxed) == attendu_len;
        let reply_ok = REMOTE_REPLY
            .lock()
            .await
            .iter()
            .any(|r| r == attendu.as_bytes());
        if compteurs_ok && reply_ok {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "le relais en port 0 n'a pas atteint MON service : total={}, up={}, down={}",
            compteurs.total.load(Relaxed),
            compteurs.bytes_up.load(Relaxed),
            compteurs.bytes_down.load(Relaxed),
        );
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
}

/// Trouvé par l'audit du 7 septembre 2026 : la garde
/// `server_channel_open_agent_forward` — qui empêche un serveur déjà accepté
/// par TOFU d'emprunter l'agent du poste hors d'une commande explicitement
/// lancée avec redirection — n'avait AUCUN test sur son chemin de refus. Un
/// mutant qui inversait le drapeau (`!load` -> `load`), l'initialisait à
/// `true`, ou vidait le `Drop` de `GardeAgent`, survivait à toute la suite.
///
/// Le serveur de test ouvre un canal d'agent hors commande (`run`) puis pendant
/// une commande à redirection (`run_avec_agent`) et rapporte le verdict rendu
/// par le client. Les DEUX moitiés sont nécessaires : le `Drop` du handle de
/// russh rejette déjà en `AdministrativelyProhibited` quand la réponse est
/// simplement lâchée, donc « refusé hors commande » seul ne prouverait pas que
/// le drapeau commande le prêt. C'est le passage à `ConnectFailed` sous
/// `run_avec_agent` (drapeau levé, la garde atteint l'agent, absent ici) qui le
/// prouve. La troisième sonde vérifie la retombée : le `Drop` de `GardeAgent`
/// remet le drapeau à false, et l'agent n'est plus prêté ensuite.
#[cfg(unix)]
#[tokio::test]
async fn un_canal_d_agent_hors_commande_est_refuse_mais_prete_le_temps_d_une_commande() {
    // `HOME_POSE` a posé `SSH_AUTH_SOCK` sur un chemin inexistant : aucun agent
    // n'est joignable, la garde refuse en `ConnectFailed` quand le drapeau est
    // levé, sans dépendre de l'agent réel du poste ni le muter.
    std::sync::LazyLock::force(&HOME_POSE);
    let port = spawn_test_sshd().await;
    let auth = test_auth();
    let mut session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion");

    // 1) Hors commande : le serveur ne doit pas obtenir l'agent du poste.
    let (out, _) = session.run("sonde-agent").await.unwrap();
    assert!(
        out.contains("AdministrativelyProhibited"),
        "hors commande, le canal d'agent doit être administrativement refusé : {out}"
    );

    // 2) Pendant `run_avec_agent` : le drapeau est levé, la garde laisse passer
    // et échoue plus loin faute d'agent joignable — verdict distinct, preuve que
    // le drapeau a bien changé avec la commande.
    let (out, _) = session.run_avec_agent("sonde-agent", None).await.unwrap();
    assert!(
        out.contains("ConnectFailed"),
        "pendant une commande à redirection, la garde doit atteindre l'agent (absent ici) : {out}"
    );

    // 3) Retombée : après la commande, le `Drop` de `GardeAgent` a remis le
    // drapeau à false ; un canal d'agent est de nouveau refusé.
    let (out, _) = session.run("sonde-agent").await.unwrap();
    assert!(
        out.contains("AdministrativelyProhibited"),
        "après la commande, l'agent ne doit plus être prêté : {out}"
    );

    session.disconnect().await.unwrap();
}

/// Jumeau du précédent pour `server_channel_open_forwarded_tcpip` : le serveur
/// ouvre un canal `forwarded-tcpip` sur un port JAMAIS enregistré par
/// `remote_forward`, et le client doit refuser cette destination locale
/// arbitraire. Trouvé par l'audit du 7 septembre 2026 : seul le chemin
/// acceptant (port enregistré, cf. `une_redirection_distante_en_port_zero...`)
/// était testé, le chemin de refus ne l'était pas.
///
/// Le refus se lit `ConnectFailed` (et non `AdministrativelyProhibited`) : c'est
/// le code de rejet que pose `server_channel_open_forwarded_tcpip` pour une
/// destination inconnue (ssh.rs). Ce qui compte est que le canal soit REFUSÉ
/// (jamais `SONDE:ok`, qui signifierait un relais accordé) : un mutant qui
/// accepterait le canal produirait `SONDE:ok` et ferait rougir l'assertion.
#[tokio::test]
async fn un_forwarded_tcpip_sur_un_port_non_enregistre_est_refuse() {
    let port = spawn_test_sshd().await;
    let auth = test_auth();
    let mut session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion");
    let (out, _) = session.run("sonde-forward").await.unwrap();
    assert!(
        out.contains("ChannelOpenFailure(ConnectFailed)"),
        "un forwarded-tcpip sur un port non enregistré doit être refusé : {out}"
    );
    session.disconnect().await.unwrap();
}

/// Trouvé par l'audit du 9 septembre 2026 : `executer_borne` (le corps commun
/// de `run` et `run_borne`) ignorait `ChannelMsg::ExitSignal` dans son `_ => {}`
/// et partait d'un code de sortie à 0. Une commande tuée par un signal côté
/// distant (OOM-killer, `kill -9`, SIGPIPE) était donc rapportée `(sortie, 0)`,
/// c'est-à-dire réussie : le déploiement de clé publique (`key_deploy`, qui
/// teste `code == 0`) annonçait un succès pour un `ssh-copy-id` tué en route, et
/// l'exécution de commande affichait « [exit 0] ». Le correctif existait déjà
/// pour `run_avec_agent`, il n'avait jamais été porté ici.
#[tokio::test]
async fn run_echoue_quand_la_commande_est_tuee_par_un_signal() {
    let port = spawn_test_sshd().await;
    let auth = test_auth();
    let mut session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion");
    // Le serveur de test répond exit-signal (et jamais exit-status) sur ce
    // marqueur, comme le fait un vrai serveur pour un processus tué.
    let err = session
        .run("echo TUE_PAR_SIGNAL")
        .await
        .expect_err("une commande tuée par un signal ne doit pas passer pour un succès");
    assert!(
        err.to_string().contains("interrompue par un signal"),
        "le message doit nommer le signal : {err}"
    );
    // Même verdict par le chemin borné, qui partage la même boucle.
    let err = session
        .run_borne("echo TUE_PAR_SIGNAL", std::time::Duration::from_secs(5))
        .await
        .expect_err("run_borne doit refuser tout autant");
    assert!(
        err.to_string().contains("interrompue par un signal"),
        "le message doit nommer le signal : {err}"
    );
    // Une commande normale rend toujours sa sortie et son code.
    let (out, code) = session.run("echo ok ; exit 4").await.expect("cas nominal");
    assert_eq!(code, 4);
    assert!(out.contains("CMD:echo"), "sortie inattendue : {out}");
    session.disconnect().await.unwrap();
}

/// Trouvé par l'audit du 9 septembre 2026, même famille que le test précédent :
/// `executer_borne` n'avait aucun équivalent du `statut_recu` de
/// `run_avec_agent`. Un canal fermé sans le moindre `exit-status` (lien coupé,
/// processus disparu) rendait donc 0, un succès inventé. Sans statut de sortie,
/// on ne conclut pas au succès.
#[tokio::test]
async fn run_echoue_quand_le_canal_ferme_sans_statut() {
    let port = spawn_test_sshd().await;
    let auth = test_auth();
    let mut session = avash::ssh::AvashSession::connect("127.0.0.1", port, &auth)
        .await
        .expect("connexion");
    let err = session
        .run("echo SANS_STATUT")
        .await
        .expect_err("une fermeture sans statut doit échouer");
    assert!(
        err.to_string().contains("sans code de sortie"),
        "message inattendu : {err}"
    );
    session.disconnect().await.unwrap();
}

// ---------- Audit du 12 septembre 2026 ----------

/// Contrat K6 de l'audit du 12 septembre 2026 (C-perf-4) : quand la sortie du
/// terminal sature (le front ne suit pas), le pump attendait DANS le bras de
/// sortie que le canal se libère, et le bras du clavier n'était plus servi :
/// Ctrl+C restait dans la file. Ici personne ne lit la sortie ; la frappe doit
/// quand même atteindre le serveur.
#[tokio::test]
async fn une_frappe_passe_pendant_que_la_sortie_sature() {
    let serveur = lancer_serveur().await;
    let mut session = connect_for_tunnel(serveur.port).await;
    let pty = session.open_pty(80, 24, "rafale").await.unwrap();
    let echeance = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while pty.out_rx.len() < 256 {
        assert!(
            tokio::time::Instant::now() < echeance,
            "le canal de sortie ne s'est jamais rempli ({} blocs)",
            pty.out_rx.len()
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    pty.in_tx.send(b"\x03".to_vec()).await.unwrap();
    let echeance = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    while !serveur.frappes.lock().unwrap().contains(&0x03) {
        assert!(
            tokio::time::Instant::now() < echeance,
            "Ctrl+C n'a pas atteint le serveur pendant que la sortie saturait"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    drop(pty);
}

/// Audit du 12 septembre 2026 (C-perf-3 a) : un petit fichier lisait ses
/// attributs deux fois (`download_reprise`, puis `download_with` sur le même
/// chemin), un aller-retour de plus par fichier d'un dossier.
#[tokio::test]
async fn un_petit_fichier_ne_lit_ses_attributs_qu_une_fois() {
    fs_poser("/fs/stat-unique/a.txt", b"alpha");
    let sftp = sftp_de_test().await;
    let local = dossier_temp("stat-unique");
    let n = sftp
        .download_reprise(
            "/fs/stat-unique/a.txt",
            &local.join("a.txt"),
            None,
            |_, _| {},
        )
        .await
        .unwrap();
    assert_eq!(n, 5);
    assert_eq!(fs_stats_de("/fs/stat-unique/a.txt"), 1, "un seul STAT");
    let _ = std::fs::remove_dir_all(&local);
    sftp.close().await.unwrap();
}

/// Huit petits fichiers sous `racine`, au contenu propre à chacun.
fn huit_petits_fichiers(racine: &str) -> Vec<(String, Vec<u8>)> {
    (0..8)
        .map(|i| {
            let chemin = format!("{racine}/f{i}.txt");
            let contenu = format!("contenu {i}").into_bytes();
            fs_poser(&chemin, &contenu);
            (chemin, contenu)
        })
        .collect()
}

/// Audit du 12 septembre 2026 (C-perf-3 b) : un dossier passait un fichier
/// après l'autre, cinq allers-retours strictement séquentiels par petit
/// fichier. Plusieurs fichiers sont désormais en vol à la fois.
#[tokio::test]
async fn un_dossier_de_petits_fichiers_garde_plusieurs_descripteurs_ouverts_a_la_fois() {
    let fichiers = huit_petits_fichiers("/fs/en-vol-dl");
    let sftp = sftp_de_test().await;
    let local = dossier_temp("en-vol-dl");
    let mut dernier = avash::sftp::Avancement::default();
    let n = sftp
        .download_dir_with("/fs/en-vol-dl", &local, None, |a| dernier = a)
        .await
        .unwrap();
    for (chemin, contenu) in &fichiers {
        let nom = chemin.rsplit('/').next().unwrap();
        assert_eq!(&std::fs::read(local.join(nom)).unwrap(), contenu, "{nom}");
    }
    let attendu: usize = fichiers.iter().map(|(_, c)| c.len()).sum();
    assert_eq!(n as usize, attendu);
    assert_eq!((dernier.termines, dernier.nombre, dernier.fait), (8, 8, n));
    let max = fs_max_ouverts_sous("/fs/en-vol-dl/");
    assert!(max > 1, "un seul descripteur ouvert à la fois ({max})");
    let _ = std::fs::remove_dir_all(&local);
    sftp.close().await.unwrap();
}

/// Même promesse pour l'envoi d'un dossier.
#[tokio::test]
async fn un_envoi_de_dossier_garde_plusieurs_fichiers_en_vol() {
    let local = dossier_temp("en-vol-up");
    for i in 0..8 {
        std::fs::write(local.join(format!("f{i}.txt")), format!("envoi {i}")).unwrap();
    }
    let sftp = sftp_de_test().await;
    let mut dernier = avash::sftp::Avancement::default();
    sftp.upload_dir_with(&local, "/fs/en-vol-up", None, |a| dernier = a)
        .await
        .unwrap();
    for i in 0..8 {
        assert_eq!(
            fs_lire(&format!("/fs/en-vol-up/f{i}.txt")).unwrap(),
            format!("envoi {i}").into_bytes()
        );
    }
    assert_eq!((dernier.termines, dernier.nombre), (8, 8));
    let max = fs_max_ouverts_sous("/fs/en-vol-up/");
    assert!(max > 1, "un seul fichier en vol à la fois ({max})");
    let _ = std::fs::remove_dir_all(&local);
    sftp.close().await.unwrap();
}

/// Même promesse pour le relais d'un dossier d'un serveur à l'autre.
#[tokio::test]
async fn un_relais_de_dossier_garde_plusieurs_fichiers_en_vol() {
    let fichiers = huit_petits_fichiers("/fs/en-vol-src");
    let source = sftp_de_test().await;
    let cible = sftp_de_test().await;
    let mut dernier = avash::sftp::Avancement::default();
    source
        .relayer_dir_vers("/fs/en-vol-src", &cible, "/fs/en-vol-dst", None, |a| {
            dernier = a;
        })
        .await
        .unwrap();
    for (chemin, contenu) in &fichiers {
        let copie = chemin.replace("en-vol-src", "en-vol-dst");
        assert_eq!(&fs_lire(&copie).unwrap(), contenu, "{copie}");
    }
    assert_eq!((dernier.termines, dernier.nombre), (8, 8));
    let max = fs_max_ouverts_sous("/fs/en-vol-src/");
    assert!(max > 1, "un seul fichier relayé à la fois ({max})");
    source.close().await.unwrap();
    cible.close().await.unwrap();
}

/// Audit du 12 septembre 2026 (C-perf-3 b) : avec plusieurs fichiers en vol,
/// une erreur sur l'un arrête les autres, qui ne laissent aucun partiel, et
/// l'erreur rendue est la vraie cause, pas l'« annulé » de ceux qu'elle arrête.
#[tokio::test]
async fn un_fichier_refuse_arrete_le_dossier_sans_laisser_de_partiel() {
    huit_petits_fichiers("/fs/err-dl");
    fs_poser("/fs/err-dl/x-refus.txt", b"interdit");
    let sftp = sftp_de_test().await;
    let local = dossier_temp("err-dl");
    let e = sftp
        .download_dir_with("/fs/err-dl", &local, None, |_| {})
        .await
        .expect_err("un fichier refusé fait échouer le dossier");
    assert!(format!("{e:#}").contains("x-refus.txt"), "{e:#}");
    assert!(!format!("{e:#}").contains(avash::sftp::ANNULE), "{e:#}");
    let partiels: Vec<_> = std::fs::read_dir(&local)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".part"))
        .collect();
    assert!(partiels.is_empty(), "partiels laissés : {partiels:?}");
    let _ = std::fs::remove_dir_all(&local);
    sftp.close().await.unwrap();
}

/// Un répertoire personnel vierge, avec `~/.ssh/config` pour le binaire.
fn bac_cli(nom: &str, config: &str) -> std::path::PathBuf {
    let bac = dossier_temp(nom);
    std::fs::create_dir_all(bac.join(".ssh")).unwrap();
    std::fs::write(bac.join(".ssh").join("config"), config).unwrap();
    bac
}

/// Audit du 12 septembre 2026 (C-couv-4.9) : `avash run` (connexion réelle)
/// n'avait aucun test. Le binaire se connecte au serveur de test, exécute, rend
/// le code du serveur, et apprend la clé d'hôte au premier contact (TOFU).
#[tokio::test]
async fn avash_run_execute_et_rend_le_code_du_serveur() {
    let port = spawn_test_sshd().await;
    let bac = bac_cli(
        "avash-run",
        &format!(
            "Host prod\n  HostName 127.0.0.1\n  Port {port}\n  User testuser\n  IdentityFile {}\n",
            temp_key_path().display()
        ),
    );
    // `tokio::process` et non `std` : le serveur tourne sur ce même runtime,
    // une attente bloquante l'empêcherait de répondre.
    let sortie = tokio::process::Command::new(env!("CARGO_BIN_EXE_avash"))
        .args(["run", "prod", "echo ok ; exit 4"])
        .env("AVASH_HOME", &bac)
        .env("HOME", &bac)
        .env("SSH_AUTH_SOCK", bac.join("pas-d-agent.sock"))
        .output()
        .await
        .unwrap();
    let stdout = String::from_utf8_lossy(&sortie.stdout);
    assert_eq!(
        sortie.status.code(),
        Some(4),
        "stdout : {stdout}\nstderr : {}",
        String::from_utf8_lossy(&sortie.stderr)
    );
    assert!(stdout.contains("CMD:echo ok ; exit 4"), "{stdout}");
    let appris = std::fs::read_to_string(bac.join(".ssh").join("known_hosts")).unwrap();
    assert!(appris.contains(&format!("[127.0.0.1]:{port}")), "{appris}");
    let _ = std::fs::remove_dir_all(&bac);
}

/// Audit du 12 septembre 2026 (C-SIL-5, C-reseau-1) : un port qui accepte le
/// TCP et se tait laissait `avash run` (et l'onglet, le tunnel, le dépôt de
/// clé) attendre sans fin. `AVASH_SSH_DELAI` règle le délai de garde.
#[tokio::test]
async fn un_serveur_muet_fait_echouer_avash_run_en_temps_borne() {
    let ecoute = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = ecoute.local_addr().unwrap().port();
    tokio::spawn(async move {
        let mut gardees = Vec::new();
        while let Ok((flux, _)) = ecoute.accept().await {
            gardees.push(flux);
        }
    });
    let bac = bac_cli(
        "avash-run-muet",
        &format!("Host muet\n  HostName 127.0.0.1\n  Port {port}\n  User u\n"),
    );
    let debut = std::time::Instant::now();
    let lancement = tokio::process::Command::new(env!("CARGO_BIN_EXE_avash"))
        .args(["run", "muet", "true"])
        .env("AVASH_HOME", &bac)
        .env("HOME", &bac)
        .env("AVASH_SSH_DELAI", "1")
        .env("SSH_AUTH_SOCK", bac.join("pas-d-agent.sock"))
        .kill_on_drop(true)
        .output();
    let sortie = tokio::time::timeout(std::time::Duration::from_secs(10), lancement)
        .await
        .expect("avash run doit rendre la main en temps borné")
        .unwrap();
    assert!(!sortie.status.success());
    assert!(
        debut.elapsed() < std::time::Duration::from_secs(6),
        "{:?}",
        debut.elapsed()
    );
    let stderr = String::from_utf8_lossy(&sortie.stderr);
    assert!(stderr.contains(&format!("127.0.0.1:{port}")), "{stderr}");
    assert!(stderr.contains("n'a pas répondu en 1 s"), "{stderr}");
    let _ = std::fs::remove_dir_all(&bac);
}
