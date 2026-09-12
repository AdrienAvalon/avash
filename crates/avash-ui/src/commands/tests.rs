//! Tests des commandes : magasin de sessions, cibles, UTF-8, verrous, hôtes.

use super::*;
use avash::tunnel::TunnelKind;
use std::sync::Mutex;

fn target_with(password: Option<&str>) -> Target {
    Target {
        addr: "h".into(),
        port: 22,
        user: "u".into(),
        key_path: None,
        password: password.map(|p| avash::secrets::Zeroizing::new(p.to_owned())),
        label: "h".into(),
        jumps: Vec::new(),
    }
}

#[test]
fn override_password_garde_le_mot_de_passe_du_trousseau_sans_saisie() {
    let mut t = target_with(Some("du-trousseau"));
    t.override_password(None);
    assert_eq!(
        t.password.as_deref().map(String::as_str),
        Some("du-trousseau")
    );
    t.override_password(Some(String::new()));
    assert_eq!(
        t.password.as_deref().map(String::as_str),
        Some("du-trousseau"),
        "saisie vide = pas de saisie"
    );
}

#[test]
fn override_password_prefere_la_saisie_quand_il_y_en_a_une() {
    let mut t = target_with(Some("ancien"));
    t.override_password(Some("nouveau".into()));
    assert_eq!(t.password.as_deref().map(String::as_str), Some("nouveau"));
    let mut t = target_with(None);
    t.override_password(Some("saisi".into()));
    assert_eq!(t.password.as_deref().map(String::as_str), Some("saisi"));
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
    // Isole aussi le trousseau : `Target::from_alias` appelle `secrets::load`
    // et le diagnostic `secrets::sonder`, qui sans cette dérogation tapent dans
    // le vrai Secret Service du poste (keyring v1 sous Linux = un aller-retour
    // D-Bus). `with_ssh_config` isolait `~/.ssh`, pas le trousseau ; une entrée
    // réelle `deploy@10.0.0.1:2222` faisait alors échouer les `is_none()`.
    // Trouvé par l'audit du 7 septembre 2026. Voir `avash::secrets::en_memoire`.
    let previous_trousseau = std::env::var("AVASH_TROUSSEAU").ok();
    poser_variable("HOME", Some(dir.as_os_str()));
    poser_variable("AVASH_HOME", Some(dir.as_os_str()));
    poser_variable("AVASH_TROUSSEAU", Some("memoire".as_ref()));
    HomeGuard {
        previous,
        previous_avash,
        previous_trousseau,
        dir,
        _lock: lock,
    }
}

pub(crate) struct HomeGuard {
    previous: Option<String>,
    previous_avash: Option<String>,
    previous_trousseau: Option<String>,
    pub(crate) dir: std::path::PathBuf,
    _lock: std::sync::MutexGuard<'static, ()>,
}

/// Pose (`Some`) ou retire (`None`) une variable d'environnement du
/// processus de test. Un seul bloc `unsafe` pour les deux gardes de ce
/// fichier (audit du 12 septembre 2026, C-unsafe-6 : les appels étaient nus,
/// à moitié migrés vers l'édition 2024).
fn poser_variable(nom: &str, valeur: Option<&std::ffi::OsStr>) {
    match valeur {
        // SAFETY: appelé sous HOME_LOCK (tenu par `HomeGuard`), qui sérialise
        // tous les tests de ce binaire qui écrivent l'environnement ; les
        // autres fils ne le lisent que par std::env (verrou interne de la
        // std), et aucun code C de ce binaire de test n'y lit (moteur factice
        // pur Rust, trousseau simulé).
        Some(v) => unsafe { std::env::set_var(nom, v) },
        // SAFETY: même garantie que le bras précédent : sous HOME_LOCK,
        // lecteurs Rust seulement.
        None => unsafe { std::env::remove_var(nom) },
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        poser_variable("HOME", self.previous.as_deref().map(std::ffi::OsStr::new));
        poser_variable(
            "AVASH_HOME",
            self.previous_avash.as_deref().map(std::ffi::OsStr::new),
        );
        poser_variable(
            "AVASH_TROUSSEAU",
            self.previous_trousseau.as_deref().map(std::ffi::OsStr::new),
        );
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Comme `with_ssh_config`, mais le trousseau simulé est EN PANNE : tout accès
/// rend une erreur (contrat K1 de l'audit du 12 septembre 2026).
pub(crate) fn with_trousseau_en_panne(contents: &str) -> HomeGuard {
    let g = with_ssh_config(contents);
    poser_variable("AVASH_TROUSSEAU", Some("panne".as_ref()));
    g
}

/// Les charges d'un événement émis par l'application factice, dans l'ordre.
pub(crate) fn ecouter(
    app: &tauri::App<tauri::test::MockRuntime>,
    evenement: &str,
) -> std::sync::Arc<Mutex<Vec<serde_json::Value>>> {
    use tauri::Listener as _;
    let recus = std::sync::Arc::new(Mutex::new(Vec::new()));
    let r = recus.clone();
    app.listen_any(evenement, move |e| {
        r.lock()
            .unwrap()
            .push(serde_json::from_str(e.payload()).unwrap());
    });
    recus
}

// ---------- local_target ----------

#[test]
fn local_target_derive_le_nom_du_fichier_distant() {
    // `with_ssh_config` pose `AVASH_HOME` sur un dossier vierge : la cible par
    // défaut n'existe pas, on garde donc le nom tel quel (déterministe, sans
    // dépendre du vrai dossier Téléchargements du poste).
    let _g = with_ssh_config("");
    let got = local_target("/srv/data/rapport.md").unwrap();
    assert!(
        got.ends_with("rapport.md"),
        "le nom distant doit etre conserve : {got}"
    );
}

#[test]
fn local_target_ne_garde_que_le_dernier_segment() {
    // Un remote contenant ../ ne doit pas remonter dans l'arborescence locale.
    let _g = with_ssh_config("");
    let got = local_target("/srv/../../etc/passwd").unwrap();
    assert!(got.ends_with("passwd"), "{got}");
    assert!(!got.contains(".."), "traversee de chemin : {got}");
}

/// Trouvé par l'audit du 7 septembre 2026 : un fichier déjà présent au chemin
/// par défaut était écrasé sans un mot. `local_target` doit rendre un chemin
/// libre, différent, et laisser l'existant intact.
#[test]
fn local_target_ne_prend_pas_un_nom_deja_pris() {
    let _g = with_ssh_config("");
    let dir = avash::sftp::default_local_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let pris = dir.join("backup.sql");
    std::fs::write(&pris, b"ancien").unwrap();

    let got = local_target("/srv/backup.sql").unwrap();
    let got = std::path::Path::new(&got);
    assert_ne!(got, pris, "la cible ne doit pas viser le fichier existant");
    assert!(
        !got.exists(),
        "le nom choisi doit etre libre : {}",
        got.display()
    );
    assert!(
        got.ends_with("backup (2).sql"),
        "nom libre attendu : {}",
        got.display()
    );
    assert_eq!(
        std::fs::read(&pris).unwrap(),
        b"ancien",
        "le fichier existant a ete touche"
    );
}

/// Le nom libre se décide aussi sur un dossier : télécharger un dossier distant
/// sur un dossier local homonyme ne doit pas les fusionner (racine du transfert).
#[test]
fn local_target_evite_aussi_un_dossier_homonyme() {
    let _g = with_ssh_config("");
    let dir = avash::sftp::default_local_dir();
    std::fs::create_dir_all(dir.join("logs")).unwrap();

    let got = local_target("/srv/logs").unwrap();
    let got = std::path::Path::new(&got);
    assert!(
        !got.exists(),
        "le nom choisi doit etre libre : {}",
        got.display()
    );
    assert!(
        got.ends_with("logs (2)"),
        "nom libre attendu : {}",
        got.display()
    );
}

/// Trouvé par l'audit du 9 septembre 2026 : le `local` de `sftp_upload` est le
/// fichier LOCAL lu puis envoyé au serveur distant, et il partait sans aucune
/// garde, seul de tout ce fichier de commandes. Il doit au moins être absolu et
/// désigner quelque chose qui existe, comme le dossier partagé RDP
/// (`dossier_partage`) et le chemin de diagnostic (`diagnostic_exporter`) :
/// un chemin relatif viserait le répertoire courant de l'application, invisible
/// pour l'utilisateur, et une cible absente doit donner une erreur claire
/// plutôt qu'un échec de lecture au milieu du transfert.
#[test]
fn local_source_refuse_un_chemin_relatif_ou_absent() {
    let _g = with_ssh_config("");
    let dir = avash::sftp::default_local_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let present = dir.join("rapport.md");
    std::fs::write(&present, b"contenu").unwrap();

    assert!(
        local_source("rapport.md").is_err(),
        "un chemin relatif devrait être refusé"
    );
    assert!(
        local_source("").is_err(),
        "un chemin vide devrait être refusé"
    );
    assert!(
        local_source(&dir.join("absent.md").to_string_lossy()).is_err(),
        "un fichier absent devrait être refusé"
    );
    assert_eq!(
        local_source(&present.to_string_lossy()).unwrap(),
        present,
        "un fichier absolu et existant reste accepte tel quel"
    );
}

#[test]
fn local_target_refuse_un_chemin_sans_nom_de_fichier() {
    // Regression : file_name() renvoyait None, unwrap_or_default() donnait
    // une chaine vide et la destination devenait le dossier lui-meme.
    for remote in ["/", "..", "/srv/.."] {
        assert!(
            local_target(remote).is_err(),
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

#[test]
fn from_alias_applique_les_defauts_de_host_etoile() {
    // Trouvé par l'audit du 7 septembre 2026 : `Host *` posant User + IdentityFile
    // s'applique à `prod` (qui n'a ni l'un ni l'autre), comme `ssh prod`. Avant,
    // Avash résolvait `prod` avec l'utilisateur courant et sans clé, puis
    // demandait un mot de passe à tort.
    let _g = with_ssh_config(
        "Host *\n  User adrien\n  IdentityFile /tmp/k\n\nHost prod\n  HostName 10.0.0.1\n",
    );
    let t = Target::from_alias("prod").unwrap();
    assert_eq!(t.addr, "10.0.0.1");
    assert_eq!(t.user, "adrien");
    assert_eq!(t.key_path.as_deref(), Some(std::path::Path::new("/tmp/k")));
}

#[test]
fn from_alias_retient_la_premiere_valeur_du_fichier() {
    // `Host *` en tête pose `User root` avant le bloc littéral `Host prod`
    // (`User adrien`) : OpenSSH retient la première valeur obtenue, root gagne.
    let _g =
        with_ssh_config("Host *\n  User root\n\nHost prod\n  HostName 10.0.0.1\n  User adrien\n");
    assert_eq!(Target::from_alias("prod").unwrap().user, "root");
}

#[test]
fn un_rebond_par_alias_herite_du_host_etoile() {
    // Variante de `un_rebond_par_alias_reprend_la_config_du_bastion` : le bastion
    // n'a pas de `User` propre, il l'hérite de `Host *`. Le rebond doit prendre
    // cet utilisateur, pas l'utilisateur courant.
    let _g = with_ssh_config(
        "Host *\n  User global\n\nHost bastion\n  HostName 10.0.0.1\n\n\
         Host cible\n  HostName 10.0.0.2\n  ProxyJump bastion\n",
    );
    let t = Target::from_alias("cible").unwrap();
    assert_eq!(t.jumps.len(), 1);
    assert_eq!(t.jumps[0].auth.user, "global");
    assert_eq!(t.jumps[0].addr, "10.0.0.1");
}

#[test]
fn une_cle_en_tilde_est_resolue_dans_le_repertoire_personnel() {
    // Trouvé par l'audit du 7 septembre 2026 : `IdentityFile ~/.ssh/k` restait
    // littéral, la clé était introuvable et l'hôte inconnectable. Le tilde doit
    // être développé à la résolution, dans le répertoire personnel (ici le bac
    // à sable via AVASH_HOME).
    // `with_ssh_config` prend un verrou global non réentrant : chaque garde
    // doit être relâché avant d'en reprendre un (sinon interblocage). D'où les
    // deux blocs distincts.
    {
        let g = with_ssh_config("Host prod\n  HostName 10.0.0.1\n  IdentityFile ~/.ssh/k\n");
        let attendu = g.dir.join(".ssh").join("k");
        let t = Target::from_alias("prod").unwrap();
        assert_eq!(t.key_path.as_deref(), Some(attendu.as_path()));
    }
    // Un rebond qui reprend une clé en `~/` du bastion doit aussi la développer.
    {
        let g2 = with_ssh_config(
            "Host bastion\n  HostName 10.0.0.9\n  IdentityFile ~/.ssh/b\n\
             Host cible\n  HostName 10.0.0.2\n  ProxyJump bastion\n",
        );
        let attendu2 = g2.dir.join(".ssh").join("b");
        let t2 = Target::from_alias("cible").unwrap();
        assert_eq!(t2.jumps.len(), 1);
        assert_eq!(
            t2.jumps[0].auth.key_path.as_deref(),
            Some(attendu2.as_path())
        );
    }
}

// ---------- identifiant_encore_utilise ----------

#[test]
fn deux_alias_vers_le_meme_serveur_partagent_le_secret_a_ne_pas_oublier() {
    // Trouvé par l'audit du 7 septembre 2026 : `Host web` et `Host web-via-bastion`
    // (même HostName/User, un `ProxyJump` en plus) partagent l'entrée du
    // trousseau `deploy@web.exemple.com:22`. Supprimer l'un ne doit pas oublier
    // le mot de passe tant que l'autre le réclame.
    let _g = with_ssh_config(
        "Host web\n  HostName web.exemple.com\n  User deploy\n\
         Host web-via-bastion\n  HostName web.exemple.com\n  User deploy\n  ProxyJump bastion\n",
    );
    let hotes = avash::parse_ssh_config().unwrap();
    let id = avash::secrets::account_id("deploy", "web.exemple.com", 22);
    assert!(
        identifiant_encore_utilise(
            &avash::configuration_resolue().unwrap(),
            &hotes,
            "web-via-bastion",
            &id
        ),
        "web réclame encore l'entrée : ne pas l'oublier"
    );
}

#[test]
fn un_seul_alias_vers_le_serveur_laisse_oublier_le_secret() {
    // Le cas inverse : plus aucun autre alias, on doit bien oublier.
    let _g = with_ssh_config("Host web\n  HostName web.exemple.com\n  User deploy\n");
    let hotes = avash::parse_ssh_config().unwrap();
    let id = avash::secrets::account_id("deploy", "web.exemple.com", 22);
    assert!(
        !identifiant_encore_utilise(&avash::configuration_resolue().unwrap(), &hotes, "web", &id),
        "aucun autre alias : l'entrée est orpheline, on l'oublie"
    );
}

#[test]
fn un_port_different_ne_partage_pas_l_entree() {
    // Deux alias vers le même hôte mais un port distinct : identifiants
    // différents, changer le port de l'un ne touche pas l'entrée de l'autre.
    let _g = with_ssh_config(
        "Host web\n  HostName web.exemple.com\n  User deploy\n\
         Host web-alt\n  HostName web.exemple.com\n  User deploy\n  Port 2222\n",
    );
    let hotes = avash::parse_ssh_config().unwrap();
    let id = avash::secrets::account_id("deploy", "web.exemple.com", 22);
    assert!(
        !identifiant_encore_utilise(&avash::configuration_resolue().unwrap(), &hotes, "web", &id),
        "web-alt est sur le port 2222 : il ne partage pas deploy@…:22"
    );
}

#[test]
fn un_alias_venu_d_un_include_compte_comme_utilisateur_de_l_entree() {
    // L'alias partagé peut venir d'un fichier `Include` : la résolution parcourt
    // la config aplatie, comme `parse_ssh_config`. Sinon on oublierait un secret
    // encore réclamé par un hôte déclaré ailleurs (cf. 664d45e).
    let g = with_ssh_config(
        "Include config.d/*\nHost web\n  HostName web.exemple.com\n  User deploy\n",
    );
    let confd = g.dir.join(".ssh").join("config.d");
    std::fs::create_dir_all(&confd).unwrap();
    std::fs::write(
        confd.join("bastion.conf"),
        "Host web-via-bastion\n  HostName web.exemple.com\n  User deploy\n  ProxyJump bastion\n",
    )
    .unwrap();
    let hotes = avash::parse_ssh_config().unwrap();
    let id = avash::secrets::account_id("deploy", "web.exemple.com", 22);
    assert!(
        identifiant_encore_utilise(&avash::configuration_resolue().unwrap(), &hotes, "web", &id),
        "web-via-bastion vient d'un Include mais réclame encore l'entrée"
    );
}

#[test]
fn un_alias_sans_user_partage_l_entree_utilisateur_courant() {
    // Piège de résolution : un alias sans `User` ni `Port` retombe sur
    // l'utilisateur courant et le port 22, exactement comme `Target::from_alias`.
    // Il doit compter comme utilisateur de l'entrée `courant@hote:22`, sinon on
    // retombe sur le décalage save/relit de secrets.rs.
    let courant = avash::ssh::current_username();
    let _g = with_ssh_config(&format!(
        "Host implicite\n  HostName srv\n\
         Host explicite\n  HostName srv\n  User {courant}\n"
    ));
    let hotes = avash::parse_ssh_config().unwrap();
    let id = avash::secrets::account_id(&courant, "srv", 22);
    assert!(
        identifiant_encore_utilise(
            &avash::configuration_resolue().unwrap(),
            &hotes,
            "explicite",
            &id
        ),
        "l'alias sans User résout courant@srv:22 et partage l'entrée"
    );
}

// ---------- plan_deplacement ----------

#[test]
fn repointer_un_hote_n_ecrase_pas_le_secret_de_la_cible() {
    // Trouvé par l'audit du 7 septembre 2026 (scénario 2 du constat) : repointer
    // `web` vers 10.0.0.2 où `db` avait déjà mémorisé son mot de passe écrasait
    // celui de `db`, qui ne se connectait plus. Quand la cible est occupée, on
    // ne copie pas (le secret existant lui appartient), donc on n'oublie pas non
    // plus l'ancien.
    assert_eq!(plan_deplacement(false, true), (false, false));
    // Même l'ancien non partagé ne change rien tant que la cible porte un secret.
    assert_eq!(plan_deplacement(true, true), (false, false));
}

#[test]
fn modifier_un_alias_ne_perd_pas_le_secret_d_un_alias_jumeau() {
    // Un jumeau (`prod` et `prod-tunnel` vers deploy@10.0.0.1:22) partage
    // l'entrée : changer le port de l'un copie le secret vers la nouvelle cible
    // mais NE l'oublie PAS pour l'ancienne, que le jumeau réclame encore.
    assert_eq!(plan_deplacement(true, false), (true, false));
    // Aucun jumeau, cible libre : déplacement classique (copier puis oublier).
    assert_eq!(plan_deplacement(false, false), (true, true));
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

#[test]
fn utf8_un_flot_d_octets_invalides_ne_gonfle_pas_le_carry() {
    // Trouvé par l'audit du 7 septembre 2026 : sur un octet invalide, `push`
    // ne sautait qu'une séquence et différait le reste du bloc dans `carry`,
    // qui gonflait sans borne (un `cat` d'un binaire faisait du O(n²) et tuait
    // l'onglet). Tout le bloc doit être consommé ; `carry` ne retient qu'une
    // éventuelle séquence tronquée de fin (au plus 3 octets).
    let mut d = Utf8Stream::default();
    for _ in 0..1000 {
        let _ = d.push(&[0x80u8; 512]); // octets de continuation, tous invalides
        assert!(d.carry.len() <= 3, "carry non borné : {}", d.carry.len());
    }
    // Le texte qui suit un octet invalide sort dans le MÊME appel, pas au suivant.
    let out = d.push(&[0xFF, b'O', b'K']);
    assert!(out.ends_with("OK"), "{out:?}");
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
    assert_eq!(t.password.as_deref().map(String::as_str), Some("secret"));
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

/// Application factice qui gère TOUS les états de `lib.rs::run` : un test de
/// n'importe quel module (`rdp.rs`, `sante.rs`, `tests_reseau.rs`) la prend
/// telle quelle au lieu de réinventer la sienne. Audit du 12 septembre 2026
/// (C-couv-8) : `rdp.rs` avait son propre `app_de_test`, qui ne gérait que
/// `RdpStore`, et chaque nouveau module de test recopiait l'état factice.
pub(crate) fn app_de_test() -> tauri::App<tauri::test::MockRuntime> {
    tauri::test::mock_builder()
        .manage(SessionStore::default())
        .manage(TunnelStore::default())
        .manage(TransfertsStore::default())
        .manage(ChoixLocaux::default())
        .manage(crate::rdp::RdpStore::default())
        .manage(Accuses::default())
        .manage(TrousseauSignale::default())
        .manage(RenduTerminal::default())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("application factice")
}

/// Poignée de session sans transport : SFTP et exécution rendent une erreur,
/// le clavier part dans le vide.
pub(crate) fn poignee(epoch: u64) -> SessionHandle {
    poignee_avec_clavier(epoch).0
}

/// Comme `poignee`, mais rend aussi le récepteur du clavier : un test voit ce
/// que `pty_write` ou `snippet_send` a réellement envoyé au canal. Le garder
/// vivant compte : lâché, le canal se ferme et `pty_write` rend une erreur.
pub(crate) fn poignee_avec_clavier(
    epoch: u64,
) -> (SessionHandle, tokio::sync::mpsc::Receiver<Vec<u8>>) {
    let (input, clavier) = tokio::sync::mpsc::channel(16);
    let (resize, _) = tokio::sync::mpsc::channel(1);
    let h = SessionHandle {
        epoch,
        input,
        resize,
        sftp: Mutex::new(None),
        ouvrir_sftp: std::sync::Arc::new(|| {
            Box::pin(async { Err("pas de transport dans ce test".to_owned()) })
        }),
        executer: std::sync::Arc::new(|_, _| {
            Box::pin(async { Err("pas de transport dans ce test".to_owned()) })
        }),
        label: "h".into(),
        cible: ("h".into(), 22, "u".into()),
        enregistreur: std::sync::Arc::new(Mutex::new(None)),
    };
    (h, clavier)
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
    pty_resize(
        app.handle().clone(),
        app.state::<SessionStore>(),
        7,
        100,
        30,
    )
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
    pty_close(app.handle().clone(), app.state::<SessionStore>(), 1)
        .await
        .unwrap();
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
    pty_close(app.handle().clone(), app.state::<SessionStore>(), 2)
        .await
        .unwrap();
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

/// Trouvé par l'audit du 7 septembre 2026 : `clore_session` testait l'époque
/// (`is_superseded`) puis retirait l'entrée sous DEUX prises de verrou
/// distinctes. Une session enregistrée entre les deux était alors retirée à la
/// place de l'ancienne, et son pump émettait `pty-closed` pour un onglet que le
/// front croyait vivant (renumérotation après rechargement de la webview).
///
/// Deux fils martèlent le même identifiant : l'un enregistre l'époque
/// « nouvelle », l'autre clôt l'époque « ancienne » que le magasin portait
/// avant le tour. La clôture de l'ancienne ne doit JAMAIS emporter la nouvelle :
/// à la fin de chaque tour, l'entrée doit exister et porter la nouvelle époque.
/// Le test tourne sous un runtime multi-thread : en `current_thread`, l'absence
/// d'`await` dans `clore_session` rend l'entrelacement impossible et le trou
/// resterait invisible (c'est le piège qui l'a laissé passer).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clore_session_ne_retire_que_l_epoque_qu_on_lui_donne() {
    use std::sync::Arc;
    use std::sync::Barrier;

    const N: u64 = 20_000;
    const ID: u64 = 3;

    let app = app_de_test();
    let debut = Arc::new(Barrier::new(3));
    let fin = Arc::new(Barrier::new(3));

    // Fil qui enregistre l'époque « nouvelle » du tour.
    let fil_reg = {
        let handle = app.handle().clone();
        let debut = debut.clone();
        let fin = fin.clone();
        std::thread::spawn(move || {
            for i in 1..=N {
                debut.wait();
                let e_new = 2 * i;
                enregistrer_session(&handle.state::<SessionStore>(), ID, poignee(e_new)).unwrap();
                fin.wait();
            }
        })
    };
    // Fil qui clôt l'époque « ancienne » que le magasin portait au départ.
    let fil_clo = {
        let handle = app.handle().clone();
        let debut = debut.clone();
        let fin = fin.clone();
        std::thread::spawn(move || {
            for i in 1..=N {
                debut.wait();
                let e_prev = 2 * i - 1;
                clore_session(&handle, ID, e_prev);
                fin.wait();
            }
        })
    };

    for i in 1..=N {
        let e_prev = 2 * i - 1;
        let e_new = 2 * i;
        // On amorce le magasin avec l'époque « ancienne » : la clôture concurrente
        // la voit et croit avoir le droit de retirer, tandis que l'enregistrement
        // concurrent la remplace par l'époque « nouvelle ».
        enregistrer_session(&app.state::<SessionStore>(), ID, poignee(e_prev)).unwrap();
        debut.wait();
        fin.wait();
        // Sous le correctif, seule une clôture de `e_new` pourrait retirer
        // l'entrée, et personne ne la demande ce tour-ci : l'entrée doit donc
        // exister et porter `e_new`. Le défaut la retirait à la place de
        // l'ancienne, laissant le magasin vide.
        let epoque = app
            .state::<SessionStore>()
            .inner
            .lock()
            .unwrap()
            .get(&ID)
            .map(|h| h.epoch);
        assert_eq!(
            epoque,
            Some(e_new),
            "la clôture de l'époque {e_prev} a emporté l'époque {e_new} (tour {i})"
        );
    }

    fil_reg.join().unwrap();
    fil_clo.join().unwrap();
}

/// Trouvé par l'audit du 7 septembre 2026 : quand une copie directe (scp)
/// retenait le verrou de session, le `disconnect().await` du pump — et donc le
/// retrait du magasin et `pty-closed` — attendait la fin de la copie. Shell
/// mort, l'onglet restait alors listé « connecté » et chaque frappe rendait
/// « channel closed ». On vérifie que la fermeture de l'onglet précède la
/// déconnexion : même si celle-ci ne rend jamais la main, l'entrée a déjà quitté
/// le magasin. Avant le correctif (déconnexion d'abord), l'entrée y restait.
#[tokio::test]
async fn le_pump_ferme_l_onglet_avant_le_disconnect_bloque() {
    let app = app_de_test();
    let id = 5u64;
    let epoch = 9u64;
    enregistrer_session(&app.state::<SessionStore>(), id, poignee(epoch)).unwrap();

    // `deconnexion` qui ne se résout jamais : imite un `disconnect().await`
    // retenu par une copie directe toujours en cours.
    let jamais = tokio::sync::Notify::new();
    let handle = app.handle().clone();
    tokio::select! {
        () = fermer_onglet_apres_pump(&handle, id, epoch, async { jamais.notified().await }) => {
            panic!("la déconnexion ne devait jamais rendre la main");
        }
        () = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
    }
    assert!(
        app.state::<SessionStore>()
            .inner
            .lock()
            .unwrap()
            .get(&id)
            .is_none(),
        "l'onglet doit avoir quitté le magasin avant que le disconnect ne rende la main"
    );
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
    assert!(
        pty_resize(app.handle().clone(), app.state::<SessionStore>(), 9, 80, 24)
            .await
            .is_err()
    );
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

/// Un écrivain qui accepte l'en-tête mais refuse toute écriture qui porte un
/// événement (`"o"` ou `"r"`) : il rejoue un disque qui se remplit dès la
/// première sortie, sans dépendre de `/dev/full` (absent sous Windows).
///
/// Depuis le contrat K4 (audit du 12 septembre 2026), l'enregistreur tamponne
/// et ne vide qu'à `vider()` : en-tête et première sortie partent alors dans la
/// même écriture. Compter les écritures ne distinguait plus rien ; regarder ce
/// qu'elles portent, si.
struct PleinApresEntete {
    ecritures: usize,
}

impl std::io::Write for PleinApresEntete {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.ecritures += 1;
        let evenement = buf.windows(3).any(|w| w == b"\"o\"" || w == b"\"r\"");
        if evenement {
            Err(std::io::Error::other("No space left on device"))
        } else {
            Ok(buf.len()) // l'en-tête
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn enregistreur_condamne() -> avash::enregistrement::Enregistreur {
    avash::enregistrement::Enregistreur::depuis_ecrivain(
        Box::new(PleinApresEntete { ecritures: 0 }),
        std::path::PathBuf::from("/inexistant/plein.cast"),
        "h",
        80,
        24,
    )
    .expect("l'en-tête passe")
}

/// Le pump retire l'enregistreur dès qu'une écriture est refusée, au lieu de
/// l'avaler et de poursuivre après un trou. Avant l'audit du 7 septembre 2026,
/// `let _ = e.sortie(&text)` laissait l'enregistreur en place et le voyant
/// « rec » allumé sur un fichier lacunaire.
#[tokio::test]
async fn le_pump_retire_l_enregistreur_sur_ecriture_refusee() {
    let app = app_de_test();
    let slot: Enregistrement = std::sync::Arc::new(Mutex::new(Some(enregistreur_condamne())));
    let (tx, rx) = tokio::sync::mpsc::channel::<Vec<u8>>(4);
    tx.send(b"bonjour\r\n".to_vec()).await.unwrap();
    drop(tx); // ferme le canal : le pump s'arrête après avoir traité l'octet
    relayer_sortie(app.handle(), 1, rx, slot.clone()).await;
    assert!(
        slot.lock().unwrap().is_none(),
        "un refus d'écriture doit retirer l'enregistreur du magasin"
    );
}

/// Un redimensionnement écrit lui aussi dans l'enregistrement : une écriture
/// refusée doit le retirer comme le pump, sinon `pty_resize` restait le seul à
/// continuer d'écrire dans un enregistreur condamné. Audit du 7 septembre 2026.
#[tokio::test]
async fn un_redimensionnement_refuse_retire_l_enregistreur() {
    let app = app_de_test();
    let state = app.state::<SessionStore>();
    let mut h = poignee(1);
    h.enregistreur = std::sync::Arc::new(Mutex::new(Some(enregistreur_condamne())));
    let slot = h.enregistreur.clone();
    enregistrer_session(&state, 8, h).unwrap();
    pty_resize(
        app.handle().clone(),
        app.state::<SessionStore>(),
        8,
        100,
        30,
    )
    .await
    .unwrap();
    assert!(
        slot.lock().unwrap().is_none(),
        "un redimensionnement refusé doit retirer l'enregistreur"
    );
}

/// Fermer l'onglet ferme le fichier : `pty_close` arrête explicitement
/// l'enregistrement et prévient si le fichier est incomplet, au lieu de laisser
/// le `Drop` du `BufWriter` avaler l'erreur de vidage. Audit du 7 septembre 2026.
#[tokio::test]
async fn fermer_l_onglet_arrete_l_enregistrement_et_signale_l_echec() {
    let app = app_de_test();
    let state = app.state::<SessionStore>();
    let mut h = poignee(1);
    let mut enr = enregistreur_condamne();
    // Un vidage refusé condamne l'enregistreur (contrat K4 : l'écriture est
    // tamponnée, l'erreur apparaît au vidage) : la fermeture doit passer par la
    // branche qui signale l'échec, sans planter.
    enr.sortie("x").unwrap();
    assert!(enr.vider().is_err());
    h.enregistreur = std::sync::Arc::new(Mutex::new(Some(enr)));
    let slot = h.enregistreur.clone();
    enregistrer_session(&state, 10, h).unwrap();
    pty_close(app.handle().clone(), app.state::<SessionStore>(), 10)
        .await
        .unwrap();
    assert!(
        slot.lock().unwrap().is_none(),
        "fermer l'onglet doit fermer (retirer) l'enregistreur, pas le lâcher"
    );
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

#[tokio::test]
async fn open_external_refuse_les_schemas_dangereux() {
    // Un lien du terminal ne doit jamais ouvrir file://, javascript:, etc.
    for mauvais in [
        "file:///etc/passwd",
        "javascript:alert(1)",
        "data:text/html,<script>",
        "vbscript:x",
        "  file:///home",
    ] {
        assert!(
            open_external(mauvais.into()).await.is_err(),
            "devrait refuser : {mauvais}"
        );
    }
}

// ---------- Copie directe (scp chez la source) : annulation ----------
//
// Trouvé par l'audit du 7 septembre 2026 : la branche `direct` de
// `sftp_copier_vers` n'inscrivait aucun drapeau d'annulation. `sftp_annuler`
// rendait donc `false` et le bouton « Annuler » du front, affiché sur toute
// ligne en cours, ne coupait rien, sans le dire.

/// Une poignée de session dont l'`executer` mime `run_avec_agent` : il tourne
/// jusqu'à ce que l'interface lève le drapeau d'annulation, puis rend l'erreur
/// `ANNULE` que le front reconnaît. C'est la source d'une copie directe.
pub(crate) fn poignee_scp_annulable(epoch: u64) -> SessionHandle {
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
        executer: std::sync::Arc::new(|_commande, annulation| {
            Box::pin(async move {
                loop {
                    if annulation
                        .as_ref()
                        .is_some_and(|a| a.load(std::sync::atomic::Ordering::Relaxed))
                    {
                        return Err(avash::sftp::ANNULE.to_owned());
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                }
            })
        }),
        label: "src".into(),
        cible: ("s".into(), 22, "u".into()),
        enregistreur: std::sync::Arc::new(Mutex::new(None)),
    }
}

#[tokio::test]
async fn une_copie_directe_s_annule_pendant_le_transfert() {
    let app = app_de_test();
    enregistrer_session(&app.state::<SessionStore>(), 1, poignee_scp_annulable(1)).unwrap();
    // La cible ne sert que par son adresse (arguments de scp) : une poignée
    // ordinaire suffit, son `executer` n'est jamais appelé.
    enregistrer_session(&app.state::<SessionStore>(), 2, poignee(1)).unwrap();

    let copie = sftp_copier_vers(
        app.handle().clone(),
        app.state::<SessionStore>(),
        app.state::<TransfertsStore>(),
        1,
        42,
        "f.txt".into(),
        false,
        2,
        "/tmp".into(),
        true,
    );
    // En parallèle, on lève le drapeau dès que le transfert est inscrit :
    // `sftp_annuler` doit rendre `true` (c'est ce qui manquait), puis la copie
    // doit se terminer en `Err` contenant « Transfert annulé ».
    let annule = async {
        loop {
            if sftp_annuler(app.state::<TransfertsStore>(), 42) {
                break true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    };
    let (issue, a_rendu_true) = tokio::join!(copie, annule);
    assert!(
        a_rendu_true,
        "sftp_annuler doit trouver le transfert de copie directe et rendre true"
    );
    let e = issue.expect_err("la copie directe annulée doit rendre une erreur");
    assert!(
        e.contains("Transfert annulé"),
        "le front reconnaît ce marqueur : {e}"
    );
    // Le drapeau a bien été retiré à la fin de la commande.
    assert!(
        !sftp_annuler(app.state::<TransfertsStore>(), 42),
        "après la fin, plus aucun transfert 42 à annuler"
    );
}

#[tokio::test]
async fn une_copie_directe_terminee_ne_reste_pas_annulable() {
    // Régression sur le retrait : après une copie directe menée à bien,
    // `sftp_annuler` doit rendre `false` (le drapeau a été retiré), sinon la
    // ligne resterait « annulable » alors que le transfert est fini.
    let app = app_de_test();
    let (input, _) = tokio::sync::mpsc::channel(1);
    let (resize, _) = tokio::sync::mpsc::channel(1);
    let source = SessionHandle {
        epoch: 1,
        input,
        resize,
        sftp: Mutex::new(None),
        ouvrir_sftp: std::sync::Arc::new(|| {
            Box::pin(async { Err("pas de transport dans ce test".to_owned()) })
        }),
        // scp « réussit » tout de suite (code 0).
        executer: std::sync::Arc::new(|_c, _a| Box::pin(async { Ok((String::new(), 0u32)) })),
        label: "src".into(),
        cible: ("s".into(), 22, "u".into()),
        enregistreur: std::sync::Arc::new(Mutex::new(None)),
    };
    enregistrer_session(&app.state::<SessionStore>(), 1, source).unwrap();
    enregistrer_session(&app.state::<SessionStore>(), 2, poignee(1)).unwrap();

    let cible = sftp_copier_vers(
        app.handle().clone(),
        app.state::<SessionStore>(),
        app.state::<TransfertsStore>(),
        1,
        7,
        "f.txt".into(),
        false,
        2,
        "/tmp".into(),
        true,
    )
    .await
    .expect("la copie directe doit réussir");
    assert_eq!(cible, "/tmp/f.txt");
    assert!(
        !sftp_annuler(app.state::<TransfertsStore>(), 7),
        "le transfert terminé a été retiré du magasin"
    );
}

// ---------- ChoixLocaux : les chemins que l'utilisateur a désignés ----------

/// Trouvé par l'audit du 9 septembre 2026 et laissé en réserve par sa
/// relecture : exiger de `sftp_upload` un chemin absolu et existant ne fermait
/// pas l'exfiltration, `~/.ssh/id_ed25519` étant absolu et existant. Ce que le
/// front sait d'un fichier à envoyer, il le tient de la boîte de sélection ou
/// d'un dépôt sur la fenêtre, deux gestes que le natif voit passer : il retient
/// ces chemins, et n'envoie ensuite que ce qu'il a vu l'utilisateur désigner.
#[test]
fn seul_un_chemin_designe_par_l_utilisateur_peut_etre_envoye() {
    let _g = with_ssh_config("");
    let dir = avash::sftp::default_local_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let choisi = dir.join("rapport.pdf");
    let voisin = dir.join("id_ed25519");
    std::fs::write(&choisi, b"choisi").unwrap();
    std::fs::write(&voisin, b"secret").unwrap();

    let choix = ChoixLocaux::default();
    assert!(
        source_autorisee(&choix, &choisi.to_string_lossy()).is_err(),
        "rien n'est envoyable tant que l'utilisateur n'a rien désigné"
    );
    choix.retenir([choisi.clone()]);
    assert_eq!(
        source_autorisee(&choix, &choisi.to_string_lossy()).unwrap(),
        choisi,
        "un chemin désigné est accepté tel quel"
    );
    let refus = source_autorisee(&choix, &voisin.to_string_lossy()).unwrap_err();
    assert!(
        refus.contains("désigné"),
        "le refus doit dire que le chemin n'a pas été désigné : {refus}"
    );
}

/// La désignation complète la garde d'existence, elle ne la remplace pas : un
/// chemin choisi puis supprimé avant l'envoi reste refusé, avec le même
/// message qu'avant.
#[test]
fn un_chemin_designe_mais_disparu_reste_refuse() {
    let _g = with_ssh_config("");
    let dir = avash::sftp::default_local_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let choisi = dir.join("ephemere.txt");
    std::fs::write(&choisi, b"x").unwrap();
    let choix = ChoixLocaux::default();
    choix.retenir([choisi.clone()]);
    std::fs::remove_file(&choisi).unwrap();
    let refus = source_autorisee(&choix, &choisi.to_string_lossy()).unwrap_err();
    assert!(refus.contains("n'existe pas"), "{refus}");
}

// ---------- Audit de sécurité du 12 septembre 2026 : surface IPC ----------

/// C-ipc-2 : `sftp_download` acceptait un chemin local imposé par la page, que
/// le front n'envoie jamais. Contrôlé seulement « absolu », il laissait un
/// script de la webview CRÉER un fichier au contenu d'un serveur de son choix
/// là où il voulait (`~/.config/autostart/x.desktop`). Le paramètre disparaît :
/// garde de contrat sur la signature, faute de pouvoir appeler la commande
/// sans session SFTP.
#[test]
fn sftp_download_ne_prend_plus_de_chemin_local_de_la_page() {
    let source = include_str!("sftp.rs");
    let debut = source.find("pub async fn sftp_download(").unwrap();
    let fin = debut + source[debut..].find(") -> Result").unwrap();
    let signature = &source[debut..fin];
    assert!(
        !signature.contains("local"),
        "sftp_download ne doit prendre aucun chemin local : {signature}"
    );
}

/// C-SIL-14 : ouvrir le panneau SFTP pendant une copie directe attendait la
/// fin de la copie derrière le verrou de la session, des minutes sous
/// « Chargement… ». La poignée rejoue ce verrou : la copie le tient, et
/// l'ouverture du canal SFTP l'attend. La réponse doit venir tout de suite.
#[tokio::test]
async fn ouvrir_le_sftp_pendant_une_copie_directe_repond_tout_de_suite() {
    let app = app_de_test();
    let verrou = std::sync::Arc::new(tokio::sync::Mutex::new(()));
    let mut source = poignee_scp_annulable(1);
    {
        let v = verrou.clone();
        source.ouvrir_sftp = std::sync::Arc::new(move || {
            let v = v.clone();
            Box::pin(async move {
                let _g = v.lock().await;
                Err("pas de transport dans ce test".to_owned())
            })
        });
        let v = verrou.clone();
        let executer = source.executer.clone();
        source.executer = std::sync::Arc::new(move |c, a| {
            let v = v.clone();
            let executer = executer.clone();
            Box::pin(async move {
                let _g = v.lock().await;
                executer(c, a).await
            })
        });
    }
    enregistrer_session(&app.state::<SessionStore>(), 1, source).unwrap();
    enregistrer_session(&app.state::<SessionStore>(), 2, poignee(1)).unwrap();
    let copie = sftp_copier_vers(
        app.handle().clone(),
        app.state::<SessionStore>(),
        app.state::<TransfertsStore>(),
        1,
        42,
        "f.txt".into(),
        false,
        2,
        "/tmp".into(),
        true,
    );
    let pendant = async {
        // La copie est inscrite (donc lancée) : on ouvre le panneau.
        while !app
            .state::<TransfertsStore>()
            .inner
            .lock()
            .unwrap()
            .contains_key(&42)
        {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        let issue = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            sftp_of(&app.state::<SessionStore>(), 1),
        )
        .await;
        assert!(sftp_annuler(app.state::<TransfertsStore>(), 42));
        issue
    };
    let (_, issue) = tokio::join!(copie, pendant);
    let Ok(Err(e)) = issue else {
        panic!("l'ouverture du panneau a attendu la fin de la copie directe")
    };
    assert!(e.contains("copie directe"), "{e}");
    // La copie finie, le panneau s'ouvre de nouveau normalement.
    let e = sftp_of(&app.state::<SessionStore>(), 1)
        .await
        .err()
        .unwrap();
    assert_eq!(e, "pas de transport dans ce test");
}

// ---------- Relais de la sortie : regroupement et contre-pression ----------

/// L'émetteur d'un relais de test, sa tâche, et les messages reçus.
type RelaisDeTest = (
    tokio::sync::mpsc::Sender<Vec<u8>>,
    tokio::task::JoinHandle<()>,
    std::sync::Arc<Mutex<Vec<serde_json::Value>>>,
);

/// Pousse des blocs dans un relais lancé à part, et rend l'émetteur, la tâche
/// et les messages `pty-output` reçus.
fn relais_de_test(app: &tauri::App<tauri::test::MockRuntime>) -> RelaisDeTest {
    let recus = ecouter(app, "pty-output");
    let (tx, rx) = tokio::sync::mpsc::channel::<Vec<u8>>(32);
    let h = app.handle().clone();
    let tache = tokio::spawn(async move {
        relayer_sortie(&h, 1, rx, std::sync::Arc::new(Mutex::new(None))).await;
    });
    (tx, tache, recus)
}

fn donnees(recus: &Mutex<Vec<serde_json::Value>>) -> Vec<String> {
    recus
        .lock()
        .unwrap()
        .iter()
        .map(|v| v["data"].as_str().unwrap().to_owned())
        .collect()
}

/// Audit du 12 septembre 2026 (C-perf-1) : le regroupement était en fin de
/// fenêtre, et chaque écho de frappe attendait 8 ms avant de partir (10 ms
/// d'écho médian mesurés sur la boucle locale). Temps figé : l'écho doit être
/// émis sans que l'horloge avance d'un instant.
#[tokio::test(start_paused = true)]
async fn un_echo_isole_part_sans_attendre_la_fenetre_de_regroupement() {
    let app = app_de_test();
    let (tx, tache, recus) = relais_de_test(&app);
    let depart = tokio::time::Instant::now();
    tx.send(b"a".to_vec()).await.unwrap();
    for _ in 0..100 {
        if !recus.lock().unwrap().is_empty() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(tokio::time::Instant::now(), depart, "l'horloge a avancé");
    assert_eq!(donnees(&recus), ["a"]);
    assert_eq!(recus.lock().unwrap()[0]["seq"], 1);
    drop(tx);
    tache.await.unwrap();
}

/// Ce qui suit l'écho dans la même fenêtre part en un seul message : le
/// regroupement reste, il a seulement changé de bord.
#[tokio::test(start_paused = true)]
async fn deux_blocs_dans_la_meme_fenetre_ne_font_qu_un_message() {
    let app = app_de_test();
    let (tx, tache, recus) = relais_de_test(&app);
    tx.send(b"a".to_vec()).await.unwrap();
    while recus.lock().unwrap().is_empty() {
        tokio::task::yield_now().await;
    }
    tx.send(b"b".to_vec()).await.unwrap();
    tx.send(b"c".to_vec()).await.unwrap();
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        donnees(&recus),
        ["a"],
        "b et c attendent la fin de la fenêtre"
    );
    tokio::time::sleep(std::time::Duration::from_millis(9)).await;
    assert_eq!(donnees(&recus), ["a", "bc"]);
    assert_eq!(recus.lock().unwrap()[1]["seq"], 2);
    drop(tx);
    tache.await.unwrap();
}

/// Envoie `0` à `9`, un bloc toutes les 10 ms (chacun après une fenêtre
/// calme, donc émis seul tant que la contre-pression le permet).
async fn dix_blocs_espaces(tx: &tokio::sync::mpsc::Sender<Vec<u8>>) {
    for i in 0..10 {
        tx.send(i.to_string().into_bytes()).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

/// Contrat K6 (audit du 12 septembre 2026) : sans accusé du front, le relais
/// n'a jamais plus de quatre messages en vol ; un accusé rouvre la fenêtre.
#[tokio::test(start_paused = true)]
async fn le_relais_n_emet_pas_plus_de_quatre_messages_sans_accuse() {
    let app = app_de_test();
    let (tx, tache, recus) = relais_de_test(&app);
    dix_blocs_espaces(&tx).await;
    assert_eq!(
        donnees(&recus),
        ["0", "1", "2", "3"],
        "quatre messages au plus sans accusé"
    );
    pty_ack(app.state::<Accuses>(), 1, 2);
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let recu = donnees(&recus);
    assert_eq!(recu.len(), 6, "{recu:?}");
    assert_eq!(recu.concat(), "0123456789");
    drop(tx);
    tache.await.unwrap();
}

/// Contrat K6 : un front qui n'accuse jamais ne gèle pas l'onglet ; passé le
/// filet de 250 ms, le relais reprend.
#[tokio::test(start_paused = true)]
async fn sans_accuse_le_relais_reprend_apres_le_filet() {
    let app = app_de_test();
    let (tx, tache, recus) = relais_de_test(&app);
    dix_blocs_espaces(&tx).await;
    assert_eq!(donnees(&recus).len(), 4);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let recu = donnees(&recus);
    assert!(recu.len() > 4, "{recu:?}");
    assert_eq!(recu.concat(), "0123456789");
    drop(tx);
    tache.await.unwrap();
}

/// Un accusé pour un onglet inconnu (fermé entre-temps) ne fait rien.
#[test]
fn un_accuse_pour_un_onglet_inconnu_est_ignore() {
    let app = app_de_test();
    pty_ack(app.state::<Accuses>(), 77, 3);
}

/// Audit du 12 septembre 2026 (C-perf-8) : un bloc valide sans reliquat,
/// cas de presque tous les blocs, ne passe plus par `carry`.
#[test]
fn un_bloc_valide_sans_reliquat_ne_passe_pas_par_carry() {
    let mut d = Utf8Stream::default();
    assert_eq!(d.push(b"abc"), "abc");
    assert_eq!(d.carry.capacity(), 0, "le bloc a été recopié dans carry");
    // Le reliquat, lui, est toujours recollé.
    assert_eq!(d.push(&[0xC3]), "");
    assert_eq!(d.push(&[0xA9, b'!']), "é!");
}

/// Un écrivain qui panique : rejoue un défaut du code d'écriture de
/// l'enregistrement, qui ferait paniquer la tâche du relais.
struct Paniqueur;

impl std::io::Write for Paniqueur {
    fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
        panic!("écriture de l'enregistrement impossible (panique de test)");
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Audit du 12 septembre 2026 (C-SIL-9) : tokio avale la panique d'une tâche
/// détachée, et l'onglet restait « connecté » sans session. La garde
/// `FinDeSession` le ferme quel que soit le chemin de sortie.
#[tokio::test]
async fn une_panique_du_relais_ferme_quand_meme_l_onglet() {
    let app = app_de_test();
    let fermes = ecouter(&app, "pty-closed");
    let mut h = poignee(3);
    h.enregistreur = std::sync::Arc::new(Mutex::new(Some(
        avash::enregistrement::Enregistreur::depuis_ecrivain(
            Box::new(Paniqueur),
            std::path::PathBuf::from("/inexistant/panique.cast"),
            "h",
            80,
            24,
        )
        .unwrap(),
    )));
    let slot = h.enregistreur.clone();
    enregistrer_session(&app.state::<SessionStore>(), 5, h).unwrap();
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    let tache = lancer_relais(app.handle().clone(), 5, 3, rx, slot, async {}, async {});
    tx.send(b"x".to_vec()).await.unwrap();
    let issue = tache.await;
    assert!(issue.unwrap_err().is_panic(), "le relais devait paniquer");
    assert!(
        !app.state::<SessionStore>()
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(&5),
        "l'onglet est resté dans le magasin"
    );
    assert_eq!(
        *fermes.lock().unwrap(),
        [serde_json::json!({ "id": 5 })],
        "pty-closed doit partir"
    );
}

/// Audit du 12 septembre 2026 (C-panique-4) : un verrou empoisonné par une
/// panique ailleurs condamnait toutes les commandes du magasin. Frappe et
/// fermeture doivent répondre malgré tout.
#[tokio::test]
async fn un_verrou_empoisonne_n_empeche_ni_d_ecrire_ni_de_fermer() {
    let app = app_de_test();
    let (h, mut clavier) = poignee_avec_clavier(1);
    enregistrer_session(&app.state::<SessionStore>(), 1, h).unwrap();
    let handle = app.handle().clone();
    let _ = std::thread::spawn(move || {
        let s = handle.state::<SessionStore>();
        let _g = s.inner.lock().unwrap();
        panic!("verrou empoisonné (panique de test)");
    })
    .join();
    assert!(app.state::<SessionStore>().inner.is_poisoned());
    pty_write(app.state::<SessionStore>(), 1, "ls".into())
        .await
        .unwrap();
    assert_eq!(clavier.recv().await.unwrap(), b"ls");
    pty_close(app.handle().clone(), app.state::<SessionStore>(), 1)
        .await
        .unwrap();
    assert!(open_sessions(app.state::<SessionStore>()).is_empty());
}

// ---------- Trousseau en panne (contrat K1) ----------

/// Audit du 12 septembre 2026 (C-SIL-8) : une panne du trousseau valait
/// « pas de mot de passe », sans un mot. Elle est désormais signalée au front,
/// une fois par lancement, et la demande de saisie suit son cours.
#[tokio::test]
async fn un_trousseau_en_panne_est_signale_une_fois_par_lancement() {
    let _g = with_trousseau_en_panne("Host h\n  HostName 10.0.0.1\n  IdentityFile ~/.ssh/k\n");
    let app = app_de_test();
    let signales = ecouter(&app, "trousseau-indisponible");
    for _ in 0..2 {
        // Une clé déclarée suffit : pas de saisie à réclamer.
        assert!(!host_needs_password(app.handle().clone(), "h".into())
            .await
            .unwrap());
        assert!(!password_known(app.handle().clone(), "10.0.0.1".into(), None, None).await);
    }
    let recus = signales.lock().unwrap().clone();
    assert_eq!(recus.len(), 1, "{recus:?}");
    let message = recus[0]["message"].as_str().unwrap();
    assert!(message.contains("trousseau"), "{message}");
}

/// Même panne à la modification d'un hôte : l'hôte change, et la commande dit
/// que le mot de passe n'a pas suivi au lieu de rendre `Ok` sans rien faire.
#[tokio::test]
async fn modifier_un_hote_quand_le_trousseau_est_en_panne_le_dit() {
    let _g = with_trousseau_en_panne("Host web\n  HostName 10.0.0.1\n  User deploy\n");
    let e = host_update(
        "web".into(),
        "web".into(),
        "10.0.0.2".into(),
        None,
        Some("deploy".into()),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap_err();
    assert!(e.contains("n'a pas pu être déplacé"), "{e}");
    assert_eq!(
        find_host("web").unwrap().hostname.as_deref(),
        Some("10.0.0.2")
    );
}

// ---------- Secrets : commandes (C-couv-1) ----------
//
// Audit du 12 septembre 2026 : les décisions pures (`plan_deplacement`,
// `identifiant_encore_utilise`) étaient testées, pas les commandes qui les
// appliquent. Inverser `save` et `forget` dans `host_update` passait `check.sh`.

async fn connu(
    app: &tauri::App<tauri::test::MockRuntime>,
    addr: &str,
    port: Option<u16>,
    user: &str,
) -> bool {
    password_known(app.handle().clone(), addr.into(), port, Some(user.into())).await
}

/// LE test du bug de la 0.10 (« mémoriser » cassé pour tout hôte sans
/// `User`) : écriture par `password_save`, relecture par la résolution d'alias
/// ET par `password_known`.
#[tokio::test]
async fn le_mot_de_passe_memorise_est_relu_par_la_resolution_d_alias() {
    let _g = with_ssh_config("Host h\n  HostName 10.0.0.1\n");
    let app = app_de_test();
    password_save("10.0.0.1".into(), None, None, "s3cr3t".into())
        .await
        .unwrap();
    let t = Target::from_alias("h").unwrap();
    assert_eq!(t.password.as_deref().map(String::as_str), Some("s3cr3t"));
    assert!(password_known(app.handle().clone(), "10.0.0.1".into(), None, None).await);
}

#[tokio::test]
async fn oublier_un_mot_de_passe_le_retire_et_ne_se_plaint_pas_deux_fois() {
    let _g = with_ssh_config("");
    let app = app_de_test();
    password_save("10.0.0.1".into(), None, Some("u".into()), "s".into())
        .await
        .unwrap();
    assert!(connu(&app, "10.0.0.1", None, "u").await);
    password_forget("10.0.0.1".into(), None, Some("u".into()))
        .await
        .unwrap();
    assert!(!connu(&app, "10.0.0.1", None, "u").await);
    password_forget("10.0.0.1".into(), None, Some("u".into()))
        .await
        .unwrap();
}

#[tokio::test]
async fn le_port_et_l_espace_de_l_adresse_sont_normalises_avant_le_trousseau() {
    let _g = with_ssh_config("");
    let app = app_de_test();
    password_save(
        "  10.0.0.1 ".into(),
        Some(22),
        Some(" deploy ".into()),
        "s".into(),
    )
    .await
    .unwrap();
    assert!(connu(&app, "10.0.0.1", None, "deploy").await);
}

const JUMEAUX: &str = "Host web\n  HostName 10.0.0.1\n  User deploy\n\nHost web-via-bastion\n  HostName 10.0.0.1\n  User deploy\n  ProxyJump bastion\n";

#[tokio::test]
async fn supprimer_un_hote_oublie_son_secret_seulement_s_il_est_orphelin() {
    let _g = with_ssh_config(JUMEAUX);
    let app = app_de_test();
    password_save("10.0.0.1".into(), None, Some("deploy".into()), "s".into())
        .await
        .unwrap();
    host_delete("web-via-bastion".into()).await.unwrap();
    assert!(
        connu(&app, "10.0.0.1", None, "deploy").await,
        "web le partage encore"
    );
    host_delete("web".into()).await.unwrap();
    assert!(!connu(&app, "10.0.0.1", None, "deploy").await);
    assert!(avash::parse_ssh_config().unwrap().is_empty());
}

#[tokio::test]
async fn supprimer_un_hote_inconnu_ne_touche_pas_au_trousseau() {
    let _g = with_ssh_config("Host web\n  HostName 10.0.0.1\n  User deploy\n");
    let app = app_de_test();
    password_save("10.0.0.1".into(), None, Some("deploy".into()), "s".into())
        .await
        .unwrap();
    assert!(host_delete("absent".into()).await.is_err());
    assert!(connu(&app, "10.0.0.1", None, "deploy").await);
}

/// `host_update` avec les champs d'une fiche : alias inchangé, nouvelle
/// adresse et port.
async fn repointer(ancien: &str, addr: &str, port: Option<u16>) -> Result<(), String> {
    host_update(
        ancien.into(),
        ancien.into(),
        addr.into(),
        port,
        Some("deploy".into()),
        None,
        None,
        None,
        None,
    )
    .await
}

#[tokio::test]
async fn modifier_l_adresse_d_un_hote_deplace_son_secret() {
    let _g = with_ssh_config("Host web\n  HostName 10.0.0.1\n  User deploy\n");
    let app = app_de_test();
    password_save("10.0.0.1".into(), None, Some("deploy".into()), "s".into())
        .await
        .unwrap();
    repointer("web", "10.0.0.2", None).await.unwrap();
    assert!(!connu(&app, "10.0.0.1", None, "deploy").await);
    assert!(connu(&app, "10.0.0.2", None, "deploy").await);
}

#[tokio::test]
async fn modifier_un_hote_vers_une_cible_occupee_garde_le_secret_de_la_cible() {
    let _g = with_ssh_config(
        "Host web\n  HostName 10.0.0.1\n  User deploy\n\nHost db\n  HostName 10.0.0.2\n  User deploy\n",
    );
    let app = app_de_test();
    password_save(
        "10.0.0.1".into(),
        None,
        Some("deploy".into()),
        "web-mdp".into(),
    )
    .await
    .unwrap();
    password_save(
        "10.0.0.2".into(),
        None,
        Some("deploy".into()),
        "db-mdp".into(),
    )
    .await
    .unwrap();
    repointer("web", "10.0.0.2", None).await.unwrap();
    let cible = avash::secrets::charger("deploy@10.0.0.2:22")
        .unwrap()
        .unwrap();
    assert_eq!(
        cible.as_str(),
        "db-mdp",
        "le secret de la cible a été écrasé"
    );
    assert!(
        connu(&app, "10.0.0.1", None, "deploy").await,
        "l'ancien est conservé"
    );
}

#[tokio::test]
async fn modifier_le_port_d_un_alias_jumeau_ne_perd_pas_le_secret_de_l_autre() {
    let _g = with_ssh_config(
        "Host prod\n  HostName 10.0.0.1\n  User deploy\n\nHost prod-tunnel\n  HostName 10.0.0.1\n  User deploy\n",
    );
    let app = app_de_test();
    password_save("10.0.0.1".into(), None, Some("deploy".into()), "s".into())
        .await
        .unwrap();
    repointer("prod", "10.0.0.1", Some(2222)).await.unwrap();
    assert!(
        connu(&app, "10.0.0.1", None, "deploy").await,
        "prod-tunnel le partage"
    );
    assert!(connu(&app, "10.0.0.1", Some(2222), "deploy").await);
}

#[tokio::test]
async fn une_modification_refusee_laisse_le_secret_en_place() {
    let _g = with_ssh_config("Host web\n  HostName 10.0.0.1\n  User deploy\n");
    let app = app_de_test();
    password_save("10.0.0.1".into(), None, Some("deploy".into()), "s".into())
        .await
        .unwrap();
    assert!(repointer("absent", "10.0.0.2", None).await.is_err());
    assert!(connu(&app, "10.0.0.1", None, "deploy").await);
}

/// Le formulaire d'édition reçoit les champs du bloc littéral, pas les
/// valeurs héritées d'un `Host *` (contrat commenté dans `Target::from_alias`).
#[test]
fn host_get_rend_les_champs_bruts_sans_les_valeurs_heritees() {
    let _g = with_ssh_config("Host *\n  User global\n\nHost h\n  HostName 10.0.0.1\n");
    let h = host_get("h".into()).unwrap();
    assert_eq!(h.user, None);
    assert_eq!(h.hostname.as_deref(), Some("10.0.0.1"));
}

/// Garde de contrat : ce qui part vers le front ne porte aucun secret.
#[tokio::test]
async fn host_get_ne_serialise_aucun_champ_secret() {
    let _g = with_ssh_config("Host h\n  HostName 10.0.0.1\n  User u\n");
    password_save(
        "10.0.0.1".into(),
        None,
        Some("u".into()),
        "tres-secret".into(),
    )
    .await
    .unwrap();
    let v = serde_json::to_value(host_get("h".into()).unwrap()).unwrap();
    assert!(v.get("password").is_none(), "{v}");
    assert!(!v.to_string().contains("tres-secret"), "{v}");
}

/// Oublier une clé d'hôte retire ses lignes, et elles seules.
#[test]
fn oublier_une_cle_d_hote_retire_ses_lignes_et_rend_leur_nombre() {
    const CLE: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIKZTt57uQjti9xLGa1ZWBlI1FtsRw8E9NL4ayZQ9dcD0";
    let g = with_ssh_config("");
    let chemin = g.dir.join(".ssh").join("known_hosts");
    std::fs::write(
        &chemin,
        format!("# commentaire\n[10.0.0.1]:2222 {CLE}\n10.0.0.9 {CLE}\n"),
    )
    .unwrap();
    assert_eq!(known_hosts_forget("10.0.0.1".into(), Some(2222)), Ok(1));
    let reste = std::fs::read_to_string(&chemin).unwrap();
    assert!(reste.contains("# commentaire"), "{reste}");
    assert!(reste.contains("10.0.0.9"), "{reste}");
    assert!(!reste.contains("[10.0.0.1]:2222"), "{reste}");
    assert_eq!(known_hosts_forget("10.0.0.1".into(), Some(2222)), Ok(0));
}

// ---------- Clés et hôtes enregistrés (C-couv-3) ----------

const CLE_PUBLIQUE: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIKZTt57uQjti9xLGa1ZWBlI1FtsRw8E9NL4ayZQ9dcD0 test";

/// `key_deploy` exige le mot de passe AVANT toute connexion : sans lui, il
/// n'a rien à faire (la clé serait déjà acceptée) et ne doit rien tenter.
#[tokio::test]
async fn key_deploy_refuse_sans_mot_de_passe_avant_toute_connexion() {
    let issue = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        key_deploy(
            "avash-inexistant.invalid".into(),
            None,
            "u".into(),
            String::new(),
            CLE_PUBLIQUE.into(),
        ),
    )
    .await
    .expect("la garde doit répondre sans tenter de connexion");
    let e = issue.unwrap_err();
    assert!(e.contains("mot de passe"), "{e}");
}

#[tokio::test]
async fn key_deploy_refuse_une_ligne_qui_n_est_pas_une_cle() {
    let e = key_deploy(
        "avash-inexistant.invalid".into(),
        None,
        "u".into(),
        "p".into(),
        "bonjour".into(),
    )
    .await
    .unwrap_err();
    assert!(e.contains("ne ressemble pas"), "{e}");
}

#[test]
fn host_save_rogne_analyse_les_etiquettes_et_ecrit_dans_la_config() {
    let g = with_ssh_config("");
    let h = host_save(
        " web ".into(),
        " 10.0.0.1 ".into(),
        Some(2222),
        " deploy ".into(),
        Some("  ".into()),
        Some(" bastion ".into()),
        Some("a, ,b ,".into()),
    )
    .unwrap();
    assert_eq!(h.alias, "web");
    assert_eq!(h.identity_file, None);
    assert_eq!(h.tags, ["a", "b"]);
    assert_eq!(h.proxy_jump.as_deref(), Some("bastion"));
    assert!(avash::parse_ssh_config()
        .unwrap()
        .iter()
        .any(|x| x.alias == "web"));
    let texte = std::fs::read_to_string(g.dir.join(".ssh").join("config")).unwrap();
    assert!(!texte.to_lowercase().contains("password"), "{texte}");
}

#[test]
fn host_save_refuse_un_alias_deja_declare() {
    let g = with_ssh_config("");
    let enregistrer = || {
        host_save(
            "web".into(),
            "10.0.0.1".into(),
            None,
            "deploy".into(),
            None,
            None,
            None,
        )
    };
    enregistrer().unwrap();
    let avant = std::fs::read(g.dir.join(".ssh").join("config")).unwrap();
    let e = enregistrer().unwrap_err();
    assert!(e.contains("déjà déclaré"), "{e}");
    assert_eq!(
        std::fs::read(g.dir.join(".ssh").join("config")).unwrap(),
        avant
    );
}

// ---------- Snippets (C-couv-6) ----------

async fn envoyer(
    app: &tauri::App<tauri::test::MockRuntime>,
    ids: Vec<u64>,
    run: bool,
    crochets: Option<bool>,
) -> usize {
    snippet_send(
        app.state::<SessionStore>(),
        ids,
        "a\nb".into(),
        run,
        crochets,
    )
    .await
    .unwrap()
}

/// Correctif du 7 septembre 2026 : une insertion multi-lignes part entre
/// crochets de collage, sinon chaque ligne sauf la dernière s'exécutait.
#[tokio::test]
async fn inserer_un_snippet_multi_lignes_l_entoure_de_crochets_par_defaut() {
    let app = app_de_test();
    let (h, mut clavier) = poignee_avec_clavier(1);
    enregistrer_session(&app.state::<SessionStore>(), 1, h).unwrap();
    assert_eq!(envoyer(&app, vec![1], false, None).await, 1);
    assert_eq!(clavier.recv().await.unwrap(), b"\x1b[200~a\rb\x1b[201~");
}

#[tokio::test]
async fn executer_un_snippet_ajoute_le_retour_chariot_sans_crochets() {
    let app = app_de_test();
    let (h, mut clavier) = poignee_avec_clavier(1);
    enregistrer_session(&app.state::<SessionStore>(), 1, h).unwrap();
    envoyer(&app, vec![1], true, None).await;
    assert_eq!(clavier.recv().await.unwrap(), b"a\rb\r");
}

#[tokio::test]
async fn le_front_peut_refuser_les_crochets() {
    let app = app_de_test();
    let (h, mut clavier) = poignee_avec_clavier(1);
    enregistrer_session(&app.state::<SessionStore>(), 1, h).unwrap();
    envoyer(&app, vec![1], false, Some(false)).await;
    assert_eq!(clavier.recv().await.unwrap(), b"a\rb");
}

/// Une session absente ou fermée n'arrête pas l'envoi aux autres, et ne
/// compte pas parmi les sessions atteintes.
#[tokio::test]
async fn une_session_fermee_est_ignoree_et_les_autres_recoivent() {
    let app = app_de_test();
    let (h, mut clavier) = poignee_avec_clavier(1);
    enregistrer_session(&app.state::<SessionStore>(), 1, h).unwrap();
    // `poignee` lâche son récepteur : l'envoi y échoue.
    enregistrer_session(&app.state::<SessionStore>(), 2, poignee(1)).unwrap();
    assert_eq!(envoyer(&app, vec![1, 9, 2], true, None).await, 1);
    assert_eq!(clavier.recv().await.unwrap(), b"a\rb\r");
}

#[test]
fn snippet_vars_rend_les_variables_dans_l_ordre_sans_doublon() {
    assert_eq!(
        snippet_vars("ssh {{hote}} -p {{port}} {{hote}}".into()),
        ["hote", "port"]
    );
}

#[test]
fn snippet_list_lit_le_fichier_du_bac_a_sable() {
    let _g = with_ssh_config("");
    assert!(snippet_list().unwrap().is_empty());
    let s = snippet_save(None, "n".into(), "c".into(), true, None).unwrap();
    let liste = snippet_list().unwrap();
    assert_eq!(liste.len(), 1);
    assert_eq!(liste[0].id, s.id);
    snippet_delete(s.id).unwrap();
    assert!(snippet_list().unwrap().is_empty());
}

// ---------- Petites promesses (C-couv-7) ----------

/// Au plus un événement de progression toutes les 80 ms, et toujours le
/// dernier : une régression noierait l'IPC pendant un gros transfert.
#[tokio::test]
async fn la_progression_n_emet_qu_un_evenement_par_80_ms_et_toujours_le_dernier() {
    let app = app_de_test();
    let recus = ecouter(&app, "sftp-progress");
    let mut rapport = progress_reporter(app.handle(), 1, 7, "f", "download");
    for i in 0..100 {
        rapport("f", i, 100, 0, 1);
    }
    rapport("f", 100, 100, 0, 1);
    let recus = recus.lock().unwrap().clone();
    assert!((2..=3).contains(&recus.len()), "{} événements", recus.len());
    let dernier = recus.last().unwrap();
    assert_eq!(
        (dernier["done"].as_u64(), dernier["total"].as_u64()),
        (Some(100), Some(100))
    );
    assert_eq!(dernier["transfert"], 7);
}

/// Un arrêt demandé pendant l'ouverture note l'annulation, que
/// `tunnel_start` verra en arrivant ; sans ouverture en vol, rien n'est semé.
#[tokio::test]
async fn arreter_un_tunnel_pendant_son_ouverture_note_l_annulation() {
    let app = app_de_test();
    let tunnels = || app.state::<TunnelStore>();
    tunnels().en_cours.lock().unwrap().insert("t".into());
    tunnel_stop(tunnels(), "t".into()).await.unwrap();
    tunnel_stop(tunnels(), "u".into()).await.unwrap();
    let annules = tunnels().annules.lock().unwrap().clone();
    assert!(annules.contains("t"));
    assert!(!annules.contains("u"));
}

#[tokio::test]
async fn une_definition_de_tunnel_exige_un_alias_declare_et_un_type_connu() {
    let _g = with_ssh_config("Host h\n  HostName 10.0.0.1\n");
    let app = app_de_test();
    let definir = |alias: &str, kind: &str| {
        tunnel_def_save(
            None,
            alias.into(),
            kind.into(),
            8080,
            Some("x".into()),
            Some(80),
            None,
        )
    };
    assert!(definir("absent", "local")
        .unwrap_err()
        .contains("introuvable"));
    assert!(definir("h", "socks").is_err());
    let d = definir("h", "local").unwrap();
    assert!(tunnel_defs().unwrap().iter().any(|x| x.id == d.id));
    tunnel_def_delete(app.state::<TunnelStore>(), d.id)
        .await
        .unwrap();
    assert!(tunnel_defs().unwrap().is_empty());
}

#[test]
fn arreter_l_enregistrement_d_une_session_inconnue_est_une_erreur() {
    let app = app_de_test();
    let e = enregistrement_arreter(app.state::<SessionStore>(), 99).unwrap_err();
    assert!(e.contains("inconnue"), "{e}");
}

/// Sans chemin désigné, l'analyse lit les sessions `PuTTY` du répertoire
/// personnel, et dit où elle a regardé.
#[cfg(not(windows))]
#[test]
fn le_scan_par_defaut_lit_les_sessions_putty_du_repertoire_personnel() {
    let _g = with_ssh_config("");
    let dir = avash::import::repertoire_putty().unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("prod"), "HostName=10.0.0.7\nProtocol=ssh\n").unwrap();
    let bilan = import_scan(None).unwrap();
    assert!(
        bilan
            .consultes
            .iter()
            .any(|c| c == &dir.display().to_string()),
        "{:?}",
        bilan.consultes
    );
    assert!(
        bilan.candidats.iter().any(|c| c.host.alias == "prod"),
        "{:?}",
        bilan
            .candidats
            .iter()
            .map(|c| &c.host.alias)
            .collect::<Vec<_>>()
    );
}
