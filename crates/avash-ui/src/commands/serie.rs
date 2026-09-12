//! Sessions série : le port du poste dans un onglet de terminal.
//!
//! Une session série est une session comme les autres pour le front et pour
//! le magasin : mêmes canaux clavier et sortie, même pump vers `pty-output`,
//! mêmes `pty_write` et `pty_close`. Elle n'a ni SFTP ni commande à distance ;
//! ce qu'on lui demande d'autre répond clairement qu'un port série ne le fait
//! pas.

use super::sessions::{
    enregistrer_session, lancer_relais, Enregistrement, SessionHandle, SessionStore, SESSION_EPOCH,
};
use avash::Verrou as _;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use tauri::AppHandle;

/// Les ports série du poste.
///
/// Contrat K2 de l'audit du 12 septembre 2026 : une énumération qui échoue
/// (udev ou registre illisible) rend une erreur, que le formulaire affiche,
/// au lieu d'une liste vide qu'il présentait comme « aucun port ».
#[tauri::command(async)]
pub fn serie_ports() -> Result<Vec<avash::serie::PortSerie>, String> {
    avash::serie::lister_ports().map_err(|e| format!("{e:#}"))
}

/// Ouvre un port série dans l'onglet `id` et rend le libellé de l'onglet.
///
/// Générique sur le moteur depuis l'audit du 12 septembre 2026 (C-couv-5) :
/// la poignée du moteur factice ne s'y passait pas, et la commande, qui tient
/// `en_cours`, `annules` et le relais, n'avait aucun test.
#[tauri::command]
pub async fn serie_open<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, SessionStore>,
    id: u64,
    chemin: String,
    vitesse: u32,
) -> Result<String, String> {
    state.en_cours.verrou().insert(id);
    // L'ouverture touche au pilote (et, sous Windows, au registre) : hors des
    // fils du runtime.
    let ouverture =
        super::bloquant(move || avash::serie::ouvrir(&chemin, vitesse).map_err(|e| e.to_string()))
            .await;
    let session = match ouverture {
        Ok(s) => s,
        Err(e) => {
            state.en_cours.verrou().remove(&id);
            state.annules.verrou().remove(&id);
            return Err(e);
        }
    };
    let label = session.label.clone();
    // Un port série n'a pas de taille de fenêtre : le canal reçoit et ignore.
    let (resize, _resize_rx) = tokio::sync::mpsc::channel::<(u32, u32)>(1);
    let epoch = SESSION_EPOCH.fetch_add(1, Ordering::Relaxed);
    let enregistreur: Enregistrement = std::sync::Arc::new(Mutex::new(None));
    enregistrer_session(
        &state,
        id,
        SessionHandle {
            epoch,
            input: session.in_tx,
            resize,
            sftp: Mutex::new(None),
            ouvrir_sftp: std::sync::Arc::new(|| {
                Box::pin(async { Err("Pas de SFTP sur un port série.".to_owned()) })
            }),
            executer: std::sync::Arc::new(|_, _| {
                Box::pin(async {
                    Err("Pas de commande à distance sur un port série.".to_owned())
                })
            }),
            label: label.clone(),
            // Aucune adresse : un autre hôte ne peut pas joindre un port série.
            cible: (String::new(), 0, String::new()),
            enregistreur: enregistreur.clone(),
        },
    )?;
    // Même relais que SSH (regroupement, contre-pression, garde de fin de
    // session) ; ni sonde d'OS ni déconnexion à attendre.
    let _relais = lancer_relais(
        app,
        id,
        epoch,
        session.out_rx,
        enregistreur,
        async {},
        async {},
    );
    Ok(label)
}

/// Tests de `serie_open` sur un pseudo-terminal : l'esclave tient lieu de port
/// série, le maître de l'équipement. Audit du 12 septembre 2026 (C-couv-5) :
/// la commande, qui tient `en_cours`, `annules`, la poignée et le relais,
/// n'avait aucun test (3,57 % de couverture).
#[cfg(test)]
#[cfg(target_os = "linux")]
mod tests_serie {
    use super::serie_open;
    use crate::commands::tests::{app_de_test, ecouter};
    use crate::commands::{
        open_sessions, pty_close, pty_write, sftp_of, SessionStore, CONNEXION_ANNULEE,
    };
    use std::io::{Read as _, Write as _};
    use tauri::Manager as _;

    /// Un pseudo-terminal : le maître, et le chemin de l'esclave.
    fn pty() -> (nix::pty::PtyMaster, String) {
        use nix::fcntl::OFlag;
        let maitre = nix::pty::posix_openpt(OFlag::O_RDWR | OFlag::O_NOCTTY).unwrap();
        nix::pty::grantpt(&maitre).unwrap();
        nix::pty::unlockpt(&maitre).unwrap();
        let chemin = nix::pty::ptsname_r(&maitre).unwrap();
        (maitre, chemin)
    }

    async fn ouvrir(
        app: &tauri::App<tauri::test::MockRuntime>,
        id: u64,
        chemin: &str,
        vitesse: u32,
    ) -> Result<String, String> {
        serie_open(
            app.handle().clone(),
            app.state::<SessionStore>(),
            id,
            chemin.to_owned(),
            vitesse,
        )
        .await
    }

    fn en_cours_vide(app: &tauri::App<tauri::test::MockRuntime>) -> bool {
        let s = app.state::<SessionStore>();
        let vide = s.en_cours.lock().unwrap().is_empty() && s.annules.lock().unwrap().is_empty();
        vide
    }

    #[tokio::test]
    async fn ouvrir_un_port_serie_enregistre_une_session_sans_sftp_ni_commande() {
        let (_maitre, esclave) = pty();
        let app = app_de_test();
        let label = ouvrir(&app, 1, &esclave, 9600).await.unwrap();
        assert!(label.contains(&esclave), "{label}");
        let ouvertes = open_sessions(app.state::<SessionStore>());
        assert_eq!(ouvertes.len(), 1);
        assert_eq!(ouvertes[0].id, 1);
        let e = sftp_of(&app.state::<SessionStore>(), 1)
            .await
            .err()
            .unwrap();
        assert_eq!(e, "Pas de SFTP sur un port série.");
        let executer = app.state::<SessionStore>().inner.lock().unwrap()[&1]
            .executer
            .clone();
        let e = executer("x".into(), None).await.unwrap_err();
        assert!(e.contains("commande à distance"), "{e}");
        assert!(en_cours_vide(&app));
    }

    /// Les frappes atteignent l'équipement, et ce qu'il répond revient au
    /// terminal par le relais commun.
    #[tokio::test]
    async fn les_frappes_atteignent_le_port_et_la_sortie_revient_par_le_pump() {
        let (mut maitre, esclave) = pty();
        let app = app_de_test();
        let sorties = ecouter(&app, "pty-output");
        ouvrir(&app, 1, &esclave, 9600).await.unwrap();
        pty_write(app.state::<SessionStore>(), 1, "show version\r".into())
            .await
            .unwrap();
        let mut lecteur = std::fs::File::from(nix::unistd::dup(&maitre).unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut lu = Vec::new();
            let mut tampon = [0u8; 64];
            while !lu.ends_with(b"show version\r") {
                match lecteur.read(&mut tampon) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => lu.extend_from_slice(&tampon[..n]),
                }
            }
            let _ = tx.send(lu);
        });
        let lu = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("rien n'est arrivé au port");
        assert!(lu.ends_with(b"show version\r"), "{lu:?}");
        maitre.write_all(b"Cisco IOS\r\n").unwrap();
        let mut texte = String::new();
        for _ in 0..40 {
            texte = sorties
                .lock()
                .unwrap()
                .iter()
                .filter_map(|v| v["data"].as_str().map(str::to_owned))
                .collect();
            if texte.contains("Cisco IOS") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(texte.contains("Cisco IOS"), "{texte:?}");
        pty_close(app.handle().clone(), app.state::<SessionStore>(), 1)
            .await
            .unwrap();
    }

    /// Débrancher l'équipement (le maître fermé) ferme l'onglet.
    #[tokio::test]
    async fn debrancher_le_port_ferme_l_onglet() {
        let (maitre, esclave) = pty();
        let app = app_de_test();
        let fermes = ecouter(&app, "pty-closed");
        ouvrir(&app, 1, &esclave, 9600).await.unwrap();
        drop(maitre);
        for _ in 0..40 {
            if !app
                .state::<SessionStore>()
                .inner
                .lock()
                .unwrap()
                .contains_key(&1)
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(!app
            .state::<SessionStore>()
            .inner
            .lock()
            .unwrap()
            .contains_key(&1));
        assert_eq!(*fermes.lock().unwrap(), [serde_json::json!({ "id": 1 })]);
    }

    /// Le port est ouvert en exclusif (`TIOCEXCL`) : un second onglet est
    /// refusé, sans rien laisser dans le magasin. Sous root, `CAP_SYS_ADMIN`
    /// passe outre l'exclusivité : le test ne prouve rien et s'arrête.
    #[tokio::test]
    async fn un_port_deja_ouvert_est_refuse_et_ne_laisse_rien_dans_le_magasin() {
        if nix::unistd::geteuid().is_root() {
            return;
        }
        let (_maitre, esclave) = pty();
        let app = app_de_test();
        ouvrir(&app, 1, &esclave, 9600).await.unwrap();
        assert!(ouvrir(&app, 2, &esclave, 9600).await.is_err());
        assert!(en_cours_vide(&app));
        let ouvertes = open_sessions(app.state::<SessionStore>());
        assert_eq!(ouvertes.iter().map(|o| o.id).collect::<Vec<_>>(), [1]);
    }

    #[tokio::test]
    async fn une_vitesse_nulle_et_un_chemin_hors_de_dev_sont_refuses_proprement() {
        let (_maitre, esclave) = pty();
        let app = app_de_test();
        assert!(ouvrir(&app, 1, "/etc/passwd", 9600).await.is_err());
        assert!(en_cours_vide(&app));
        assert!(ouvrir(&app, 2, &esclave, 0).await.is_err());
        assert!(en_cours_vide(&app));
        assert!(open_sessions(app.state::<SessionStore>()).is_empty());
    }

    /// Onglet fermé pendant l'ouverture : la session n'est pas enregistrée.
    #[tokio::test]
    async fn fermer_l_onglet_pendant_l_ouverture_serie_n_enregistre_rien() {
        let (_maitre, esclave) = pty();
        let app = app_de_test();
        app.state::<SessionStore>()
            .en_cours
            .lock()
            .unwrap()
            .insert(3);
        pty_close(app.handle().clone(), app.state::<SessionStore>(), 3)
            .await
            .unwrap();
        let e = ouvrir(&app, 3, &esclave, 9600).await.unwrap_err();
        assert_eq!(e, CONNEXION_ANNULEE);
        assert!(app.state::<SessionStore>().inner.lock().unwrap().is_empty());
        assert!(en_cours_vide(&app));
    }
}
