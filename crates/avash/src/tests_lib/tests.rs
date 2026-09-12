use super::*;

#[test]
fn une_cle_avec_espace_est_guillemetee_et_se_relit() {
    // Trouvé par l'audit du 7 septembre 2026 : une valeur avec espace était
    // écrite sans guillemets et faisait rejeter TOUTE la configuration par
    // OpenSSH. Elle doit être guillemetée à l'écriture et déguillemetée à la
    // lecture (round-trip).
    let mut h = SshHost {
        alias: "prod".into(),
        hostname: Some("prod.exemple.com".into()),
        ..Default::default()
    };
    h.identity_file = Some(r"C:\Users\Jean Dupont\.ssh\id".into());
    let bloc = render_host_block(&h);
    assert!(
        bloc.contains(r#"IdentityFile "C:\Users\Jean Dupont\.ssh\id""#),
        "clé avec espace non guillemetée :\n{bloc}"
    );
    let relu = &parse_config_str(&bloc)[0];
    assert_eq!(
        relu.identity_file.as_deref(),
        Some(r"C:\Users\Jean Dupont\.ssh\id"),
        "la clé doit se relire sans les guillemets"
    );
    // Une clé sans espace n'est pas guillemetée.
    h.identity_file = Some("/home/u/.ssh/id".into());
    assert!(render_host_block(&h).contains("IdentityFile /home/u/.ssh/id"));
}

#[test]
fn validate_host_refuse_l_espace_dans_hostname_user_proxyjump() {
    let base = SshHost {
        alias: "a".into(),
        ..Default::default()
    };
    let avec = |f: fn(&mut SshHost)| {
        let mut h = base.clone();
        f(&mut h);
        validate_host(&h)
    };
    assert!(avec(|h| h.hostname = Some("un hote".into())).is_err());
    assert!(avec(|h| h.user = Some("jean dupont".into())).is_err());
    assert!(avec(|h| h.proxy_jump = Some("a b".into())).is_err());
    // Un guillemet est refusé partout ; l'espace dans IdentityFile passe.
    assert!(avec(|h| h.hostname = Some("a\"b".into())).is_err());
    assert!(avec(|h| h.identity_file = Some("/home/u/ma clé".into())).is_ok());
}

#[test]
fn validate_host_accepte_un_rebond_a_plusieurs_sauts() {
    // Trouvé par l'audit du 9 septembre 2026 : « bastion, relais:2200 »
    // (virgule PUIS espace) est la forme que l'interface donne en exemple,
    // celle que `split_proxy_jump` sait découper, et celle dont `ssh -vv`
    // montre qu'OpenSSH enchaîne bien les deux sauts. La validation
    // refusait pourtant tout espace : un hôte déjà enregistré ainsi ne
    // pouvait plus être réenregistré, le seul fait de changer un tag
    // faisait échouer l'enregistrement.
    let avec_rebond = |v: &str| {
        validate_host(&SshHost {
            alias: "prod".into(),
            proxy_jump: Some(v.into()),
            ..Default::default()
        })
    };
    for bon in [
        "bastion, relais:2200",
        "bastion,deploy@10.0.0.1:2222",
        " bastion , relais ",
        "u@[2001:db8::1]:2222, bastion",
    ] {
        assert!(avec_rebond(bon).is_ok(), "devrait passer : {bon}");
    }
    // L'espace à l'intérieur d'un maillon reste refusé, par prudence : ssh
    // ne s'en plaint pas (mesuré avec OpenSSH_10.5p1), il le lit de
    // travers, « saut un.invalid » devenant l'hôte « saut » suivi d'une
    // commande distante « un.invalid ».
    for mauvais in ["bastion relais", "bastion, un relais", "a b"] {
        assert!(avec_rebond(mauvais).is_err(), "devrait échouer : {mauvais}");
    }
}

#[test]
fn validate_host_refuse_les_caracteres_de_controle() {
    // Trouvé par l'audit du 9 septembre 2026 : la validation ne refusait
    // que `\n`, `\r`, `\0` et le guillemet, et la variante « sans espace »
    // n'ajoutait que `char::is_whitespace`, qui ignore les codes de
    // contrôle C0. Un `HostName srv\x1b]0;PWNED\x07` importé depuis un
    // export PuTTY hostile passait donc jusque dans `~/.ssh/config`, où
    // `render_host_block` l'écrit tel quel (il ne fait qu'un `.trim()`).
    // La séquence repartait ensuite vers le terminal à chaque `avash list`,
    // sans que personne ouvre le fichier : titre de fenêtre réécrit,
    // presse-papiers manipulé par OSC 52, voire réponse d'une requête
    // d'état réinjectée comme une frappe sur certains émulateurs.
    let base = SshHost {
        alias: "prod".into(),
        ..Default::default()
    };
    let avec = |f: &dyn Fn(&mut SshHost)| {
        let mut h = base.clone();
        f(&mut h);
        validate_host(&h)
    };
    // ESC (début de toute séquence ANSI), BEL (fin d'un OSC), DEL, un C1
    // (0x9B, CSI sur un octet) et la tabulation, qui sépare la clé de la
    // valeur pour OpenSSH : aucun n'a sa place dans un champ.
    for c in ['\u{1b}', '\u{7}', '\u{7f}', '\u{9b}', '\t'] {
        let charge = format!("srv{c}x");
        assert!(
            avec(&|h| h.hostname = Some(charge.clone())).is_err(),
            "HostName devrait refuser U+{:04X}",
            c as u32
        );
        assert!(
            avec(&|h| h.user = Some(charge.clone())).is_err(),
            "User devrait refuser U+{:04X}",
            c as u32
        );
        assert!(
            avec(&|h| h.proxy_jump = Some(charge.clone())).is_err(),
            "ProxyJump devrait refuser U+{:04X}",
            c as u32
        );
        assert!(
            avec(&|h| h.identity_file = Some(format!("/home/u/{charge}"))).is_err(),
            "IdentityFile devrait refuser U+{:04X}",
            c as u32
        );
        assert!(
            avec(&|h| h.tags = vec![charge.clone()]).is_err(),
            "Tags devrait refuser U+{:04X}",
            c as u32
        );
        assert!(
            avec(&|h| h.folder = charge.clone()).is_err(),
            "Folder devrait refuser U+{:04X}",
            c as u32
        );
        // L'alias a sa propre validation, avec le même trou : il finit sur
        // la ligne `Host`, tout aussi lue par le terminal.
        assert!(
            validate_host(&SshHost {
                alias: charge.clone(),
                ..Default::default()
            })
            .is_err(),
            "l'alias devrait refuser U+{:04X}",
            c as u32
        );
    }
    // Rien de légitime ne doit être devenu invalide au passage.
    assert!(avec(&|h| h.hostname = Some("prod.exemple.com".into())).is_ok());
    assert!(avec(&|h| h.identity_file = Some("/home/u/ma clé".into())).is_ok());
    assert!(avec(&|h| h.folder = "Prod/Bases".into()).is_ok());
    assert!(avec(&|h| h.tags = vec!["été".into(), "bases".into()]).is_ok());
}

#[test]
fn sans_controle_neutralise_une_sequence_ansi_avant_affichage() {
    // Trouvé par l'audit du 9 septembre 2026 : durcir la seule écriture
    // d'Avash ne protège pas d'un `~/.ssh/config` déjà piégé par un autre
    // outil. `avash list` imprimait alias, hôte et rebond bruts, donc
    // chaque exécution rejouait la séquence dans le terminal.
    let propre = sans_controle("srv\u{1b}]0;PWNED\u{7}");
    assert!(
        !propre.chars().any(char::is_control),
        "il reste un caractère de contrôle : {propre:?}"
    );
    assert_eq!(propre, "srv ]0;PWNED ");
    // Le texte inoffensif, accents compris, ne bouge pas.
    assert_eq!(sans_controle("prod.exemple.com"), "prod.exemple.com");
    assert_eq!(sans_controle("relais été"), "relais été");
}

#[test]
fn developper_tilde_resout_dans_le_repertoire_personnel() {
    // Trouvé par l'audit du 7 septembre 2026 : la forme `~/…` d'IdentityFile
    // n'était jamais développée. `~` seul et `~/x` visent le HOME ; un chemin
    // absolu ou relatif sans tilde reste inchangé.
    let g = crate::testutil::temp_home();
    assert_eq!(developper_tilde("~/.ssh/k"), g.dir().join(".ssh").join("k"));
    assert_eq!(developper_tilde("~"), g.dir().to_path_buf());
    assert_eq!(developper_tilde("/tmp/k"), PathBuf::from("/tmp/k"));
    assert_eq!(developper_tilde("k"), PathBuf::from("k"));
    // Un tilde qui n'est pas en tête n'est pas un raccourci de HOME.
    assert_eq!(developper_tilde("/a/~/b"), PathBuf::from("/a/~/b"));
}

#[test]
fn tags_lus_et_reecrits() {
    let cfg = "Host prod\n  HostName 10.0.0.1\n  #Tags: prod, web\n";
    let h = &parse_config_str(cfg)[0];
    assert_eq!(h.tags, vec!["prod", "web"]);
    // Round-trip : render puis relit les memes tags.
    let rendered = render_host_block(h);
    assert!(rendered.contains("#Tags: prod, web"), "{rendered}");
    assert_eq!(parse_config_str(&rendered)[0].tags, vec!["prod", "web"]);
}

#[test]
fn tags_hors_bloc_host_ignores() {
    // Un #Tags avant tout Host ne s'attache a rien.
    let h = parse_config_str("#Tags: orphelin\nHost a\n  HostName x\n");
    assert!(h[0].tags.is_empty());
}

#[test]
fn split_proxy_jump_decoupe_une_chaine() {
    let v = split_proxy_jump("bastion, deploy@10.0.0.1:2222");
    assert_eq!(v.len(), 2);
    assert_eq!(
        v[0],
        HopSpec {
            user: None,
            host: "bastion".into(),
            port: None
        }
    );
    assert_eq!(
        v[1],
        HopSpec {
            user: Some("deploy".into()),
            host: "10.0.0.1".into(),
            port: Some(2222)
        }
    );
}

#[test]
fn split_proxy_jump_gere_none_et_vide() {
    assert!(split_proxy_jump("none").is_empty());
    assert!(split_proxy_jump("").is_empty());
    assert!(split_proxy_jump("  ,  ").is_empty());
}

// Trouvé par l'audit du 7 septembre 2026 : un bastion IPv6 littéral s'écrit
// entre crochets (`[2001:db8::1]:2222`), la seule syntaxe qu'OpenSSH accepte
// en ProxyJump (`hpdelim` coupe une IPv6 nue au premier `:`, « Bad
// ProxyJump »). Le `rsplit_once(':')` gardait les crochets dans l'hôte, si
// bien que russh recevait `"[2001:db8::1]"`, ni IP analysable ni nom
// résolvable : le rebond échouait là où `ssh -J` marchait.
#[test]
fn split_proxy_jump_retire_les_crochets_ipv6() {
    let v = split_proxy_jump("u@[2001:db8::1]:2222");
    assert_eq!(v.len(), 1);
    assert_eq!(
        v[0],
        HopSpec {
            user: Some("u".into()),
            host: "2001:db8::1".into(),
            port: Some(2222)
        }
    );

    // Même adresse sans port : les crochets tombent, port `None`.
    let sans_port = split_proxy_jump("[2001:db8::1]");
    assert_eq!(
        sans_port,
        vec![HopSpec {
            user: None,
            host: "2001:db8::1".into(),
            port: None
        }]
    );

    // Un port nul derrière les crochets reste refusé : morceau sans port,
    // la résolution le dira introuvable plutôt que de viser le port 0.
    let port_nul = split_proxy_jump("[2001:db8::1]:0");
    assert_eq!(port_nul[0].host, "2001:db8::1");
    assert_eq!(port_nul[0].port, None);

    // Crochet ouvrant sans fermant (saisie malformée) : on retire quand même
    // le `[` de tête pour tenir l'invariant « pas de crochet dans l'hôte »
    // gardé par la cible fuzz et le test de mutation. La résolution refusera
    // cet hôte de toute façon.
    let non_ferme = split_proxy_jump("[2001:db8::1");
    assert_eq!(non_ferme[0].host, "2001:db8::1");
    assert_eq!(non_ferme[0].port, None);
    assert!(!non_ferme[0].host.starts_with('['));

    // Crochets imbriqués (`[[h]:22`), trouvés par cargo-fuzz après le
    // premier correctif : le premier `[` retiré, `split_once(']')` laissait
    // `[h`. Aucun crochet ne doit subsister dans l'hôte.
    for pathologique in [
        "[[h]:22",
        "[[2001:db8::1]]",
        "[]",
        "[a]b]",
        "] a",
        "[ a",
        "a ]",
        "] a ]:2",
    ] {
        for hop in split_proxy_jump(pathologique) {
            assert!(
                !hop.host.contains(['[', ']']),
                "crochet gardé pour {pathologique:?} : {hop:?}"
            );
            assert_eq!(
                hop.host.trim(),
                hop.host,
                "hôte non rogné pour {pathologique:?} : {hop:?}"
            );
        }
    }
}

// Comportement documenté d'une IPv6 littérale SANS crochets : OpenSSH la
// refuse en ProxyJump, il n'y a donc pas de résultat « correct » à viser.
// On note ce que rend `split_proxy_jump` (découpe au dernier `:`, jamais
// entre crochets) pour que le choix soit visible et gardé : la forme entre
// crochets reste la seule voie vers un bastion IPv6 littéral.
#[test]
fn split_proxy_jump_ipv6_nue_reste_une_erreur_de_config() {
    let v = split_proxy_jump("2001:db8::1");
    // Le dernier groupe décimal (`1`) est pris pour un port : résultat
    // volontairement faux, comme une IPv6 nue l'est déjà pour OpenSSH.
    assert_eq!(v[0].host, "2001:db8:");
    assert_eq!(v[0].port, Some(1));
    // Aucun morceau ne conserve de crochet : invariant tenu même ici.
    assert!(!v[0].host.starts_with('['));
}

#[test]
fn parses_basic_config() {
    let cfg = r"
# commentaire
Host web
HostName 10.0.0.5
User adrien
Port 2222
IdentityFile ~/.ssh/id_ed25519

Host db bastion
HostName 10.0.0.9
User root
";
    let hosts = parse_config_str(cfg);
    assert_eq!(hosts.len(), 3);
    assert_eq!(hosts[0].alias, "web");
    assert_eq!(hosts[0].port, Some(2222));
    assert_eq!(hosts[1].alias, "db");
    assert_eq!(hosts[2].alias, "bastion");
    assert_eq!(hosts[2].user, Some("root".into()));
}

#[test]
fn les_blocs_a_motif_sont_absents_de_la_liste_editable() {
    // La liste éditable ne montre que des hôtes connectables : un bloc à
    // joker (`Host db*`) n'en est pas un, on ne le liste donc pas. Il n'est
    // pas ignoré pour autant — ses valeurs par défaut sont appliquées à la
    // résolution (`resoudre_hote_dans`), comme le fait `ssh`.
    let cfg = "Host db*\n  User admin\nHost prod-1\n  User root";
    let hosts = parse_config_str(cfg);
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0].alias, "prod-1");
}

#[test]
fn les_valeurs_par_defaut_de_host_etoile_s_appliquent() {
    // Trouvé par l'audit du 7 septembre 2026 : un `User`/`IdentityFile` posé
    // dans `Host *` s'applique à chaque hôte. `parse_config_str` jetait le
    // bloc à joker, et Avash résolvait `prod` avec l'utilisateur courant et
    // sans clé, là où `ssh prod` prenait l'utilisateur et la clé du `Host *`.
    let cfg = "Host *\n  User adrien\n  IdentityFile ~/.ssh/id_ed25519\n\n\
               Host prod\n  HostName 10.0.0.1\n";
    let h = resoudre_hote_dans(cfg, "prod").expect("prod est un hôte littéral");
    assert_eq!(h.user.as_deref(), Some("adrien"));
    assert_eq!(h.identity_file.as_deref(), Some("~/.ssh/id_ed25519"));
    assert_eq!(h.hostname.as_deref(), Some("10.0.0.1"));
    // Le tilde n'est PAS développé ici : la résolution le laisse au fichier,
    // les appelants (Target::from_alias…) appellent `developper_tilde`. Le
    // port reste vide, le défaut 22 étant appliqué par les appelants.
    assert_eq!(h.port, None);
}

#[test]
fn la_premiere_valeur_obtenue_est_retenue() {
    // Ordre du fichier : `Host *` en tête pose `User root` AVANT que le bloc
    // littéral `Host prod` ne pose `User adrien`. OpenSSH retient la première
    // valeur obtenue : root l'emporte.
    let cfg = "Host *\n  User root\n\nHost prod\n  HostName 10.0.0.1\n  User adrien\n";
    assert_eq!(
        resoudre_hote_dans(cfg, "prod").unwrap().user.as_deref(),
        Some("root"),
        "première valeur = Host *"
    );
    // Bloc littéral d'abord : c'est lui qui l'emporte alors (disposition
    // recommandée par le man, `Host *` en fin de fichier).
    let cfg2 = "Host prod\n  HostName 10.0.0.1\n  User adrien\n\nHost *\n  User root\n";
    assert_eq!(
        resoudre_hote_dans(cfg2, "prod").unwrap().user.as_deref(),
        Some("adrien"),
        "le bloc littéral vient avant"
    );
}

#[test]
fn la_premiere_occurrence_dans_un_meme_bloc_est_retenue() {
    // Trouvé par l'audit du 9 septembre 2026 : la règle « la première valeur
    // obtenue est retenue » n'était appliquée qu'ENTRE blocs. À l'intérieur
    // d'un même bloc, chaque directive écrasait la précédente, si bien qu'un
    // bloc issu d'une fusion manuelle (`User adrien` puis `User root`) faisait
    // afficher et connecter Avash en « root » là où `ssh prod` se connecte en
    // « adrien ». Aucun message ne signalait l'ambiguïté.
    let cfg = "Host prod\n  HostName 10.0.0.1\n  HostName 10.0.0.2\n  \
               User adrien\n  User root\n  Port 22\n  Port 2222\n  \
               IdentityFile ~/.ssh/premiere\n  IdentityFile ~/.ssh/seconde\n  \
               ProxyJump bastion\n  ProxyJump autre\n";

    let liste = parse_config_str(cfg);
    assert_eq!(liste.len(), 1);
    let h = &liste[0];
    assert_eq!(h.hostname.as_deref(), Some("10.0.0.1"), "HostName dupliqué");
    assert_eq!(h.user.as_deref(), Some("adrien"), "User dupliqué");
    assert_eq!(h.port, Some(22), "Port dupliqué");
    assert_eq!(
        h.identity_file.as_deref(),
        Some("~/.ssh/premiere"),
        "IdentityFile dupliqué"
    );
    assert_eq!(
        h.proxy_jump.as_deref(),
        Some("bastion"),
        "ProxyJump dupliqué"
    );

    // Même règle à la résolution : les deux chemins partagent `blocs_bruts`.
    let r = resoudre_hote_dans(cfg, "prod").unwrap();
    assert_eq!(r.hostname.as_deref(), Some("10.0.0.1"));
    assert_eq!(r.user.as_deref(), Some("adrien"));
    assert_eq!(r.port, Some(22));
    assert_eq!(r.identity_file.as_deref(), Some("~/.ssh/premiere"));
    assert_eq!(r.proxy_jump.as_deref(), Some("bastion"));

    // Un port refusé par OpenSSH (`Port 0`, « Bad port ») ne compte pas comme
    // une première valeur : Avash l'ignore, et le port suivant valide sert.
    // L'ordre inverse est le cas qui discrimine : sans la règle « premier
    // gagnant », le `Port 0` final effaçait le 2222 déjà lu.
    let cfg_port_nul = "Host prod\n  HostName 10.0.0.1\n  Port 0\n  Port 2222\n";
    assert_eq!(
        resoudre_hote_dans(cfg_port_nul, "prod").unwrap().port,
        Some(2222)
    );
    let cfg_port_nul_apres = "Host prod\n  HostName 10.0.0.1\n  Port 2222\n  Port 0\n";
    assert_eq!(
        resoudre_hote_dans(cfg_port_nul_apres, "prod").unwrap().port,
        Some(2222)
    );
}

#[test]
fn une_valeur_vide_ne_compte_pas_comme_premiere_valeur() {
    // Trouvé à la relecture de l'audit du 9 septembre 2026 : la règle
    // « premier gagnant » posée juste au-dessus faisait, appliquée sans
    // nuance, qu'un résidu de fusion manuelle (`HostName` sans argument,
    // `User ""`) masquait la vraie valeur écrite juste en dessous. Avash
    // aurait visé une adresse vide, que rien ne rattrape en aval : le
    // binaire fait `hostname.unwrap_or(alias)`, qui laisse passer `""`.
    let cfg = "Host prod\n  HostName\n  HostName 10.0.0.1\n  \
               User \"\"\n  User adrien\n  IdentityFile\n  \
               IdentityFile ~/.ssh/prod\n  ProxyJump \"\"\n  ProxyJump bastion\n";

    let h = resoudre_hote_dans(cfg, "prod").unwrap();
    assert_eq!(h.hostname.as_deref(), Some("10.0.0.1"), "HostName vide");
    assert_eq!(h.user.as_deref(), Some("adrien"), "User vide");
    assert_eq!(
        h.identity_file.as_deref(),
        Some("~/.ssh/prod"),
        "IdentityFile vide"
    );
    assert_eq!(h.proxy_jump.as_deref(), Some("bastion"), "ProxyJump vide");

    // Seule : une directive vide laisse le champ absent, jamais `Some("")`.
    // Le repli sur l'alias peut alors jouer.
    let seule = parse_config_str("Host prod\n  HostName\n  User \"\"\n");
    assert_eq!(seule[0].hostname, None);
    assert_eq!(seule[0].user, None);
}

#[test]
fn la_premiere_convention_avash_du_bloc_est_retenue() {
    // Même audit : les conventions `#Tags:`/`#Folder:` restaient en
    // dernier-gagne dans un bloc alors que la résolution leur applique le
    // premier-gagne entre blocs. Un bloc recollé à la main affichait donc
    // l'étiquette du morceau collé, pas celle d'origine.
    let cfg = "Host prod\n  #Tags: prod, linux\n  #Tags: brouillon\n  \
               #Folder: Client/Prod\n  #Folder: Corbeille\n  HostName 10.0.0.1\n";
    let h = &parse_config_str(cfg)[0];
    assert_eq!(h.tags, vec!["prod".to_string(), "linux".to_string()]);
    assert_eq!(h.folder, "Client/Prod");

    // Une liste vide ne compte pas comme première valeur non plus.
    let vide = "Host prod\n  #Tags:\n  #Tags: prod\n  #Folder:\n  \
                #Folder: Client\n  HostName 10.0.0.1\n";
    let h = &parse_config_str(vide)[0];
    assert_eq!(h.tags, vec!["prod".to_string()]);
    assert_eq!(h.folder, "Client");
}

#[test]
fn un_motif_de_negation_annule_le_bloc() {
    // `Host !prod *` : le `!prod` matche `prod` et annule tout le bloc, donc
    // `prod` n'hérite pas de son `User`. Un autre hôte, lui, en hérite.
    let cfg = "Host !prod *\n  User root\n\nHost prod\n  HostName 10.0.0.1\n\n\
               Host web\n  HostName 10.0.0.2\n";
    assert_eq!(resoudre_hote_dans(cfg, "prod").unwrap().user, None);
    assert_eq!(
        resoudre_hote_dans(cfg, "web").unwrap().user.as_deref(),
        Some("root")
    );
}

#[test]
fn un_alias_inconnu_ne_se_resout_pas() {
    // Sans bloc littéral, ce n'est pas un hôte connu d'Avash, même si
    // `Host *` le couvrirait : `resoudre_hote` rend alors `None`.
    let cfg = "Host *\n  User root\n";
    assert!(resoudre_hote_dans(cfg, "inexistant").is_none());
}

/// Contrat K5 de l'audit du 12 septembre 2026 (C-SIL-4) : un `~/.ssh/config`
/// absent est une configuration vide (poste neuf), un fichier présent mais
/// illisible est une erreur. `parse_ssh_config` rendait une erreur dans les
/// deux cas, et l'interface montrait « Aucun hôte » pour un fichier illisible,
/// comme pour un fichier absent. Le fichier illisible est ici un répertoire à
/// sa place : la lecture échoue même pour root, contrairement à un mode 0000.
#[test]
fn un_config_illisible_n_est_pas_un_config_vide() {
    let garde = crate::testutil::temp_home();
    assert!(
        parse_ssh_config().expect("absent : Ok").is_empty(),
        "un fichier absent vaut une configuration vide"
    );
    assert_eq!(configuration_resolue().expect("absent : Ok"), "");
    std::fs::create_dir_all(garde.dir().join(".ssh").join("config")).unwrap();
    let e = parse_ssh_config().expect_err("un fichier illisible est une erreur");
    assert!(
        e.to_string().contains("config"),
        "l'erreur nomme le fichier : {e}"
    );
    assert!(configuration_resolue().is_err());
}

/// Contrat K3 de l'audit du 12 septembre 2026 (C-perf-5) : la configuration
/// lue une fois, `Include` résolus, pour que les appelants qui résolvent
/// plusieurs hôtes passent par `resoudre_hote_dans` au lieu de relire et
/// réanalyser le fichier (et ses inclus) une fois par hôte.
#[test]
fn la_configuration_resolue_porte_les_hotes_inclus() {
    let garde = crate::testutil::temp_home();
    let ssh = garde.dir().join(".ssh");
    std::fs::create_dir_all(ssh.join("config.d")).unwrap();
    std::fs::write(
        ssh.join("config"),
        "Include config.d/*\n\nHost principal\n  HostName 10.0.0.1\n\nHost *\n  User defaut\n",
    )
    .unwrap();
    std::fs::write(
        ssh.join("config.d").join("equipe"),
        "Host inclus\n  HostName 10.0.0.2\n",
    )
    .unwrap();
    let contenu = configuration_resolue().unwrap();
    let inclus = resoudre_hote_dans(&contenu, "inclus").expect("hôte d'un Include");
    assert_eq!(inclus.hostname.as_deref(), Some("10.0.0.2"));
    assert_eq!(
        inclus.user.as_deref(),
        Some("defaut"),
        "le Host * s'applique"
    );
    assert!(resoudre_hote_dans(&contenu, "principal").is_some());
    // Même résultat que la résolution qui relit le fichier.
    assert_eq!(resoudre_hote("inclus").unwrap().hostname, inclus.hostname);
}

/// Contrat K15 de l'audit du 12 septembre 2026 (C-panique-4) : un verrou
/// empoisonné par une panique ailleurs se reprend, avec ses données, au lieu
/// de faire paniquer à son tour chaque commande qui le prend.
#[test]
fn un_verrou_empoisonne_se_reprend_avec_ses_donnees() {
    let m = std::sync::Arc::new(std::sync::Mutex::new(vec![1]));
    let m2 = m.clone();
    let _ = std::thread::spawn(move || {
        let _g = m2.lock().unwrap();
        panic!("panique volontaire sous le verrou");
    })
    .join();
    assert!(m.is_poisoned(), "le décor : un verrou empoisonné");
    m.verrou().push(2);
    assert_eq!(*m.verrou(), vec![1, 2]);
}

/// Audit du 12 septembre 2026 (C-panique-10) : trois fichiers de `config.d`
/// qui s'incluent chacun par motif faisaient 3^16 lectures ; `list_hosts`
/// figeait l'interface. Chaque fichier n'est désormais lu qu'une fois, et
/// chaque hôte n'apparaît qu'une fois.
#[test]
fn trois_fichiers_qui_s_incluent_par_motif_ne_figent_pas() {
    let garde = crate::testutil::temp_home();
    let ssh = garde.dir().join(".ssh");
    std::fs::create_dir_all(ssh.join("config.d")).unwrap();
    std::fs::write(ssh.join("config"), "Include ~/.ssh/config.d/*\n").unwrap();
    for nom in ["a", "b", "c"] {
        std::fs::write(
            ssh.join("config.d").join(nom),
            format!("Include ~/.ssh/config.d/*\n\nHost hote-{nom}\n  HostName 10.0.0.1\n"),
        )
        .unwrap();
    }
    let (envoi, reception) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = envoi.send(parse_ssh_config());
    });
    let hotes = reception
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("la résolution des Include doit terminer")
        .unwrap();
    let mut noms: Vec<_> = hotes.iter().map(|h| h.alias.as_str()).collect();
    noms.sort_unstable();
    assert_eq!(noms, ["hote-a", "hote-b", "hote-c"], "chaque hôte une fois");
}
