use super::*;

/// Le commentaire de dossier se pose juste après la dernière directive du
/// bloc, avant ses lignes vides de fin (mutants survivants : `!` de la
/// recherche de la dernière ligne pleine, et `i + 1` devenu `i * 1`).
#[test]
fn set_host_folder_se_pose_apres_la_derniere_directive() {
    let dir = std::env::temp_dir().join(format!("avash-sf2-{}", rand::random::<u64>()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config");
    std::fs::write(
        &path,
        "Host prod\n    HostName 10.0.0.1\n\n\nHost autre\n    HostName 10.0.0.2\n",
    )
    .unwrap();
    set_host_folder_at(&path, "prod", "x").unwrap();
    let t = std::fs::read_to_string(&path).unwrap();
    assert!(
        t.contains("    HostName 10.0.0.1\n    #Folder: x\n\n"),
        "commentaire mal placé : {t:?}"
    );
    assert!(t.starts_with("Host prod\n    HostName 10.0.0.1\n"), "{t:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn set_host_folder_preserve_les_autres_directives() {
    let dir = std::env::temp_dir().join(format!("avash-sf-{}", rand::random::<u64>()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config");
    std::fs::write(
        &path,
        "Host prod
HostName 10.0.0.1
ForwardAgent yes

Host autre
HostName 10.0.0.2
",
    )
    .unwrap();
    // Ranger « prod » dans prod/web : la directive custom reste, le folder est posé.
    set_host_folder_at(&path, "prod", "prod/web").unwrap();
    let t = std::fs::read_to_string(&path).unwrap();
    assert!(t.contains("ForwardAgent yes"), "directive perdue : {t}");
    assert!(t.contains("#Folder: prod/web"), "folder absent : {t}");
    // Le bloc « autre » n'est pas touché.
    assert!(
        !t.contains(
            "Host autre
HostName 10.0.0.2
#Folder"
        ),
        "{t}"
    );
    // Re-déplacer remplace (pas de doublon), et vider retire la ligne.
    set_host_folder_at(&path, "prod", "").unwrap();
    let t2 = std::fs::read_to_string(&path).unwrap();
    assert!(!t2.contains("#Folder"), "folder non retiré : {t2}");
    assert!(t2.contains("ForwardAgent yes"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn set_host_folder_refuse_une_injection_par_saut_de_ligne() {
    let dir = std::env::temp_dir().join(format!("avash-inj-{}", rand::random::<u64>()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config");
    std::fs::write(&path, "Host prod\n    HostName 10.0.0.1\n").unwrap();
    // Un dossier contenant un saut de ligne tenterait d'injecter une directive.
    let r = set_host_folder_at(&path, "prod", "web\n    ProxyCommand nc evil 22");
    assert!(r.is_err(), "l'injection aurait dû être refusée");
    let t = std::fs::read_to_string(&path).unwrap();
    assert!(!t.contains("ProxyCommand"), "directive injectée : {t}");
    let _ = std::fs::remove_dir_all(&dir);
}

fn host(alias: &str) -> SshHost {
    SshHost {
        alias: alias.into(),
        hostname: Some("10.0.0.7".into()),
        user: Some("adrien".into()),
        port: Some(2222),
        identity_file: Some("/home/a/.ssh/id_ed25519".into()),
        ..Default::default()
    }
}

#[test]
fn le_bloc_rendu_est_relu_a_l_identique() {
    // Boucle complete : ce qu'on ecrit doit etre relisible par le parseur.
    let h = host("prod");
    let relu = parse_config_str(&render_host_block(&h));
    assert_eq!(relu.len(), 1);
    assert_eq!(relu[0].alias, "prod");
    assert_eq!(relu[0].hostname.as_deref(), Some("10.0.0.7"));
    assert_eq!(relu[0].user.as_deref(), Some("adrien"));
    assert_eq!(relu[0].port, Some(2222));
    assert_eq!(
        relu[0].identity_file.as_deref(),
        Some("/home/a/.ssh/id_ed25519")
    );
}

#[test]
fn le_port_par_defaut_n_est_pas_ecrit() {
    // Ecrire « Port 22 » partout alourdit le fichier pour rien.
    let mut h = host("simple");
    h.port = Some(22);
    assert!(
        !render_host_block(&h).contains("Port"),
        "{}",
        render_host_block(&h)
    );
}

#[test]
fn les_champs_vides_sont_omis() {
    let h = SshHost {
        alias: "minimal".into(),
        hostname: Some("  ".into()),
        user: None,
        ..Default::default()
    };
    let bloc = render_host_block(&h);
    assert_eq!(bloc.trim(), "Host minimal", "bloc : {bloc:?}");
    // Une clé ou un rebond faits d'espaces ne donnent pas de directive vide
    // (mutant survivant : le filtre `!v.trim().is_empty()` du ProxyJump).
    let h = SshHost {
        alias: "blancs".into(),
        identity_file: Some("   ".into()),
        proxy_jump: Some(" \t".into()),
        ..Default::default()
    };
    let bloc = render_host_block(&h);
    assert_eq!(bloc.trim(), "Host blancs", "bloc : {bloc:?}");
}

#[test]
fn un_alias_avec_saut_de_ligne_est_refuse() {
    // Sans ce garde-fou on injecte n'importe quelle directive dans la
    // configuration SSH — ProxyCommand comprise.
    for mechant in [
        "prod\n    ProxyCommand nc evil.example 22",
        "prod\rHost *",
        "prod\0",
    ] {
        assert!(
            validate_alias(mechant).is_err(),
            "devrait etre refuse : {mechant:?}"
        );
    }
}

#[test]
fn append_host_refuse_une_injection_de_directive_dans_les_champs() {
    // Regression securite : un saut de ligne dans HostName/User/
    // IdentityFile injecterait une directive arbitraire (ex. ProxyCommand,
    // execute par ssh a la connexion). Seul l'alias etait protege.
    let _g = crate::testutil::temp_home();
    for bad in [
        SshHost {
            alias: "srv".into(),
            hostname: Some("1.2.3.4\n    ProxyCommand evil".into()),
            ..Default::default()
        },
        SshHost {
            alias: "srv".into(),
            user: Some("root\nProxyCommand evil".into()),
            ..Default::default()
        },
        SshHost {
            alias: "srv".into(),
            identity_file: Some("/k\r  ProxyCommand evil".into()),
            ..Default::default()
        },
    ] {
        assert!(append_host(&bad).is_err(), "doit refuser : {bad:?}");
    }
    // Un hote propre passe toujours.
    assert!(append_host(&SshHost {
        alias: "ok".into(),
        hostname: Some("10.0.0.1".into()),
        ..Default::default()
    })
    .is_ok());
}

#[test]
fn un_alias_joker_est_refuse() {
    // « Host * » s'appliquerait a TOUTES les connexions de la machine.
    for mechant in ["*", "prod*", "?", "!prod"] {
        assert!(
            validate_alias(mechant).is_err(),
            "devrait etre refuse : {mechant}"
        );
    }
}

#[test]
fn un_alias_avec_espace_ou_vide_est_refuse() {
    assert!(validate_alias("").is_err());
    assert!(validate_alias("mon serveur").is_err());
}

#[test]
fn un_alias_normal_passe() {
    for bon in ["prod", "prod-web", "serveur_1", "10.0.0.5"] {
        assert!(validate_alias(bon).is_ok(), "devrait passer : {bon}");
    }
}
// ---------- Ecriture reelle dans ~/.ssh/config ----------

use crate::testutil::temp_home;

#[test]
fn append_host_cree_le_fichier_et_le_relit() {
    let _h = temp_home();
    append_host(&host("neuf")).unwrap();
    let relu = parse_ssh_config().unwrap();
    assert_eq!(relu.len(), 1);
    assert_eq!(relu[0].alias, "neuf");
}

#[test]
fn append_host_enregistre_un_rebond_a_plusieurs_sauts() {
    // Trouvé par l'audit du 9 septembre 2026 : le scénario complet, celui
    // que l'interface propose dans son propre exemple de champ ProxyJump.
    // L'enregistrement échouait avant même d'écrire quoi que ce soit.
    let _h = temp_home();
    let mut h = host("prod");
    h.proxy_jump = Some("bastion, relais:2200".into());
    append_host(&h).unwrap();
    let relu = parse_ssh_config().unwrap();
    assert_eq!(
        relu[0].proxy_jump.as_deref(),
        Some("bastion, relais:2200"),
        "le rebond doit se relire tel quel"
    );
}

#[test]
fn append_host_preserve_le_contenu_existant() {
    // Le risque majeur : abimer une configuration que l'utilisateur a
    // ecrite a la main, commentaires compris.
    let _h = temp_home();
    let path = ssh_config_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let avant = "# Ma config perso\nHost ancien\n    HostName 1.2.3.4\n";
    std::fs::write(&path, avant).unwrap();

    append_host(&host("nouveau")).unwrap();

    let apres = std::fs::read_to_string(&path).unwrap();
    assert!(
        apres.starts_with(avant),
        "le contenu d'origine doit rester intact :\n{apres}"
    );
    assert!(apres.contains("# Ma config perso"), "commentaire perdu");

    let relu = parse_ssh_config().unwrap();
    let noms: Vec<_> = relu.iter().map(|h| h.alias.as_str()).collect();
    assert_eq!(noms, vec!["ancien", "nouveau"]);
}

#[test]
fn append_host_separe_les_blocs_par_une_ligne_vide() {
    // Sans separation, `Host` se colle a la directive precedente et en
    // devient une sous-directive : le nouvel hote serait invisible.
    let _h = temp_home();
    let path = ssh_config_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "Host a\n    HostName 1.1.1.1").unwrap(); // sans \n final
    append_host(&host("b")).unwrap();
    assert_eq!(parse_ssh_config().unwrap().len(), 2);
}

/// Un alias déclaré dans un fichier inclus doit être refusé lui aussi.
///
/// Sans cela on ajoutait un second bloc pour le même alias : OpenSSH retenant
/// la première occurrence, l'hôte semblait ne plus répondre aux
/// modifications, et la liste affichait deux entrées identiques.
/// La ligne vide avant un nouveau bloc n'est mise que s'il en faut une :
/// aucune sur un fichier vide, une seule après un bloc, aucune de plus
/// après une ligne vide déjà là. Mutants survivants : `&&` devenu `||`
/// (ligne vide en tête d'un fichier vide) et le `!` de `ends_with('\n')`
/// (un bloc collé au précédent, ou deux lignes vides).
#[test]
fn append_host_ne_met_de_ligne_vide_que_s_il_en_faut() {
    let _h = temp_home();
    let path = ssh_config_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    for (avant, attendu) in [
        ("", "Host b\n"),
        (
            "Host a\n    HostName 1\n",
            "Host a\n    HostName 1\n\nHost b\n",
        ),
        (
            "Host a\n    HostName 1",
            "Host a\n    HostName 1\n\nHost b\n",
        ),
        (
            "Host a\n    HostName 1\n\n",
            "Host a\n    HostName 1\n\nHost b\n",
        ),
    ] {
        std::fs::write(&path, avant).unwrap();
        append_host(&host("b")).unwrap();
        let apres = std::fs::read_to_string(&path).unwrap();
        assert!(
            apres.starts_with(attendu),
            "avant {avant:?} : attendu {attendu:?}, obtenu {apres:?}"
        );
    }
}

#[test]
fn append_host_refuse_une_config_non_lisible_au_lieu_de_l_abimer() {
    // Trouvé par l'audit du 7 septembre 2026 : un `~/.ssh/config` non UTF-8
    // (commentaire Latin-1 `# R\xe9seau` d'un vieil éditeur) et sans saut de
    // ligne final. Avec l'ancien `unwrap_or_default()`, `existing` devenait
    // vide : le contrôle d'unicité ne voyait plus l'alias `a` (doublon
    // possible) et le bloc `Host b` se soudait à `IdentityFile ~/.ssh/k`,
    // cassant la directive pour OpenSSH. `append_host` doit refuser et
    // laisser le fichier strictement intact.
    let _h = temp_home();
    let path = ssh_config_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let octets = b"# R\xe9seau\nHost a\n    IdentityFile ~/.ssh/k";
    std::fs::write(&path, octets).unwrap();
    let e = append_host(&host("b")).unwrap_err().to_string();
    assert!(e.contains("Lecture de"), "message inattendu : {e}");
    assert_eq!(
        std::fs::read(&path).unwrap(),
        octets,
        "le fichier illisible a été modifié"
    );
}

#[test]
fn append_host_cree_le_fichier_absent_sans_erreur() {
    // Garde-fou du correctif ci-dessus : seul `NotFound` vaut `""`. Sur un
    // fichier absent (cas normal du premier hôte), l'écriture doit réussir.
    let _h = temp_home();
    let path = ssh_config_path();
    assert!(!path.exists());
    append_host(&host("premier")).unwrap();
    assert_eq!(parse_ssh_config().unwrap().len(), 1);
}

#[test]
fn append_host_voit_les_alias_declares_dans_un_include() {
    let _h = temp_home();
    let ssh = repertoire_personnel().unwrap().join(".ssh");
    std::fs::create_dir_all(ssh.join("config.d")).unwrap();
    std::fs::write(
        ssh.join("config.d").join("10-prod"),
        "Host venu-d-un-include\n    HostName 10.0.0.9\n",
    )
    .unwrap();
    std::fs::write(ssh.join("config"), "Include config.d/*\n").unwrap();

    let e = append_host(&host("venu-d-un-include"))
        .unwrap_err()
        .to_string();
    assert!(e.contains("déjà déclaré"), "{e}");
}

#[test]
fn append_host_refuse_un_alias_deja_present() {
    let _h = temp_home();
    append_host(&host("double")).unwrap();
    let e = append_host(&host("double")).unwrap_err().to_string();
    assert!(e.contains("déjà déclaré"), "{e}");
    // Insensible a la casse : OpenSSH l'est aussi.
    let mut autre = host("DOUBLE");
    autre.alias = "DOUBLE".into();
    assert!(append_host(&autre).is_err());
}

#[cfg(unix)]
#[test]
fn append_host_pose_les_droits_attendus() {
    use std::os::unix::fs::PermissionsExt;
    let _h = temp_home();
    append_host(&host("droits")).unwrap();
    let path = ssh_config_path();
    let m = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(m, 0o600, "config SSH lisible par d'autres");
    let d = std::fs::metadata(path.parent().unwrap())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(d, 0o700, "~/.ssh trop ouvert");
}

// ---------- Match ----------

#[test]
fn un_bloc_match_ne_contamine_pas_l_hote_precedent() {
    // Regression : `Match` n'etait pas reconnu comme delimiteur, donc ses
    // directives etaient appliquees au dernier Host. Un `Match exec`
    // jamais satisfait pouvait ainsi changer l'utilisateur et le port
    // d'un hote reel — sans le moindre avertissement.
    let cfg = "Host prod\n  HostName 10.0.0.1\n  User root\n\n                   Match exec \"test -f /tmp/jamais\"\n  User compromis\n  Port 9999\n";
    let hosts = parse_config_str(cfg);
    assert_eq!(hosts.len(), 1, "seul `prod` est un hote : {hosts:?}");
    assert_eq!(
        hosts[0].user.as_deref(),
        Some("root"),
        "utilisateur contamine"
    );
    assert_eq!(hosts[0].port, None, "port contamine");
}

#[test]
fn un_host_apres_un_match_est_bien_lu() {
    let cfg = "Match user root\n  ForwardAgent yes\n\nHost apres\n  HostName 1.2.3.4\n";
    let hosts = parse_config_str(cfg);
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0].alias, "apres");
    assert_eq!(hosts[0].hostname.as_deref(), Some("1.2.3.4"));
}

// ---------- Include ----------

#[test]
fn include_absolu_est_resolu() {
    let _h = crate::testutil::temp_home();
    let dir = ssh_config_path().parent().unwrap().to_path_buf();
    std::fs::create_dir_all(&dir).unwrap();
    let inc = dir.join("perso");
    std::fs::write(&inc, "Host inclus\n  HostName 5.5.5.5\n").unwrap();
    std::fs::write(
        ssh_config_path(),
        format!(
            "Host principal\n  HostName 1.1.1.1\n\nInclude {}\n",
            inc.display()
        ),
    )
    .unwrap();

    let noms: Vec<_> = parse_ssh_config()
        .unwrap()
        .into_iter()
        .map(|h| h.alias)
        .collect();
    assert!(noms.contains(&"principal".to_string()), "{noms:?}");
    assert!(
        noms.contains(&"inclus".to_string()),
        "l'hote inclus doit apparaitre : {noms:?}"
    );
}

#[test]
fn include_relatif_part_de_ssh() {
    // OpenSSH resout les chemins relatifs depuis ~/.ssh.
    let _h = crate::testutil::temp_home();
    let dir = ssh_config_path().parent().unwrap().to_path_buf();
    std::fs::create_dir_all(dir.join("config.d")).unwrap();
    std::fs::write(
        dir.join("config.d/dix"),
        "Host relatif\n  HostName 9.9.9.9\n",
    )
    .unwrap();
    std::fs::write(ssh_config_path(), "Include config.d/dix\n").unwrap();

    let noms: Vec<_> = parse_ssh_config()
        .unwrap()
        .into_iter()
        .map(|h| h.alias)
        .collect();
    assert_eq!(noms, vec!["relatif"], "{noms:?}");
}

#[test]
fn include_avec_motif_prend_tous_les_fichiers_en_ordre() {
    let _h = crate::testutil::temp_home();
    let dir = ssh_config_path().parent().unwrap().to_path_buf();
    std::fs::create_dir_all(dir.join("config.d")).unwrap();
    std::fs::write(dir.join("config.d/10-a"), "Host aaa\n  HostName 1.1.1.1\n").unwrap();
    std::fs::write(dir.join("config.d/20-b"), "Host bbb\n  HostName 2.2.2.2\n").unwrap();
    std::fs::write(ssh_config_path(), "Include config.d/*\n").unwrap();

    let noms: Vec<_> = parse_ssh_config()
        .unwrap()
        .into_iter()
        .map(|h| h.alias)
        .collect();
    assert_eq!(
        noms,
        vec!["aaa", "bbb"],
        "ordre lexicographique attendu : {noms:?}"
    );
}

#[test]
fn include_manquant_est_ignore_sans_planter() {
    // OpenSSH tolere un Include qui ne correspond a rien ; une config
    // partielle vaut mieux qu'aucune.
    let _h = crate::testutil::temp_home();
    let dir = ssh_config_path().parent().unwrap().to_path_buf();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        ssh_config_path(),
        "Include /rien/du/tout\nHost seul\n  HostName 1.1.1.1\n",
    )
    .unwrap();
    let noms: Vec<_> = parse_ssh_config()
        .unwrap()
        .into_iter()
        .map(|h| h.alias)
        .collect();
    assert_eq!(noms, vec!["seul"], "{noms:?}");
}

#[test]
fn include_circulaire_ne_boucle_pas() {
    // Deux fichiers qui s'incluent mutuellement : borne a 16 niveaux,
    // comme OpenSSH. Sans borne, le parseur ne rendrait jamais la main.
    let _h = crate::testutil::temp_home();
    let dir = ssh_config_path().parent().unwrap().to_path_buf();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("boucle"),
        "Include config\nHost cycle\n  HostName 3.3.3.3\n",
    )
    .unwrap();
    std::fs::write(ssh_config_path(), "Include boucle\n").unwrap();
    let hosts = parse_ssh_config().unwrap();
    assert!(hosts.iter().any(|h| h.alias == "cycle"), "{hosts:?}");
}

#[test]
fn glob_match_gere_etoile_et_point_interrogation() {
    assert!(glob_match("*", "quoi-que-ce-soit"));
    assert!(glob_match("10-*", "10-web"));
    assert!(glob_match("*.conf", "prod.conf"));
    assert!(glob_match("config?", "config1"));
    assert!(!glob_match("config?", "config12"));
    assert!(!glob_match("10-*", "20-web"));
    assert!(glob_match("a*b*c", "axxbyyc"));
    // Une étoile en fin de nom : rien à consommer, et rien ne déborde
    // (mutant survivant : `&&` devenu `||` faisait indexer un nom vide).
    assert!(glob_match("a*", "a"));
    assert!(!glob_match("a*b", "a"));
}

/// Trouvé par l'audit du 7 septembre 2026 : `glob_match` faisait le même
/// retour arrière exponentiel que les moteurs de motif naïfs. Un motif
/// d'`Include` à plusieurs étoiles (`conf.d/*a*a*a…*b`) confronté à un long
/// nom de fichier sans correspondance (`aaaa…a` dans `~/.ssh` ou un `conf.d`)
/// faisait exploser le temps et figeait `parse_ssh_config` à chaque
/// rafraîchissement de la liste d'hôtes. Le balayage itératif à un seul point
/// de retour rend en O(n*m) : on borne ici à une seconde, alors que la
/// version récursive n'en finissait pas.
#[test]
fn glob_match_ne_part_pas_en_retour_arriere_exponentiel() {
    let motif = format!("{}b", "*a".repeat(30));
    let nom = "a".repeat(60);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(glob_match(&motif, &nom));
    });
    match rx.recv_timeout(std::time::Duration::from_secs(1)) {
        Ok(r) => assert!(!r, "le motif ne doit pas correspondre au nom"),
        Err(e) => {
            panic!("glob_match n'a pas rendu la main en une seconde ({e}) : retour arrière exponentiel")
        }
    }
}

// ---------- remove_host ----------

/// Un bloc `Match` qui suit l'hôte retiré termine le bloc à retirer et
/// reste entier ; les directives de l'hôte retiré, elles, partent toutes
/// (mutant survivant : `key == "match"` devenu `!=`, qui gardait les
/// directives dès la première ligne).
#[test]
fn remove_host_s_arrete_au_bloc_match_qui_suit() {
    let _h = crate::testutil::temp_home();
    let path = ssh_config_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "Host prod\n    HostName 1.1.1.1\n    User root\n\nMatch host x\n    User u\n\nHost b\n    HostName 2.2.2.2\n",
    )
    .unwrap();
    remove_host("prod").unwrap();
    let apres = std::fs::read_to_string(&path).unwrap();
    assert!(
        !apres.contains("1.1.1.1") && !apres.contains("User root"),
        "{apres}"
    );
    assert!(apres.contains("Match host x\n    User u"), "{apres}");
    assert!(apres.contains("Host b\n    HostName 2.2.2.2"), "{apres}");
}

#[test]
fn remove_host_supprime_le_bon_bloc_et_garde_le_reste() {
    let _h = crate::testutil::temp_home();
    let path = ssh_config_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "# entete perso\nHost prod\n  HostName 1.1.1.1\n\nHost staging\n  HostName 2.2.2.2\n",
    )
    .unwrap();

    remove_host("prod").unwrap();

    let apres = std::fs::read_to_string(&path).unwrap();
    assert!(
        apres.contains("# entete perso"),
        "commentaire perdu : {apres}"
    );
    assert!(
        !apres.contains("prod"),
        "prod aurait du disparaitre : {apres}"
    );
    assert!(apres.contains("staging"), "staging efface a tort : {apres}");
    let noms: Vec<_> = parse_ssh_config()
        .unwrap()
        .into_iter()
        .map(|h| h.alias)
        .collect();
    assert_eq!(noms, vec!["staging"]);
}

#[test]
fn remove_host_ajoute_puis_retire_revient_a_l_etat_initial() {
    let _h = crate::testutil::temp_home();
    append_host(&host("temporaire")).unwrap();
    assert_eq!(parse_ssh_config().unwrap().len(), 1);
    remove_host("temporaire").unwrap();
    assert_eq!(parse_ssh_config().unwrap().len(), 0);
}

#[test]
fn remove_host_signale_un_alias_absent() {
    let _h = crate::testutil::temp_home();
    append_host(&host("existe")).unwrap();
    let e = remove_host("absent").unwrap_err().to_string();
    assert!(e.contains("introuvable"), "{e}");
}

#[test]
fn remove_host_ne_touche_pas_un_bloc_a_alias_multiples() {
    // `Host prod backup` partage des directives : retirer « prod » ne doit
    // pas casser « backup ». On laisse le bloc entier plutot que d'abimer
    // l'autre alias.
    let _h = crate::testutil::temp_home();
    let path = ssh_config_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "Host prod backup\n  User root\n").unwrap();
    let e = remove_host("prod").unwrap_err().to_string();
    // Trouvé par l'audit du 7 septembre 2026 : le refus est délibéré, mais le
    // message disait « introuvable » alors que « prod » est bien listé. Il
    // doit maintenant dire « plusieurs alias » et nommer le bloc.
    assert!(e.contains("plusieurs alias"), "{e}");
    assert!(e.contains("Host prod backup"), "{e}");
    assert!(!e.contains("introuvable"), "{e}");
}

#[test]
fn update_host_dit_plusieurs_alias_sur_un_bloc_a_noms_multiples() {
    // Même contre-vérité qu'au glisser-déposer : éditer « prod » depuis le
    // menu contextuel donnait « introuvable » pour un bloc `Host prod backup`.
    let _h = crate::testutil::temp_home();
    let path = ssh_config_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "Host prod backup\n  User root\n").unwrap();
    let e = update_host("prod", &host("prod")).unwrap_err().to_string();
    assert!(e.contains("plusieurs alias"), "{e}");
    assert!(e.contains("Host prod backup"), "{e}");
    assert!(!e.contains("introuvable"), "{e}");
}

// Vrai si aucun `\n` du texte n'est « nu » (non précédé de `\r`) : la marque
// qu'un fichier CRLF n'a pas été partiellement converti en LF.
fn aucun_lf_nu(s: &str) -> bool {
    let b = s.as_bytes();
    b.iter()
        .enumerate()
        .all(|(i, &c)| c != b'\n' || (i > 0 && b[i - 1] == b'\r'))
}

#[test]
fn un_fichier_crlf_reste_en_crlf_apres_suppression() {
    // Trouvé par l'audit du 7 septembre 2026 : `content.lines()` retire les
    // `\r\n` et `remove_host` réémettait en `\n`, convertissant tout un
    // `~/.ssh/config` CRLF (Bloc-notes, dotfiles versionnés sous Windows) en
    // LF au premier retrait — `git diff` de toutes les lignes au lieu d'une.
    let _h = crate::testutil::temp_home();
    let path = ssh_config_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "Host a\r\n  HostName 1\r\n\r\nHost b\r\n  HostName 2\r\n",
    )
    .unwrap();
    remove_host("a").unwrap();
    let apres = std::fs::read_to_string(&path).unwrap();
    assert!(aucun_lf_nu(&apres), "LF nu introduit : {apres:?}");
    assert!(apres.contains("Host b\r\n  HostName 2\r\n"), "{apres:?}");
    assert!(!apres.contains("Host a"), "{apres:?}");
}

#[test]
fn un_fichier_crlf_reste_en_crlf_apres_edition() {
    // Même conversion silencieuse par `update_host` : éditer un hôte d'un
    // fichier CRLF ne doit toucher que son bloc, pas les fins de ligne du reste.
    let _h = crate::testutil::temp_home();
    let path = ssh_config_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "Host a\r\n  HostName 1\r\n\r\nHost b\r\n  HostName 2\r\n",
    )
    .unwrap();
    update_host("a", &host("a")).unwrap();
    let apres = std::fs::read_to_string(&path).unwrap();
    assert!(aucun_lf_nu(&apres), "LF nu introduit : {apres:?}");
    assert!(apres.contains("Host b\r\n  HostName 2\r\n"), "{apres:?}");
}

#[test]
fn un_fichier_crlf_reste_en_crlf_apres_rangement() {
    // `set_host_folder_at` ne pose qu'une ligne `#Folder:` : le reste du
    // fichier CRLF doit rester octet pour octet identique, fins de ligne
    // comprises. Chemin emprunté aussi par `folders::rename_core`.
    let _h = crate::testutil::temp_home();
    let path = ssh_config_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let origine = "Host a\r\n  HostName x\r\n";
    std::fs::write(&path, origine).unwrap();
    set_host_folder_at(&path, "a", "prod").unwrap();
    let apres = std::fs::read_to_string(&path).unwrap();
    assert!(aucun_lf_nu(&apres), "LF nu introduit : {apres:?}");
    assert!(apres.contains("    #Folder: prod\r\n"), "{apres:?}");
    // Hors la ligne #Folder, le fichier est inchangé octet pour octet.
    let mut sans_folder = String::new();
    for l in apres.lines().filter(|l| !l.contains("#Folder:")) {
        sans_folder.push_str(l);
        sans_folder.push_str("\r\n");
    }
    assert_eq!(sans_folder, origine, "contenu altéré hors #Folder");
}

#[test]
fn append_host_conserve_le_crlf_du_fichier() {
    // `append_host` ne passe pas par `lines()` mais collait `render_host_block`
    // (LF) à la fin d'un fichier CRLF : fichier mixte. Le bloc ajouté suit
    // désormais la fin de ligne du fichier.
    let _h = crate::testutil::temp_home();
    let path = ssh_config_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "Host a\r\n  HostName 1\r\n").unwrap();
    append_host(&host("b")).unwrap();
    let apres = std::fs::read_to_string(&path).unwrap();
    assert!(
        aucun_lf_nu(&apres),
        "bloc ajouté en LF dans un fichier CRLF : {apres:?}"
    );
    assert!(apres.contains("Host b\r\n"), "{apres:?}");
}

#[test]
fn set_host_folder_dit_plusieurs_alias_sur_un_bloc_a_noms_multiples() {
    // Constat de l'audit : glisser « a » de `Host a b` dans un dossier rendait
    // « Hôte « a » introuvable », alors qu'il est sous les yeux de l'utilisateur.
    let dir = std::env::temp_dir().join(format!("avash-multi-{}", rand::random::<u64>()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config");
    std::fs::write(&path, "Host a b\n  HostName x\n").unwrap();
    let e = set_host_folder_at(&path, "a", "prod")
        .unwrap_err()
        .to_string();
    assert!(e.contains("plusieurs alias"), "{e}");
    assert!(e.contains("Host a b"), "{e}");
    assert!(!e.contains("introuvable"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn set_host_folder_detecte_le_bloc_multi_alias_en_casse_mixte() {
    // `parse_config_str` accepte `HoSt` et la tabulation comme séparateur :
    // la détection du bloc à plusieurs alias doit les reconnaître aussi, sinon
    // `HoSt\ta b` retomberait sur « introuvable ».
    let dir = std::env::temp_dir().join(format!("avash-multi-cx-{}", rand::random::<u64>()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config");
    std::fs::write(&path, "HoSt\ta b\n  HostName x\n").unwrap();
    let e = set_host_folder_at(&path, "a", "prod")
        .unwrap_err()
        .to_string();
    assert!(e.contains("plusieurs alias"), "{e}");
    assert!(!e.contains("introuvable"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn set_host_folder_garde_introuvable_pour_un_vrai_absent() {
    // Aucun bloc ne cite « fantome » : le message « introuvable » reste juste.
    let dir = std::env::temp_dir().join(format!("avash-absent-{}", rand::random::<u64>()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config");
    std::fs::write(&path, "Host a b\n  HostName x\n").unwrap();
    let e = set_host_folder_at(&path, "fantome", "prod")
        .unwrap_err()
        .to_string();
    assert!(e.contains("introuvable"), "{e}");
    assert!(!e.contains("plusieurs alias"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn remove_host_garde_le_commentaire_qui_annonce_le_bloc_suivant() {
    // Trouvé par l'audit du 7 septembre 2026 : le saut du bloc retiré
    // emportait la ligne vide et le commentaire qui SUIT le bloc, alors
    // qu'ils annoncent le bloc suivant (« # Staging » avant `Host staging`).
    // La note se perdait et « # Prod » coiffait alors staging.
    let _h = crate::testutil::temp_home();
    let path = ssh_config_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "# Prod\nHost prod\n  HostName 1\n\n# Staging — accès via Jean, clé chez ops\nHost staging\n  HostName 2\n",
    )
    .unwrap();

    remove_host("prod").unwrap();

    let apres = std::fs::read_to_string(&path).unwrap();
    assert!(
        apres.contains("# Staging — accès via Jean, clé chez ops\nHost staging"),
        "la note sur staging doit rester devant son bloc :\n{apres}"
    );
    assert!(
        !apres.contains("HostName 1"),
        "prod aurait dû partir :\n{apres}"
    );
}

#[test]
fn remove_host_emporte_les_marqueurs_avash_du_bloc() {
    // Complément du cas précédent : un `#Tags:` non indenté que le parseur
    // rattache à prod (jusqu'au prochain Host/Match) doit partir AVEC prod,
    // sans que le tampon ne le prenne pour l'annonce du bloc suivant.
    let _h = crate::testutil::temp_home();
    let path = ssh_config_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "Host prod\n  HostName 1\n#Tags: x\n\n# Staging note\nHost staging\n  HostName 2\n",
    )
    .unwrap();

    remove_host("prod").unwrap();

    let apres = std::fs::read_to_string(&path).unwrap();
    assert!(
        !apres.contains("#Tags: x"),
        "le marqueur Avash aurait dû partir :\n{apres}"
    );
    assert!(
        apres.contains("# Staging note\nHost staging"),
        "l'annonce de staging doit rester :\n{apres}"
    );
}

#[test]
fn remove_host_garde_un_commentaire_de_fin_de_fichier() {
    // Bloc en fin de fichier suivi d'un commentaire : le tampon est réémis à
    // la fin, le commentaire survit à la suppression du dernier bloc.
    let _h = crate::testutil::temp_home();
    let path = ssh_config_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "Host last\n  HostName x\n\n# fin\n").unwrap();

    remove_host("last").unwrap();

    let apres = std::fs::read_to_string(&path).unwrap();
    assert!(
        apres.contains("# fin"),
        "commentaire de fin perdu :\n{apres}"
    );
    assert!(
        !apres.contains("HostName x"),
        "last aurait dû partir :\n{apres}"
    );
}

// ---------- update_host ----------

#[test]
fn update_host_remplace_le_bloc_sur_place() {
    let _h = crate::testutil::temp_home();
    let path = ssh_config_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "Host un\n  HostName 1.1.1.1\n\nHost prod\n  HostName 2.2.2.2\n  User old\n\nHost deux\n  HostName 3.3.3.3\n",
    )
    .unwrap();

    let mut modifie = host("prod");
    modifie.hostname = Some("9.9.9.9".into());
    modifie.user = Some("nouveau".into());
    update_host("prod", &modifie).unwrap();

    let hosts = parse_ssh_config().unwrap();
    // Ordre preserve : un, prod, deux.
    let noms: Vec<_> = hosts.iter().map(|h| h.alias.as_str()).collect();
    assert_eq!(noms, vec!["un", "prod", "deux"], "ordre casse");
    let p = hosts.iter().find(|h| h.alias == "prod").unwrap();
    assert_eq!(p.hostname.as_deref(), Some("9.9.9.9"));
    assert_eq!(p.user.as_deref(), Some("nouveau"));
}

#[test]
fn update_host_preserve_les_directives_non_gerees() {
    // Trouvé par l'audit du 7 septembre 2026 : éditer un hôte depuis
    // l'interface réécrivait le bloc et perdait en silence toute directive
    // qu'Avash ne gère pas (ForwardAgent, LocalForward, IdentitiesOnly…).
    let _h = crate::testutil::temp_home();
    let path = ssh_config_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "Host prod\n  HostName 2.2.2.2\n  User old\n  ForwardAgent yes\n  \
         LocalForward 8080 127.0.0.1:80\n  IdentitiesOnly yes\n  # note perso\n\nHost autre\n  HostName 3.3.3.3\n",
    )
    .unwrap();

    let mut modifie = host("prod");
    modifie.hostname = Some("9.9.9.9".into());
    modifie.user = Some("nouveau".into());
    update_host("prod", &modifie).unwrap();

    let texte = std::fs::read_to_string(&path).unwrap();
    for attendu in [
        "ForwardAgent yes",
        "LocalForward 8080 127.0.0.1:80",
        "IdentitiesOnly yes",
        "# note perso",
        "HostName 9.9.9.9",
        "User nouveau",
    ] {
        assert!(texte.contains(attendu), "« {attendu} » perdu :\n{texte}");
    }
    // L'ancienne valeur régénérée ne subsiste pas en double.
    assert!(
        !texte.contains("2.2.2.2"),
        "ancienne HostName restée :\n{texte}"
    );
    assert!(!texte.contains("User old"), "ancien User resté :\n{texte}");
    // L'hôte voisin et l'ordre sont intacts.
    let noms: Vec<_> = parse_ssh_config()
        .unwrap()
        .into_iter()
        .map(|h| h.alias)
        .collect();
    assert_eq!(noms, vec!["prod", "autre"]);
}

#[test]
fn update_host_gere_le_renommage() {
    let _h = crate::testutil::temp_home();
    append_host(&host("ancien")).unwrap();
    let mut renomme = host("ancien");
    renomme.alias = "nouveau".into();
    update_host("ancien", &renomme).unwrap();
    let noms: Vec<_> = parse_ssh_config()
        .unwrap()
        .into_iter()
        .map(|h| h.alias)
        .collect();
    assert_eq!(noms, vec!["nouveau"]);
}

#[test]
fn update_host_refuse_de_renommer_vers_un_alias_existant() {
    let _h = crate::testutil::temp_home();
    append_host(&host("a")).unwrap();
    append_host(&host("b")).unwrap();
    let mut collision = host("a");
    collision.alias = "b".into();
    let e = update_host("a", &collision).unwrap_err().to_string();
    assert!(e.contains("existe déjà"), "{e}");
}

#[test]
fn update_host_voit_les_alias_declares_dans_un_include_lors_d_un_renommage() {
    // Trouvé par l'audit du 9 septembre 2026 : `append_host` vérifiait déjà
    // l'unicité sur la configuration COMPLÈTE (Include résolus), mais
    // `update_host` n'avait jamais reçu le même traitement : il lisait le
    // fichier principal brut. Renommer « ancien » en « backup » alors qu'un
    // fichier inclus déclarait déjà « backup » passait sans erreur, et
    // OpenSSH, qui retient la PREMIÈRE occurrence, continuait de joindre la
    // machine du fichier inclus : la connexion partait vers le mauvais hôte.
    let _h = crate::testutil::temp_home();
    let ssh = repertoire_personnel().unwrap().join(".ssh");
    std::fs::create_dir_all(ssh.join("conf.d")).unwrap();
    std::fs::write(
        ssh.join("conf.d").join("prod.conf"),
        "Host backup\n    HostName 10.0.0.99\n",
    )
    .unwrap();
    std::fs::write(
        ssh.join("config"),
        "Include conf.d/*.conf\n\nHost ancien\n    HostName 10.0.0.1\n",
    )
    .unwrap();

    let mut collision = host("ancien");
    collision.alias = "backup".into();
    let e = update_host("ancien", &collision).unwrap_err().to_string();
    assert!(e.contains("existe déjà"), "{e}");
    let principal = std::fs::read_to_string(ssh.join("config")).unwrap();
    assert!(
        principal.contains("Host ancien"),
        "le bloc a été renommé malgré la collision : {principal}"
    );
    assert!(
        !principal.contains("Host backup"),
        "un second « backup » a été écrit dans le fichier principal : {principal}"
    );
}

#[test]
fn update_host_meme_alias_ne_declenche_pas_la_collision() {
    // Modifier sans renommer ne doit pas se heurter a « existe deja ».
    let _h = crate::testutil::temp_home();
    append_host(&host("stable")).unwrap();
    let mut m = host("stable");
    m.user = Some("change".into());
    assert!(update_host("stable", &m).is_ok());
    assert_eq!(
        parse_ssh_config().unwrap()[0].user.as_deref(),
        Some("change")
    );
}

#[test]
fn update_host_garde_le_commentaire_qui_annonce_le_bloc_suivant() {
    // Trouvé par l'audit du 7 septembre 2026 : comme `remove_host`,
    // `update_host` avalait la ligne vide séparant le bloc réécrit du suivant
    // (le bloc rendu se collait à `Host staging`) et déplaçait la note qui
    // annonce staging. Le tampon de fin la garde devant son bloc.
    let _h = crate::testutil::temp_home();
    let path = ssh_config_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "# Prod\nHost prod\n  HostName 1\n\n# Staging — accès via Jean, clé chez ops\nHost staging\n  HostName 2\n",
    )
    .unwrap();

    let mut modifie = host("prod");
    modifie.hostname = Some("9.9.9.9".into());
    update_host("prod", &modifie).unwrap();

    let apres = std::fs::read_to_string(&path).unwrap();
    assert!(
        apres.contains("# Staging — accès via Jean, clé chez ops\nHost staging"),
        "la note sur staging doit rester devant son bloc :\n{apres}"
    );
    // Le bloc réécrit ne se colle plus à staging : une ligne vide sépare
    // encore les deux blocs.
    assert!(
        apres.contains("\n\n# Staging"),
        "le séparateur entre les deux blocs a été avalé :\n{apres}"
    );
    assert_eq!(
        parse_ssh_config().unwrap()[0].hostname.as_deref(),
        Some("9.9.9.9")
    );
}
