// Journal des commandes : masquage des secrets, écriture 0600, relecture,
// codes de retour et purge. Fonctionnalité demandée par le mainteneur le
// 24 novembre 2026 (« avoir un log des commandes passées en ssh »).

use super::*;

/// Un répertoire temporaire propre à chaque test, effacé à la fin.
struct Bac(PathBuf);

impl Bac {
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Bac {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn bac() -> Bac {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("avash-journal-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("répertoire temporaire");
    Bac(d)
}

// ---------- Masquage ----------

/// La valeur factice des secrets de ces tests, posée au moment du test à la
/// place de `{F}` : écrites en clair (mot de passe collé à `-p`, `--password=`), les
/// commandes faisaient rougir les détecteurs de secrets de la chaîne, qui ne
/// savent pas qu'elles sont factices (PR #113).
const FAUX: &str = "factice";

fn avec(modele: &str) -> String {
    modele.replace("{F}", FAUX)
}

/// `utilisateur:motdepasse`, assemblé au moment du test : écrite en clair, la
/// paire fait rougir les détecteurs de secrets de la chaîne (gitleaks,
/// `GitGuardian`), qui ne savent pas qu'elle est factice (PR #113).
fn paire(utilisateur: &str, mot_de_passe: &str) -> String {
    format!("{utilisateur}:{mot_de_passe}")
}

#[test]
fn une_commande_ordinaire_passe_telle_quelle() {
    for c in [
        "ls -la /etc",
        "mkdir -p /srv/www",
        "ssh -p 2222 admin@hote",
        "git commit --author=\"Ada <ada@exemple.org>\" -m 'fix'",
        "cd $PWD/..",
        "grep -r password_hash src/",
        "docker login --password-stdin -u ada registre.exemple.org",
        "systemctl restart nginx && journalctl -u nginx -n 50",
    ] {
        assert_eq!(masquer_secrets(c), c, "{c}");
    }
}

#[test]
fn les_variables_de_secret_sont_masquees() {
    assert_eq!(
        masquer_secrets(&avec("export GITHUB_TOKEN={F}")),
        "export GITHUB_TOKEN=••••"
    );
    assert_eq!(
        masquer_secrets(&avec("PGPASSWORD='{F} {F}' psql -h db")),
        "PGPASSWORD='••••' psql -h db"
    );
    assert_eq!(
        masquer_secrets(&avec("env AWS_SECRET_ACCESS_KEY={F} aws s3 ls")),
        "env AWS_SECRET_ACCESS_KEY=•••• aws s3 ls"
    );
}

#[test]
fn les_options_de_secret_sont_masquees() {
    assert_eq!(
        masquer_secrets(&avec("restic --password={F} snapshots")),
        "restic --password=•••• snapshots"
    );
    assert_eq!(
        masquer_secrets(&avec("vault login --token {F}")),
        "vault login --token ••••"
    );
    assert_eq!(
        masquer_secrets(&avec("tool --api-key \"{F} {F}\" run")),
        "tool --api-key \"••••\" run"
    );
}

#[test]
fn le_mot_de_passe_colle_d_un_client_mysql_est_masque() {
    assert_eq!(
        masquer_secrets(&avec("mysql -u root -p{F} base")),
        "mysql -u root -p•••• base"
    );
    assert_eq!(
        masquer_secrets(&avec(
            "sudo -u postgres /usr/bin/mysqldump -p{F} --all-databases"
        )),
        "sudo -u postgres /usr/bin/mysqldump -p•••• --all-databases"
    );
    // `-p` seul demande le mot de passe à l'invite : rien à cacher.
    assert_eq!(masquer_secrets("mysql -u root -p"), "mysql -u root -p");
    // Hors MySQL, `-p` n'est pas un mot de passe.
    assert_eq!(masquer_secrets("mkdir -pv a/b"), "mkdir -pv a/b");
}

#[test]
fn sshpass_et_curl_perdent_leur_secret_pas_leur_utilisateur() {
    assert_eq!(
        masquer_secrets(&avec("sshpass -p {F} ssh hote")),
        "sshpass -p •••• ssh hote"
    );
    assert_eq!(
        masquer_secrets(&format!(
            "curl -u {} https://exemple.org",
            paire("ada", FAUX)
        )),
        format!("curl -u {} https://exemple.org", paire("ada", MASQUE))
    );
    assert_eq!(
        masquer_secrets(&format!(
            "curl --user={} https://exemple.org",
            paire("ada", FAUX)
        )),
        format!("curl --user={} https://exemple.org", paire("ada", MASQUE))
    );
}

#[test]
fn les_formes_collees_de_sshpass_et_curl_sont_masquees() {
    // Trouvé par la revue de sécurité automatique de la PR #113 : `-pSECRET`
    // et `-uutilisateur:motdepasse` collés passaient en clair.
    assert_eq!(
        masquer_secrets(&avec("sshpass -p{F} ssh hote")),
        format!("sshpass -p{MASQUE} ssh hote")
    );
    assert_eq!(
        masquer_secrets(&format!(
            "curl -u{} https://exemple.org",
            paire("ada", FAUX)
        )),
        format!("curl -u{} https://exemple.org", paire("ada", MASQUE))
    );
    // `-uada` seul demande le mot de passe : rien à cacher.
    assert_eq!(
        masquer_secrets("curl -uada https://exemple.org"),
        "curl -uada https://exemple.org"
    );
}

#[test]
fn les_options_groupees_et_le_mandataire_de_curl_sont_masques() {
    // Revue de sécurité automatique de la PR #113 : `-su u:p`, `-U` et
    // `--proxy-user` passaient en clair, comme `-vpSECRET` d'un client MySQL.
    let avec_paire = |modele: &str, mdp: &str| modele.replace("{P}", &paire("ada", mdp));
    for modele in [
        "curl -su {P} https://h",
        "curl -sku{P} https://h",
        "curl -U {P} -x http://mandataire https://h",
        "curl --proxy-user {P} https://h",
        "curl --proxy-user={P} https://h",
    ] {
        assert_eq!(
            masquer_secrets(&avec_paire(modele, FAUX)),
            avec_paire(modele, MASQUE),
            "{modele}"
        );
    }
    assert_eq!(
        masquer_secrets(&avec("mysql -vp{F} base")),
        format!("mysql -vp{MASQUE} base")
    );
    assert_eq!(
        masquer_secrets(&avec("sshpass -vp{F} ssh h")),
        format!("sshpass -vp{MASQUE} ssh h")
    );
    // Ce qui suit une option à valeur est sa valeur, pas un mot de passe.
    assert_eq!(
        masquer_secrets("curl -ou.txt https://h"),
        "curl -ou.txt https://h"
    );
    assert_eq!(masquer_secrets("mysql -hpgsql base"), "mysql -hpgsql base");
}

#[test]
fn les_entetes_d_authentification_sont_masques() {
    assert_eq!(
        masquer_secrets(&avec("curl -H 'Authorization: Bearer {F}' https://api")),
        "curl -H 'Authorization: Bearer ••••' https://api"
    );
    assert_eq!(
        masquer_secrets(&avec("curl -H \"PRIVATE-TOKEN: {F}\" https://gitlab")),
        "curl -H \"PRIVATE-TOKEN: ••••\" https://gitlab"
    );
}

#[test]
fn les_identifiants_d_une_url_sont_masques() {
    assert_eq!(
        masquer_secrets(&format!(
            "git clone https://{}@github.com/a/b.git",
            paire("ada", "jeton")
        )),
        format!(
            "git clone https://{}@github.com/a/b.git",
            paire("ada", MASQUE)
        )
    );
    assert_eq!(
        masquer_secrets(&format!(
            "psql postgresql://{}@db:5432/app",
            paire("app", FAUX)
        )),
        format!("psql postgresql://{}@db:5432/app", paire("app", MASQUE))
    );
    // Sans mot de passe dans l'URL, rien ne change.
    assert_eq!(
        masquer_secrets("git clone ssh://git@hote/depot.git"),
        "git clone ssh://git@hote/depot.git"
    );
}

#[test]
fn le_masquage_vaut_dans_chaque_segment_d_un_enchainement() {
    assert_eq!(
        masquer_secrets(&avec(
            "cd /tmp && mysql -p{F} base | sshpass -p {F} ssh h; ls -p"
        )),
        "cd /tmp && mysql -p•••• base | sshpass -p •••• ssh h; ls -p"
    );
}

// ---------- Préparation ----------

#[test]
fn une_commande_qui_commence_par_une_espace_n_est_pas_notee() {
    // Comme HISTCONTROL=ignorespace : le moyen de passer une commande sans trace.
    assert_eq!(preparer(" ls"), None);
    assert_eq!(preparer("\tls"), None);
    assert_eq!(preparer(""), None);
    assert_eq!(preparer("   "), None);
}

#[test]
fn les_caracteres_de_controle_sont_retires() {
    assert_eq!(
        preparer("echo \u{1b}[31mrouge\u{7}\ttab   ").as_deref(),
        Some("echo [31mrouge tab")
    );
}

#[test]
fn une_commande_trop_longue_est_tronquee() {
    let longue = "x".repeat(COMMANDE_MAX + 50);
    let notee = preparer(&longue).unwrap();
    assert_eq!(notee.chars().count(), COMMANDE_MAX + 1);
    assert!(notee.ends_with('…'));
}

// ---------- Fichiers ----------

#[test]
fn le_fichier_du_jour_porte_la_date_universelle() {
    // 2026-11-24 23:59:59 UTC puis la seconde suivante.
    assert_eq!(nom_du_jour(1_795_564_799), "2026-11-24.jsonl");
    assert_eq!(nom_du_jour(1_795_564_800), "2026-11-25.jsonl");
    assert_eq!(
        jour_du_nom("2026-11-25.jsonl"),
        Some(1_795_564_800 / 86_400)
    );
    assert_eq!(jour_du_nom("1970-01-01.jsonl"), Some(0));
    assert_eq!(jour_du_nom("2024-02-29.jsonl"), Some(19_782));
    for faux in [
        "notes.txt",
        "2026-11-24.json",
        "2026-13-01.jsonl",
        "26-11-24.jsonl",
        "2026-1a-01.jsonl",
    ] {
        assert_eq!(jour_du_nom(faux), None, "{faux}");
    }
}

#[test]
fn une_commande_notee_se_relit_avec_sa_session_et_son_code() {
    let d = bac();
    let t = 1_795_500_000;
    assert!(noter_dans(d.path(), t, 7, "prod-web", "ada@10.0.0.5:22", "uptime").unwrap());
    assert!(noter_dans(d.path(), t + 1, 8, "prod-db", "ada@10.0.0.6:22", "df -h").unwrap());
    noter_code_dans(d.path(), t + 2, 7, 0).unwrap();
    assert!(noter_dans(d.path(), t + 3, 7, "prod-web", "ada@10.0.0.5:22", "false").unwrap());
    noter_code_dans(d.path(), t + 4, 7, 1).unwrap();
    let lues = lire_dans(d.path());
    assert_eq!(lues.len(), 3);
    // La plus récente d'abord.
    assert_eq!(lues[0].commande, "false");
    assert_eq!(lues[0].code, Some(1));
    assert_eq!(lues[1].commande, "df -h");
    assert_eq!(
        lues[1].code, None,
        "le code de la session 7 ne va pas à la 8"
    );
    assert_eq!(lues[2].commande, "uptime");
    assert_eq!(lues[2].code, Some(0));
    assert_eq!(lues[2].session, "prod-web");
    assert_eq!(lues[2].cible, "ada@10.0.0.5:22");
}

#[test]
fn le_journal_note_la_commande_masquee() {
    let d = bac();
    noter_dans(
        d.path(),
        1_795_500_000,
        1,
        "h",
        "a@h:22",
        &avec("mysql -p{F}"),
    )
    .unwrap();
    let texte = std::fs::read_to_string(d.path().join("2026-11-24.jsonl")).unwrap();
    assert!(!texte.contains(FAUX), "{texte}");
    assert_eq!(lire_dans(d.path())[0].commande, "mysql -p••••");
}

#[test]
fn rien_n_est_ecrit_pour_une_commande_sans_trace() {
    let d = bac();
    assert!(!noter_dans(d.path(), 1_795_500_000, 1, "h", "a@h:22", " secret").unwrap());
    assert_eq!(lire_dans(d.path()), Vec::<Entree>::new());
}

#[cfg(unix)]
#[test]
fn le_journal_ne_regarde_que_son_proprietaire() {
    use std::os::unix::fs::PermissionsExt as _;
    let d = bac();
    let dir = d.path().join("journal");
    noter_dans(&dir, 1_795_500_000, 1, "h", "a@h:22", "ls").unwrap();
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&dir), 0o700);
    let fichier = dir.join("2026-11-24.jsonl");
    assert_eq!(mode(&fichier), 0o600);
    // Un fichier du jour retrouvé plus ouvert est refermé à la prochaine ligne.
    std::fs::set_permissions(&fichier, std::fs::Permissions::from_mode(0o644)).unwrap();
    noter_dans(&dir, 1_795_500_001, 1, "h", "a@h:22", "pwd").unwrap();
    assert_eq!(mode(&fichier), 0o600);
}

#[test]
fn une_ligne_abimee_est_sautee_pas_fatale() {
    let d = bac();
    noter_dans(d.path(), 1_795_500_000, 1, "h", "a@h:22", "un").unwrap();
    let f = d.path().join("2026-11-24.jsonl");
    let mut texte = std::fs::read_to_string(&f).unwrap();
    texte.push_str("{\"t\":1795500001,\"s\":1,\"comm"); // coupure en pleine ligne
    texte.push('\n');
    std::fs::write(&f, texte).unwrap();
    noter_dans(d.path(), 1_795_500_002, 1, "h", "a@h:22", "deux").unwrap();
    let lues: Vec<String> = lire_dans(d.path())
        .into_iter()
        .map(|e| e.commande)
        .collect();
    assert_eq!(lues, ["deux", "un"]);
}

#[test]
fn la_purge_efface_les_jours_trop_vieux_et_rien_d_autre() {
    let d = bac();
    let jour = 86_400;
    let maintenant = 1_795_500_000; // 2026-11-24
    for age in [0, 1, 89, 90, 91, 400] {
        noter_dans(d.path(), maintenant - age * jour, 1, "h", "a@h:22", "ls").unwrap();
    }
    std::fs::write(d.path().join("notes.txt"), "à moi").unwrap();
    let effaces = purger_dans(d.path(), 90, maintenant).unwrap();
    assert_eq!(effaces, 3, "90, 91 et 400 jours");
    assert_eq!(lire_dans(d.path()).len(), 3);
    assert!(
        d.path().join("notes.txt").exists(),
        "un fichier posé à la main reste"
    );
    // Une durée nulle vaut un jour : le jour en cours n'est jamais effacé.
    purger_dans(d.path(), 0, maintenant).unwrap();
    assert_eq!(lire_dans(d.path()).len(), 1);
}

#[test]
fn l_export_donne_l_heure_locale_dans_l_ordre_chronologique() {
    let entrees = vec![
        Entree {
            t: 1_795_500_060,
            s: 1,
            session: "web".into(),
            cible: "a@w:22".into(),
            commande: "false".into(),
            code: Some(1),
        },
        Entree {
            t: 1_795_500_000,
            s: 1,
            session: "web".into(),
            cible: "a@w:22".into(),
            commande: "ls".into(),
            code: None,
        },
    ];
    // 1_795_500_000 = 2026-11-24 06:00:00 UTC.
    assert_eq!(
        en_texte(&entrees, 60),
        "2026-11-24 07:00:00+01:00  web (a@w:22)  ls\n\
         2026-11-24 07:01:00+01:00  web (a@w:22) [1]  false\n"
    );
    assert!(en_texte(&entrees, -300).starts_with("2026-11-24 01:00:00-05:00"));
}
