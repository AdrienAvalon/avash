//! Tests des commandes : magasin de sessions, cibles, UTF-8, verrous, hôtes.

use super::*;
use avash::tunnel::TunnelKind;
use std::collections::HashMap;
use std::sync::Mutex;

fn target_with(password: Option<&str>) -> Target {
    Target {
        addr: "h".into(),
        port: 22,
        user: "u".into(),
        key_path: None,
        password: password.map(str::to_string),
        label: "h".into(),
        jumps: Vec::new(),
    }
}

#[test]
fn override_password_garde_le_mot_de_passe_du_trousseau_sans_saisie() {
    let mut t = target_with(Some("du-trousseau"));
    t.override_password(None);
    assert_eq!(t.password.as_deref(), Some("du-trousseau"));
    t.override_password(Some(String::new()));
    assert_eq!(
        t.password.as_deref(),
        Some("du-trousseau"),
        "saisie vide = pas de saisie"
    );
}

#[test]
fn override_password_prefere_la_saisie_quand_il_y_en_a_une() {
    let mut t = target_with(Some("ancien"));
    t.override_password(Some("nouveau".into()));
    assert_eq!(t.password.as_deref(), Some("nouveau"));
    let mut t = target_with(None);
    t.override_password(Some("saisi".into()));
    assert_eq!(t.password.as_deref(), Some("saisi"));
}

#[test]
fn effective_user_retombe_sur_l_utilisateur_courant() {
    // Cle du trousseau coherente entre save et load : un hote sans `User`
    // doit resoudre le meme utilisateur des deux cotes (regression :
    // « mémoriser » etait casse pour ces hotes).
    assert_eq!(effective_user(Some("deploy".into())), "deploy");
    assert_eq!(effective_user(Some("  deploy ".into())), "deploy");
    assert_eq!(effective_user(None), avash::ssh::current_username());
    assert_eq!(
        effective_user(Some(String::new())),
        avash::ssh::current_username()
    );
}

#[test]
fn remote_join_gere_racine_point_et_slash_final() {
    assert_eq!(remote_join("/srv", "a.txt"), "/srv/a.txt");
    assert_eq!(remote_join("/srv/", "a.txt"), "/srv/a.txt");
    assert_eq!(remote_join("/", "a.txt"), "/a.txt");
    assert_eq!(
        remote_join(".", "a.txt"),
        "a.txt",
        "cwd du login : chemin relatif"
    );
    assert_eq!(remote_join("", "a.txt"), "a.txt");
}

#[test]
fn parse_kind_reconnait_les_trois_types_et_refuse_le_reste() {
    assert_eq!(parse_kind("local").unwrap(), TunnelKind::Local);
    assert_eq!(parse_kind("remote").unwrap(), TunnelKind::Remote);
    assert_eq!(parse_kind("dynamic").unwrap(), TunnelKind::Dynamic);
    assert!(parse_kind("socks").is_err());
}

/// HOME est global au processus : deux tests qui le modifient en parallele
/// se marchent dessus. Ce verrou les serialise (les autres tests restent
/// paralleles). Sans lui, `find_host`_* echoue une fois sur deux.
static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Isole HOME pour ne pas dependre du ~/.ssh/config reel de la machine.
/// Le HOME precedent est restaure a la destruction du garde.
pub(crate) fn with_ssh_config(contents: &str) -> HomeGuard {
    let lock = HOME_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = std::env::temp_dir().join(format!(
        "avash-ui-test-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let ssh = dir.join(".ssh");
    std::fs::create_dir_all(&ssh).unwrap();
    std::fs::write(ssh.join("config"), contents).unwrap();
    // `HOME` ne suffit pas : sous Windows, `dirs::home_dir()` l'ignore et
    // consulte le profil du système — les tests lisaient alors le vrai
    // `~/.ssh/config` du poste (vide sur un exécuteur de CI) et échouaient.
    // `AVASH_HOME` est la dérogation que `repertoire_personnel()` honore sur
    // toutes les plateformes ; on pose les deux, comme `testutil::temp_home`
    // dans le cœur.
    let previous = std::env::var("HOME").ok();
    let previous_avash = std::env::var("AVASH_HOME").ok();
    std::env::set_var("HOME", &dir);
    std::env::set_var("AVASH_HOME", &dir);
    HomeGuard {
        previous,
        previous_avash,
        dir,
        _lock: lock,
    }
}

pub(crate) struct HomeGuard {
    previous: Option<String>,
    previous_avash: Option<String>,
    dir: std::path::PathBuf,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        match &self.previous_avash {
            Some(h) => std::env::set_var("AVASH_HOME", h),
            None => std::env::remove_var("AVASH_HOME"),
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

// ---------- local_target ----------

#[test]
fn local_target_respecte_le_chemin_impose() {
    let got = local_target("/srv/rapport.md", Some("/tmp/ailleurs.md".into())).unwrap();
    assert_eq!(got, "/tmp/ailleurs.md");
}

#[test]
fn local_target_derive_le_nom_du_fichier_distant() {
    let got = local_target("/srv/data/rapport.md", None).unwrap();
    assert!(
        got.ends_with("rapport.md"),
        "le nom distant doit etre conserve : {got}"
    );
}

#[test]
fn local_target_ne_garde_que_le_dernier_segment() {
    // Un remote contenant ../ ne doit pas remonter dans l'arborescence locale.
    let got = local_target("/srv/../../etc/passwd", None).unwrap();
    assert!(got.ends_with("passwd"), "{got}");
    assert!(!got.contains(".."), "traversee de chemin : {got}");
}

#[test]
fn local_target_refuse_un_chemin_sans_nom_de_fichier() {
    // Regression : file_name() renvoyait None, unwrap_or_default() donnait
    // une chaine vide et la destination devenait le dossier lui-meme.
    for remote in ["/", "..", "/srv/.."] {
        assert!(
            local_target(remote, None).is_err(),
            "{remote} devrait etre refuse"
        );
    }
}

// ---------- find_host / auth_for ----------

#[test]
fn find_host_trouve_un_alias_declare() {
    let _g = with_ssh_config("Host prod\n  HostName 10.0.0.1\n  User deploy\n  Port 2222\n");
    let h = find_host("prod").expect("alias prod doit etre trouve");
    assert_eq!(h.hostname.as_deref(), Some("10.0.0.1"));
    assert_eq!(h.user.as_deref(), Some("deploy"));
    assert_eq!(h.port, Some(2222));
}

#[test]
fn find_host_signale_un_alias_inconnu() {
    let _g = with_ssh_config("Host prod\n  HostName 10.0.0.1\n");
    let err = find_host("absent").unwrap_err();
    assert!(err.contains("absent"), "message peu clair : {err}");
}

#[test]
fn target_depuis_alias_reprend_user_port_et_cle() {
    let _g = with_ssh_config(
        "Host prod\n  HostName 10.0.0.1\n  User deploy\n  Port 2222\n  IdentityFile /tmp/k\n",
    );
    let t = Target::from_alias("prod").unwrap();
    assert_eq!(t.addr, "10.0.0.1");
    assert_eq!(t.user, "deploy");
    assert_eq!(t.port, 2222);
    assert_eq!(t.key_path.as_deref(), Some(std::path::Path::new("/tmp/k")));
    assert!(t.password.is_none(), "aucun mot de passe depuis un alias");
    assert_eq!(t.label, "prod");
}

// ---------- resolve_jumps ----------

/// Un maillon nu est un alias de `~/.ssh/config` : on reprend son adresse,
/// son port, son utilisateur et sa clé. Aucun test ne couvrait cette
/// résolution, par laquelle passe pourtant chaque connexion à travers un
/// bastion.
#[test]
fn un_rebond_par_alias_reprend_la_config_du_bastion() {
    let _g = with_ssh_config(
        "Host bastion\n  HostName 10.0.0.1\n  User rebond\n  Port 2222\n  IdentityFile /k/bastion\n\n\
         Host cible\n  HostName 10.0.0.2\n  ProxyJump bastion\n",
    );
    let t = Target::from_alias("cible").unwrap();
    assert_eq!(t.jumps.len(), 1);
    let h = &t.jumps[0];
    assert_eq!(h.addr, "10.0.0.1");
    assert_eq!(h.port, 2222);
    assert_eq!(h.auth.user, "rebond");
    assert_eq!(
        h.auth.key_path.as_deref(),
        Some(std::path::Path::new("/k/bastion"))
    );
    assert!(
        h.auth.password.is_none(),
        "un rebond n'a pas de mot de passe"
    );
}

/// `user@hote:port` n'est pas cherché comme alias : la saisie fait foi, et
/// faute de clé propre le rebond réutilise celle de la cible.
#[test]
fn un_rebond_explicite_reutilise_la_cle_de_la_cible() {
    let _g = with_ssh_config(
        "Host cible\n  HostName 10.0.0.2\n  IdentityFile /k/cible\n  ProxyJump deploy@1.2.3.4:2200\n",
    );
    let t = Target::from_alias("cible").unwrap();
    assert_eq!(t.jumps.len(), 1);
    let h = &t.jumps[0];
    assert_eq!(
        (h.addr.as_str(), h.port, h.auth.user.as_str()),
        ("1.2.3.4", 2200, "deploy")
    );
    assert_eq!(
        h.auth.key_path.as_deref(),
        Some(std::path::Path::new("/k/cible"))
    );
}

/// Une chaîne `a,b` donne deux rebonds dans l'ordre ; `none` et l'absence
/// de directive n'en donnent aucun.
#[test]
fn une_chaine_de_rebonds_garde_l_ordre_et_none_n_en_donne_aucun() {
    let _g = with_ssh_config(
        "Host a\n  HostName 10.0.0.10\n\nHost b\n  HostName 10.0.0.11\n\n\
         Host cible\n  HostName 10.0.0.2\n  ProxyJump a, b\n\nHost direct\n  HostName 10.0.0.3\n  ProxyJump none\n",
    );
    let t = Target::from_alias("cible").unwrap();
    let adresses: Vec<&str> = t.jumps.iter().map(|h| h.addr.as_str()).collect();
    assert_eq!(adresses, vec!["10.0.0.10", "10.0.0.11"]);
    assert!(Target::from_alias("direct").unwrap().jumps.is_empty());
}

#[test]
fn target_depuis_alias_retombe_sur_les_defauts() {
    let _g = with_ssh_config("Host simple\n  HostName 10.0.0.9\n");
    let t = Target::from_alias("simple").unwrap();
    assert_eq!(t.port, 22, "port par defaut");
    assert_eq!(
        t.user,
        avash::ssh::current_username(),
        "utilisateur courant par defaut"
    );
    assert!(t.key_path.is_none());
}
// ---------- Utf8Stream ----------

#[test]
fn utf8_recolle_un_caractere_coupe_en_deux() {
    // "é" = 0xC3 0xA9 : on coupe entre les deux octets.
    let mut d = Utf8Stream::default();
    assert_eq!(d.push(&[0xC3]), "", "un octet seul n'est pas decodable");
    assert_eq!(d.push(&[0xA9]), "é", "le caractere doit etre recolle");
}

#[test]
fn utf8_gere_une_coupure_au_milieu_d_un_emoji() {
    // 😈 = 4 octets, coupe apres le premier.
    let full = "😈".as_bytes().to_vec();
    let mut d = Utf8Stream::default();
    assert_eq!(d.push(&full[..1]), "");
    assert_eq!(d.push(&full[1..]), "😈");
}

#[test]
fn utf8_texte_coupe_a_chaque_octet_est_restitue_intact() {
    let source = "Déjà vu — 100 % réussi 😈 ✓";
    let mut d = Utf8Stream::default();
    let mut out = String::new();
    for b in source.as_bytes() {
        out.push_str(&d.push(&[*b]));
    }
    assert_eq!(out, source, "le flux doit etre restitue a l'identique");
}

#[test]
fn utf8_ne_bloque_pas_sur_un_octet_invalide() {
    // Un octet illegal ne doit pas figer le terminal : on le saute.
    let mut d = Utf8Stream::default();
    let out = d.push(&[b'a', 0xFF, b'b']);
    assert!(out.starts_with('a'), "{out:?}");
    let suite = d.push(b"c");
    assert!(
        format!("{out}{suite}").contains('c'),
        "le flux doit repartir apres l'octet invalide"
    );
}

#[test]
fn utf8_ascii_passe_sans_latence() {
    let mut d = Utf8Stream::default();
    assert_eq!(d.push(b"ls -la\r\n"), "ls -la\r\n");
}
// ---------- Target::manual ----------

#[test]
fn manual_accepte_adresse_user_et_mot_de_passe() {
    let t = Target::manual(
        "10.0.0.5".into(),
        Some(2222),
        "adrien".into(),
        Some("secret".into()),
        None,
    )
    .unwrap();
    assert_eq!(t.addr, "10.0.0.5");
    assert_eq!(t.port, 2222);
    assert_eq!(t.user, "adrien");
    assert_eq!(t.password.as_deref(), Some("secret"));
    assert_eq!(t.label, "adrien@10.0.0.5", "libelle affiche dans l'onglet");
}

#[test]
fn manual_utilise_22_par_defaut() {
    let t = Target::manual("srv".into(), None, "u".into(), Some("p".into()), None).unwrap();
    assert_eq!(t.port, 22);
}

#[test]
fn manual_rogne_les_espaces_de_saisie() {
    // Un copier-coller traine souvent une espace : elle casserait la
    // resolution DNS avec un message incomprehensible.
    let t = Target::manual(
        "  10.0.0.5  ".into(),
        None,
        " adrien ".into(),
        Some("p".into()),
        None,
    )
    .unwrap();
    assert_eq!(t.addr, "10.0.0.5");
    assert_eq!(t.user, "adrien");
}

#[test]
fn manual_refuse_une_adresse_vide() {
    let e = Target::manual("   ".into(), None, "u".into(), Some("p".into()), None).unwrap_err();
    assert!(e.contains("adresse"), "{e}");
}

#[test]
fn manual_refuse_un_utilisateur_vide() {
    let e = Target::manual("srv".into(), None, String::new(), Some("p".into()), None).unwrap_err();
    assert!(e.contains("utilisateur"), "{e}");
}

#[test]
fn manual_exige_un_mot_de_passe_ou_une_cle() {
    // Sans l'un des deux, l'authentification echouerait cote serveur avec
    // un message opaque : autant le dire avant de tenter la connexion.
    let e = Target::manual("srv".into(), None, "u".into(), None, None).unwrap_err();
    assert!(e.contains("mot de passe") && e.contains("clé"), "{e}");
    // Une chaine vide vaut absence.
    let e = Target::manual(
        "srv".into(),
        None,
        "u".into(),
        Some(String::new()),
        Some(String::new()),
    )
    .unwrap_err();
    assert!(e.contains("mot de passe"), "{e}");
}

#[test]
fn manual_signale_une_cle_introuvable() {
    let e = Target::manual(
        "srv".into(),
        None,
        "u".into(),
        None,
        Some("/chemin/qui/n/existe/pas".into()),
    )
    .unwrap_err();
    assert!(e.contains("introuvable"), "{e}");
    assert!(
        e.contains("/chemin/qui/n/existe/pas"),
        "le chemin fautif doit etre nomme : {e}"
    );
}

#[test]
fn manual_accepte_une_cle_existante_sans_mot_de_passe() {
    let key = std::env::temp_dir().join(format!("avash-key-{}", std::process::id()));
    std::fs::write(&key, b"factice").unwrap();
    let t = Target::manual(
        "srv".into(),
        None,
        "u".into(),
        None,
        Some(key.to_string_lossy().into_owned()),
    )
    .unwrap();
    assert_eq!(t.key_path.as_deref(), Some(key.as_path()));
    assert!(t.password.is_none());
    let _ = std::fs::remove_file(&key);
}
#[test]
fn debug_ne_divulgue_jamais_le_mot_de_passe() {
    let t = Target::manual(
        "srv".into(),
        None,
        "u".into(),
        Some("tres-secret".into()),
        None,
    )
    .unwrap();
    let rendu = format!("{t:?}");
    assert!(
        !rendu.contains("tres-secret"),
        "le mot de passe ne doit jamais apparaitre dans une trace : {rendu}"
    );
    assert!(rendu.contains("masqué"), "{rendu}");
}

// ---------- Magasin de sessions : annulation pendant la connexion ----------
//
// Ces chemins ne vivaient que dans des commentaires et dans la suite bout en
// bout. Le moteur factice de Tauri permet de construire l'état sans fenêtre.

use tauri::Manager as _;

fn app_de_test() -> tauri::App<tauri::test::MockRuntime> {
    tauri::test::mock_builder()
        .manage(SessionStore {
            inner: Mutex::new(HashMap::new()),
            annules: Mutex::new(std::collections::HashSet::new()),
            en_cours: Mutex::new(std::collections::HashSet::new()),
        })
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("application factice")
}

fn poignee(epoch: u64) -> SessionHandle {
    let (input, _) = tokio::sync::mpsc::channel(1);
    let (resize, _) = tokio::sync::mpsc::channel(1);
    SessionHandle {
        epoch,
        input,
        resize,
        sftp: Mutex::new(None),
        ouvrir_sftp: std::sync::Arc::new(|| {
            Box::pin(async { Err("pas de transport dans ce test".to_owned()) })
        }),
        label: "h".into(),
        enregistreur: std::sync::Arc::new(Mutex::new(None)),
    }
}

/// Démarrer, écrire par le chemin du pump, arrêter : le fichier existe,
/// se relit, et un second démarrage pendant l'enregistrement rend le même
/// chemin au lieu d'en ouvrir un autre.
#[tokio::test]
async fn un_enregistrement_se_demarre_recoit_la_sortie_et_s_arrete() {
    let _g = with_ssh_config("");
    let app = app_de_test();
    let state = app.state::<SessionStore>();
    enregistrer_session(&state, 7, poignee(1)).unwrap();
    assert!(enregistrement_en_cours(app.state::<SessionStore>(), 7).is_none());
    let chemin = enregistrement_demarrer(
        app.state::<SessionStore>(),
        7,
        80,
        24,
        Some("\x1b[2J$ ecran-initial".into()),
    )
    .unwrap();
    assert_eq!(
        std::path::Path::new(&chemin)
            .extension()
            .and_then(|e| e.to_str()),
        Some("cast"),
        "{chemin}"
    );
    assert_eq!(
        enregistrement_demarrer(app.state::<SessionStore>(), 7, 80, 24, None).unwrap(),
        chemin
    );
    assert_eq!(
        enregistrement_en_cours(app.state::<SessionStore>(), 7).as_deref(),
        Some(chemin.as_str())
    );
    // Ce que ferait le pump.
    {
        let e = state.inner.lock().unwrap()[&7].enregistreur.clone();
        e.lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .sortie("bonjour\r\n")
            .unwrap();
    }
    pty_resize(app.state::<SessionStore>(), 7, 100, 30)
        .await
        .unwrap();
    let fin = enregistrement_arreter(app.state::<SessionStore>(), 7).unwrap();
    assert_eq!(fin.as_deref(), Some(chemin.as_str()));
    assert!(enregistrement_arreter(app.state::<SessionStore>(), 7)
        .unwrap()
        .is_none());
    let contenu = std::fs::read_to_string(&chemin).unwrap();
    let (entete, ev) = avash::enregistrement::relire(&contenu).unwrap();
    assert_eq!(entete["title"], "h");
    let kinds: Vec<&str> = ev.iter().map(|(_, k, _)| k.as_str()).collect();
    assert_eq!(
        kinds,
        vec!["o", "o", "r"],
        "l'écran initial vient en premier"
    );
    assert_eq!(ev[0].2, "\x1b[2J$ ecran-initial");
    assert_eq!(ev[1].2, "bonjour\r\n");
    assert!(enregistrement_demarrer(app.state::<SessionStore>(), 99, 80, 24, None).is_err());
    let liste = enregistrements_lister();
    assert!(
        liste
            .iter()
            .any(|i| i.chemin.display().to_string() == chemin),
        "{liste:?}"
    );
}

/// Le panneau SFTP dépend de la session de l'onglet : si le canal ne peut
/// pas s'ouvrir, l'erreur remonte telle quelle et rien n'est mémorisé —
/// le prochain essai repart de zéro, plutôt que de rendre un canal mort.
#[tokio::test]
async fn un_canal_sftp_qui_ne_s_ouvre_pas_ne_laisse_rien_dans_le_magasin() {
    let app = app_de_test();
    let state = app.state::<SessionStore>();
    enregistrer_session(&state, 5, poignee(1)).unwrap();
    let e = sftp_of(&app.state::<SessionStore>(), 5)
        .await
        .err()
        .unwrap();
    assert_eq!(e, "pas de transport dans ce test");
    let vide = state.inner.lock().unwrap()[&5]
        .sftp
        .lock()
        .unwrap()
        .is_none();
    assert!(vide, "aucun canal ne doit être mémorisé");
    let e = sftp_of(&app.state::<SessionStore>(), 6)
        .await
        .err()
        .unwrap();
    assert!(e.contains("inconnue"), "{e}");
}

/// Onglet fermé PENDANT la connexion : l'enregistrement qui suit doit
/// échouer en le disant, et ne rien laisser dans le magasin — sinon une
/// session SSH établie survivait sans onglet, listée comme cible de snippet.
#[tokio::test]
async fn fermer_pendant_la_connexion_annule_l_enregistrement() {
    let app = app_de_test();
    let state = app.state::<SessionStore>();
    state.en_cours.lock().unwrap().insert(1);
    pty_close(app.state::<SessionStore>(), 1).await.unwrap();
    let issue = enregistrer_session(&state, 1, poignee(1));
    assert_eq!(issue.unwrap_err(), CONNEXION_ANNULEE);
    assert!(
        state.inner.lock().unwrap().is_empty(),
        "rien ne doit rester"
    );
    assert!(
        state.annules.lock().unwrap().is_empty(),
        "l'annulation est consommée"
    );
    assert!(state.en_cours.lock().unwrap().is_empty());
}

/// Fermer un onglet dont la connexion avait déjà échoué ne doit PAS semer
/// une annulation : l'identifiant est réattribué après un rechargement de
/// fenêtre, et la session suivante se voyait répondre « annulée », figée.
#[tokio::test]
async fn fermer_sans_connexion_en_vol_ne_seme_pas_d_annulation() {
    let app = app_de_test();
    let state = app.state::<SessionStore>();
    pty_close(app.state::<SessionStore>(), 2).await.unwrap();
    assert!(state.annules.lock().unwrap().is_empty());
    assert!(enregistrer_session(&state, 2, poignee(1)).is_ok());
    assert!(state.inner.lock().unwrap().contains_key(&2));
}

/// Le front renumérote ses onglets à chaque rechargement : une session plus
/// récente sous le même identifiant évince l'ancienne, et la fin de
/// l'ancienne ne doit pas fermer la nouvelle.
#[tokio::test]
async fn une_session_plus_recente_evince_l_ancienne_sans_etre_close_par_elle() {
    let app = app_de_test();
    let state = app.state::<SessionStore>();
    enregistrer_session(&state, 3, poignee(1)).unwrap();
    enregistrer_session(&state, 3, poignee(2)).unwrap();
    assert!(is_superseded(app.handle(), 3, 1), "l'époque 1 est évincée");
    assert!(!is_superseded(app.handle(), 3, 2));
    // La fin du pump de l'ancienne session ne touche pas à la nouvelle.
    clore_session(app.handle(), 3, 1);
    assert_eq!(
        state.inner.lock().unwrap().get(&3).map(|h| h.epoch),
        Some(2)
    );
    // Celle de la session courante, si.
    clore_session(app.handle(), 3, 2);
    assert!(state.inner.lock().unwrap().get(&3).is_none());
}

/// Une frappe vers une session fermée doit être une erreur, pas un silence :
/// le front croyait sinon l'avoir transmise.
#[tokio::test]
async fn ecrire_dans_une_session_inconnue_est_une_erreur() {
    let app = app_de_test();
    let e = pty_write(app.state::<SessionStore>(), 9, "ls".into())
        .await
        .unwrap_err();
    assert!(e.contains("inconnue"), "{e}");
    assert!(pty_resize(app.state::<SessionStore>(), 9, 80, 24)
        .await
        .is_err());
}

#[tokio::test]
async fn open_sessions_liste_les_sessions_enregistrees() {
    let app = app_de_test();
    let state = app.state::<SessionStore>();
    assert!(open_sessions(app.state::<SessionStore>()).is_empty());
    enregistrer_session(&state, 4, poignee(1)).unwrap();
    let liste = open_sessions(app.state::<SessionStore>());
    assert_eq!(liste.len(), 1);
    assert_eq!((liste[0].id, liste[0].label.as_str()), (4, "h"));
}

/// Plancher de débit du décodeur UTF-8 en flux : il traverse chaque octet
/// de sortie du terminal. La mesure (`benches/utf8.rs`) donne des centaines
/// de Mo/s en release ; le plancher est posé dix fois sous ce qu'on observe
/// en profil de test, pour ne pas rougir sous charge, mais une régression
/// algorithmique — un recollage quadratique, un tampon recopié à chaque
/// bloc — le franchirait de loin.
#[test]
fn le_decodeur_utf8_garde_un_debit_plancher() {
    let ligne = "\x1b[32mavalon\x1b[m@\x1b[36mcachyos\x1b[m ~ » déjà vu — 100 % ✓\r\n";
    let source: Vec<u8> = ligne.repeat(5_000).into_bytes();
    let mut d = Utf8Stream::default();
    let depart = std::time::Instant::now();
    let mut sortie = 0usize;
    for bloc in source.chunks(64) {
        sortie += d.push(bloc).len();
    }
    let secondes = depart.elapsed().as_secs_f64();
    let debit = source.len() as f64 / secondes / 1e6;
    assert!(sortie > 0);
    assert!(
        debit > 2.0,
        "décodeur UTF-8 à {debit:.1} Mo/s sur des blocs de 64 octets : régression"
    );
}

#[test]
fn open_external_refuse_les_schemas_dangereux() {
    // Un lien du terminal ne doit jamais ouvrir file://, javascript:, etc.
    for mauvais in [
        "file:///etc/passwd",
        "javascript:alert(1)",
        "data:text/html,<script>",
        "vbscript:x",
        "  file:///home",
    ] {
        assert!(
            open_external(mauvais.into()).is_err(),
            "devrait refuser : {mauvais}"
        );
    }
}
