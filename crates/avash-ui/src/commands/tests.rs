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
    // Isole aussi le trousseau : `Target::from_alias` appelle `secrets::load`
    // et le diagnostic `secrets::sonder`, qui sans cette dérogation tapent dans
    // le vrai Secret Service du poste (keyring v1 sous Linux = un aller-retour
    // D-Bus). `with_ssh_config` isolait `~/.ssh`, pas le trousseau ; une entrée
    // réelle `deploy@10.0.0.1:2222` faisait alors échouer les `is_none()`.
    // Trouvé par l'audit du 7 septembre 2026. Voir `avash::secrets::en_memoire`.
    let previous_trousseau = std::env::var("AVASH_TROUSSEAU").ok();
    std::env::set_var("HOME", &dir);
    std::env::set_var("AVASH_HOME", &dir);
    std::env::set_var("AVASH_TROUSSEAU", "memoire");
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
        match &self.previous_trousseau {
            Some(t) => std::env::set_var("AVASH_TROUSSEAU", t),
            None => std::env::remove_var("AVASH_TROUSSEAU"),
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

// ---------- local_target ----------

/// Un chemin imposé libre reste respecté tel quel : la garde posée par l'audit
/// du 9 septembre 2026 ne déplace la cible que si elle est déjà prise. Le test
/// visait `/tmp/ailleurs.md` en dur, ce qui n'exerçait plus rien une fois la
/// garde en place (le fichier peut exister sur le poste) : il travaille
/// maintenant sous le HOME jetable du garde.
#[test]
fn local_target_respecte_le_chemin_impose() {
    let _g = with_ssh_config("");
    let dir = avash::sftp::default_local_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let vise = dir.join("ailleurs.md");
    let got = local_target("/srv/rapport.md", Some(vise.to_string_lossy().into_owned())).unwrap();
    assert_eq!(std::path::Path::new(&got), vise);
}

/// Trouvé par l'audit du 9 septembre 2026 : dès que l'appelant IPC fournissait
/// un `local`, `local_target` le rendait tel quel, sans passer par
/// `chemin_local_libre`. Un appel direct à `sftp_download` depuis la webview
/// (dépendance front compromise, outils de développement) écrivait alors le contenu d'un
/// serveur choisi par l'attaquant par-dessus `~/.bashrc` : `download_reprise`
/// finit par un `rename()` POSIX, qui remplace la cible sans un mot. La règle
/// de SECURITY.md (« rien n'écrase un fichier existant ») vaut aussi pour un
/// chemin imposé, pas seulement pour la cible dérivée du nom distant.
#[test]
fn local_target_n_ecrase_pas_un_chemin_impose_deja_pris() {
    let _g = with_ssh_config("");
    let dir = avash::sftp::default_local_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let pris = dir.join("bashrc");
    std::fs::write(&pris, b"a moi").unwrap();

    let got = local_target("/srv/piege", Some(pris.to_string_lossy().into_owned())).unwrap();
    let got = std::path::Path::new(&got);
    assert_ne!(got, pris, "la cible ne doit pas viser le fichier existant");
    assert!(
        !got.exists(),
        "le nom choisi doit être libre : {}",
        got.display()
    );
    assert_eq!(
        std::fs::read(&pris).unwrap(),
        b"a moi",
        "le fichier existant a été écrasé"
    );
}

/// Même audit : toutes les commandes voisines qui touchent au disque local
/// exigent un chemin absolu (`diagnostic_exporter`, `dossier_partage`,
/// `rdp_ouvrir_dossier`). Un chemin relatif se résoudrait contre le répertoire
/// courant de l'application, que l'utilisateur ne voit nulle part.
#[test]
fn local_target_refuse_un_chemin_impose_relatif() {
    for l in ["ailleurs.md", "../../etc/passwd", ""] {
        assert!(
            local_target("/srv/rapport.md", Some(l.to_owned())).is_err(),
            "{l} devrait être refusé"
        );
    }
}

#[test]
fn local_target_derive_le_nom_du_fichier_distant() {
    // `with_ssh_config` pose `AVASH_HOME` sur un dossier vierge : la cible par
    // défaut n'existe pas, on garde donc le nom tel quel (déterministe, sans
    // dépendre du vrai dossier Téléchargements du poste).
    let _g = with_ssh_config("");
    let got = local_target("/srv/data/rapport.md", None).unwrap();
    assert!(
        got.ends_with("rapport.md"),
        "le nom distant doit etre conserve : {got}"
    );
}

#[test]
fn local_target_ne_garde_que_le_dernier_segment() {
    // Un remote contenant ../ ne doit pas remonter dans l'arborescence locale.
    let _g = with_ssh_config("");
    let got = local_target("/srv/../../etc/passwd", None).unwrap();
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

    let got = local_target("/srv/backup.sql", None).unwrap();
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

    let got = local_target("/srv/logs", None).unwrap();
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
        identifiant_encore_utilise(&hotes, "web-via-bastion", &id),
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
        !identifiant_encore_utilise(&hotes, "web", &id),
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
        !identifiant_encore_utilise(&hotes, "web", &id),
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
        identifiant_encore_utilise(&hotes, "web", &id),
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
        identifiant_encore_utilise(&hotes, "explicite", &id),
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
        .manage(TransfertsStore::default())
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
        executer: std::sync::Arc::new(|_, _| {
            Box::pin(async { Err("pas de transport dans ce test".to_owned()) })
        }),
        label: "h".into(),
        cible: ("h".into(), 22, "u".into()),
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

/// Un écrivain qui laisse passer l'en-tête (première écriture) puis refuse
/// tout : il rejoue un disque qui se remplit dès la première sortie, sans
/// dépendre de `/dev/full` (absent sous Windows). Compter les écritures plutôt
/// que les octets rend le test indépendant de la longueur exacte des lignes.
struct PleinApresEntete {
    ecritures: usize,
}

impl std::io::Write for PleinApresEntete {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.ecritures += 1;
        if self.ecritures <= 1 {
            Ok(buf.len()) // l'en-tête
        } else {
            Err(std::io::Error::other("No space left on device"))
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
    // Une écriture refusée condamne l'enregistreur : la fermeture doit passer
    // par la branche qui signale l'échec, sans planter.
    assert!(enr.sortie("x").is_err());
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

// ---------- Copie directe (scp chez la source) : annulation ----------
//
// Trouvé par l'audit du 7 septembre 2026 : la branche `direct` de
// `sftp_copier_vers` n'inscrivait aucun drapeau d'annulation. `sftp_annuler`
// rendait donc `false` et le bouton « Annuler » du front, affiché sur toute
// ligne en cours, ne coupait rien, sans le dire.

/// Une poignée de session dont l'`executer` mime `run_avec_agent` : il tourne
/// jusqu'à ce que l'interface lève le drapeau d'annulation, puis rend l'erreur
/// `ANNULE` que le front reconnaît. C'est la source d'une copie directe.
fn poignee_scp_annulable(epoch: u64) -> SessionHandle {
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
