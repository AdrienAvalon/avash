//! Import de sessions `PuTTY` et `MobaXterm`.

use avash::SshHost;

/// Une session lue chez un autre outil, avec ce qu'il faut pour la montrer.
#[derive(Debug, serde::Serialize)]
pub struct CandidatImport {
    pub source: avash::import::Source,
    pub nom_origine: String,
    pub host: SshHost,
    /// Clé `PuTTY` à convertir à l'import, si `puttygen` est là.
    pub ppk: Option<String>,
    pub remarques: Vec<String>,
    /// Alias d'un hôte déjà déclaré qui vise le même serveur (hôte, port,
    /// utilisateur) : proposé décoché, pour ne pas dupliquer.
    pub doublon: Option<String>,
}

/// Un bureau RDP lu chez un autre outil, avec le doublon éventuel.
#[derive(Debug, serde::Serialize)]
pub struct BureauCandidat {
    #[serde(flatten)]
    pub bureau: avash::import::BureauImporte,
    /// Défauts qui empêchent d'écrire ce bureau (utilisateur RDP vide, adresse à
    /// espace) : le signet reste montrable mais l'import le sautera. Porter le
    /// défaut ici évite que le front le propose coché pour rien.
    pub remarques: Vec<String>,
    /// Nom d'un bureau déjà enregistré qui vise le même serveur.
    pub doublon: Option<String>,
}

/// Ce qu'une analyse a trouvé, et où elle a regardé.
#[derive(Debug, serde::Serialize)]
pub struct BilanImport {
    pub candidats: Vec<CandidatImport>,
    pub bureaux: Vec<BureauCandidat>,
    /// Sessions d'un autre protocole, laissées de côté.
    pub ignorees: usize,
    /// Emplacements consultés, pour que l'utilisateur sache d'où ça vient.
    pub consultes: Vec<String>,
}

/// Un hôte retenu à l'import, avec sa clé `PuTTY` éventuelle.
#[derive(Debug, serde::Deserialize)]
pub struct HoteAImporter {
    pub host: SshHost,
    #[serde(default)]
    pub ppk: Option<String>,
}

/// Ce que l'import a écrit, et ce qu'il n'a pas pu faire.
#[derive(Debug, Default, serde::Serialize)]
pub struct BilanApply {
    pub hotes: usize,
    pub bureaux: usize,
    pub cles_converties: usize,
    pub avertissements: Vec<String>,
}

/// Les emplacements par défaut : sessions `PuTTY` (fichiers sous Unix, registre
/// sous Windows) et `MobaXterm.ini` là où Windows le range.
fn lectures_par_defaut() -> (Vec<avash::import::Lecture>, Vec<String>) {
    let mut lectures = Vec::new();
    let mut consultes = Vec::new();
    #[cfg(not(windows))]
    if let Some(dir) = avash::import::repertoire_putty() {
        if dir.is_dir() {
            consultes.push(dir.display().to_string());
            lectures.push(avash::import::putty_sessions_dans(&dir));
        }
    }
    #[cfg(windows)]
    {
        consultes.push(r"HKCU\Software\SimonTatham\PuTTY\Sessions".to_string());
        lectures.push(avash::import::putty_sessions_registre());
    }
    for chemin in avash::import::chemins_mobaxterm() {
        if let Ok(contenu) = std::fs::read_to_string(&chemin) {
            consultes.push(chemin.display().to_string());
            lectures.push(avash::import::parse_mobaxterm_ini(&contenu));
        }
    }
    (lectures, consultes)
}

/// Analyse les sessions importables.
///
/// Sans `chemin`, les emplacements habituels ; avec, un répertoire de
/// sessions `PuTTY` ou un fichier `MobaXterm.ini` / `.mxtsessions` désigné par
/// l'utilisateur. Rien n'est écrit.
#[tauri::command(async)]
pub fn import_scan(chemin: Option<String>) -> Result<BilanImport, String> {
    let (lectures, consultes) = match chemin.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        None => lectures_par_defaut(),
        Some(c) => {
            let p = std::path::Path::new(c);
            let lecture = if p.is_dir() {
                avash::import::putty_sessions_dans(p)
            } else {
                let contenu = std::fs::read_to_string(p)
                    .map_err(|e| format!("Lecture de {c} impossible : {e}"))?;
                avash::import::parse_mobaxterm_ini(&contenu)
            };
            (vec![lecture], vec![c.to_string()])
        }
    };
    // Trouvé par l'audit du 7 septembre 2026 : un `unwrap_or_default()` ici
    // importait à l'aveugle quand `~/.ssh/config` était illisible (non UTF-8) :
    // `pris` partait vide, donc `alias_libre` ne renommait rien et proposait des
    // alias entrant en collision avec ceux du fichier illisible. On propage
    // désormais l'erreur pour la dire à l'utilisateur avant tout import.
    let existants = avash::parse_ssh_config().map_err(|e| format!("{e:#}"))?;
    let bureaux_existants = avash::rdphost::load_hosts().unwrap_or_default();
    let mut pris: Vec<String> = existants.iter().map(|h| h.alias.clone()).collect();
    let mut candidats = Vec::new();
    let mut bureaux = Vec::new();
    let mut ignorees = 0;
    for lecture in lectures {
        ignorees += lecture.ignorees;
        for b in lecture.bureaux {
            let doublon = bureaux_existants
                .iter()
                .find(|e| {
                    e.host.eq_ignore_ascii_case(&b.host) && e.port == b.port && e.user == b.user
                })
                .map(|e| e.name.clone());
            // Le même contrôle que l'écriture (`RdpHost::validate`) : un signet
            // sans utilisateur ou à adresse invalide serait sauté à l'import,
            // autant le signaler au scan plutôt que de le proposer coché.
            let remarques =
                avash::rdphost::RdpHost::new(&b.name, &b.host, b.port, &b.user, 1280, 800)
                    .validate()
                    .err()
                    .map(|e| vec![format!("{e:#}")])
                    .unwrap_or_default();
            bureaux.push(BureauCandidat {
                bureau: b,
                remarques,
                doublon,
            });
        }
        for s in lecture.sessions {
            let mut host = s.host;
            host.alias = avash::import::alias_libre(&host.alias, &pris);
            pris.push(host.alias.clone());
            // Trouvé par l'audit du 7 septembre 2026 : un hôte déclaré sans
            // `HostName` (bloc `Host web.example.com` seul) porte
            // `hostname: None`, alors que le candidat PuTTY/MobaXterm vise
            // `Some("web.example.com")`. La comparaison stricte ne voyait pas
            // le doublon : le candidat était proposé coché et l'import créait
            // une seconde entrée `web.example.com-2` pour le même serveur. On
            // applique le même repli que `hosts_health` (hostname sinon alias)
            // et, comme la branche bureaux ci-dessus et `append_host`, on
            // compare sans tenir compte de la casse (les noms DNS y sont
            // insensibles). Le candidat PuTTY/MobaXterm a toujours un
            // `hostname`, donc le repli sur son alias — déjà réécrit en `-N`
            // par `alias_libre` juste au-dessus — ne joue jamais ici ; il n'est
            // là que pour la symétrie.
            let cible_candidat = host.hostname.as_deref().unwrap_or(&host.alias);
            let doublon = existants
                .iter()
                .find(|e| {
                    e.hostname
                        .as_deref()
                        .unwrap_or(&e.alias)
                        .eq_ignore_ascii_case(cible_candidat)
                        && e.port.unwrap_or(22) == host.port.unwrap_or(22)
                        && e.user == host.user
                })
                .map(|e| e.alias.clone());
            candidats.push(CandidatImport {
                source: s.source,
                nom_origine: s.nom_origine,
                host,
                ppk: s.ppk,
                remarques: s.remarques,
                doublon,
            });
        }
    }
    Ok(BilanImport {
        candidats,
        bureaux,
        ignorees,
        consultes,
    })
}

/// Écrit les hôtes retenus dans `~/.ssh/config`, dans l'ordre, et les bureaux
/// RDP dans leur fichier. Un alias déjà pris entre-temps est renommé plutôt
/// que refusé. Une clé `PuTTY` est convertie avec `puttygen` dans `~/.ssh` quand
/// l'outil est là ; sinon, ou en cas d'échec, l'hôte est écrit sans clé et
/// l'avertissement le dit.
///
/// Hors du fil principal (audit du 12 septembre 2026, C-SIL-7) : la boucle
/// lance `puttygen` et l'attend, clé après clé.
#[tauri::command]
pub async fn import_apply(
    hosts: Vec<HoteAImporter>,
    bureaux: Vec<avash::import::BureauImporte>,
) -> Result<BilanApply, String> {
    super::bloquant(move || appliquer_import(hosts, bureaux)).await
}

fn appliquer_import(
    hosts: Vec<HoteAImporter>,
    bureaux: Vec<avash::import::BureauImporte>,
) -> Result<BilanApply, String> {
    // Trouvé par l'audit du 7 septembre 2026 : comme `import_scan`, un
    // `unwrap_or_default()` faisait partir `pris` vide sur un `~/.ssh/config`
    // illisible (non UTF-8), si bien que `alias_libre` ne détectait plus les
    // collisions et que `append_host` collait les blocs à un fichier cru vide.
    // On refuse l'import plutôt que d'abîmer la configuration.
    let mut pris: Vec<String> = avash::parse_ssh_config()
        .map_err(|e| format!("{e:#}"))?
        .into_iter()
        .map(|h| h.alias)
        .collect();
    let mut bilan = BilanApply::default();
    let dossier_cles = avash::repertoire_personnel().map(|h| h.join(".ssh"));
    // `puttygen` ne dépend pas de l'hôte : le sonder une seule fois, pas à
    // chaque tour de boucle (l'audit du 7 septembre 2026 relevait un
    // `Command::new("puttygen")` lancé par hôte importé).
    let puttygen = avash::import::puttygen_disponible();
    // Trouvé par l'audit du 7 septembre 2026 : dix sessions partageant la même
    // `.ppk` (le cas courant, une clé pour tous les serveurs) ne convertissaient
    // la clé que pour le premier hôte. `convertir_ppk` refuse d'écraser le
    // fichier déjà écrit, donc les hôtes suivants tombaient en avertissement et
    // étaient enregistrés sans `IdentityFile`. On mémorise ici la conversion et
    // on réutilise la clé pour tout hôte suivant qui cite la même `.ppk`. La
    // table est clée sur le chemin **source** de la `.ppk`, jamais sur la tige :
    // deux `.ppk` distinctes de même nom (`C:\a\cle.ppk`, `C:\b\cle.ppk`) ne
    // doivent surtout pas se voir attribuer la même clé.
    let mut deja_converties: std::collections::HashMap<std::path::PathBuf, std::path::PathBuf> =
        std::collections::HashMap::new();
    for HoteAImporter { mut host, ppk } in hosts {
        host.alias =
            avash::import::alias_libre(&avash::import::alias_depuis_nom(&host.alias), &pris);
        if let Some(ppk) = ppk.filter(|p| !p.trim().is_empty()) {
            let source = std::path::PathBuf::from(&ppk);
            if let Some(cle) = deja_converties.get(&source) {
                host.identity_file = Some(cle.display().to_string());
            } else {
                match dossier_cles.as_deref() {
                    Some(dir) if puttygen => match avash::import::convertir_ppk(&source, dir) {
                        Ok(cle) => {
                            host.identity_file = Some(cle.display().to_string());
                            deja_converties.insert(source, cle);
                            bilan.cles_converties += 1;
                        }
                        Err(e) => bilan
                            .avertissements
                            .push(format!("{} : clé non convertie ({e:#})", host.alias)),
                    },
                    _ => bilan.avertissements.push(format!(
                        "{} : clé PuTTY non reprise (puttygen absent)",
                        host.alias
                    )),
                }
            }
        }
        // Trouvé par l'audit du 7 septembre 2026 : un `?` ici interrompait
        // l'import APRÈS avoir écrit les hôtes précédents (config non
        // inscriptible, collision d'alias, saut de ligne refusé). Le compte
        // était perdu, le front affichait « interrompu » et un second essai
        // renommait en `-2` les hôtes déjà écrits. Un échec devient un
        // avertissement et la boucle continue.
        match avash::append_host(&host) {
            Ok(()) => {
                pris.push(host.alias);
                bilan.hotes += 1;
            }
            Err(e) => bilan.avertissements.push(format!("{} : {e:#}", host.alias)),
        }
    }
    let chemin_bureaux = avash::rdphost::hosts_path();
    for b in bureaux {
        let mut h = avash::rdphost::RdpHost::new(&b.name, &b.host, b.port, &b.user, 1280, 800);
        h.folder = b.folder;
        // Même règle que pour les hôtes SSH : un bureau invalide (utilisateur
        // RDP vide, adresse à espace) ou une écriture ratée devient un
        // avertissement, jamais une interruption après une première écriture.
        // `upsert_host_in` revalide, ce qui couvre les deux cas d'un seul geste.
        match avash::rdphost::upsert_host_in(&chemin_bureaux, h) {
            Ok(_) => bilan.bureaux += 1,
            Err(e) => bilan.avertissements.push(format!("{} : {e:#}", b.name)),
        }
    }
    Ok(bilan)
}

#[cfg(test)]
mod tests_import {
    use super::{appliquer_import as import_apply, import_scan, HoteAImporter};
    use crate::commands::tests::with_ssh_config;

    /// Un répertoire `PuTTY` désigné est lu, les alias sont libres, et un hôte
    /// équivalent déjà déclaré est signalé comme doublon.
    #[test]
    fn scan_d_un_repertoire_putty_propose_des_alias_libres_et_signale_les_doublons() {
        let _g =
            with_ssh_config("Host prod-web\n  HostName 10.0.0.7\n  User adrien\n  Port 2222\n");
        let dir = std::env::temp_dir().join(format!("avash-import-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("prod%20web"),
            "HostName=10.0.0.7\nPortNumber=2222\nUserName=adrien\nProtocol=ssh\n",
        )
        .unwrap();
        std::fs::write(dir.join("db"), "HostName=10.0.0.9\nProtocol=ssh\n").unwrap();
        let bilan = import_scan(Some(dir.display().to_string())).unwrap();
        assert_eq!(bilan.candidats.len(), 2);
        let pw = bilan
            .candidats
            .iter()
            .find(|c| c.nom_origine == "prod web")
            .unwrap();
        assert_eq!(pw.host.alias, "prod-web-2", "l'alias existant est évité");
        assert_eq!(pw.doublon.as_deref(), Some("prod-web"));
        let db = bilan
            .candidats
            .iter()
            .find(|c| c.nom_origine == "db")
            .unwrap();
        assert!(db.doublon.is_none());
        assert_eq!(bilan.consultes, vec![dir.display().to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Trouvé par l'audit du 7 septembre 2026 : un hôte déclaré sans `HostName`
    /// (bloc `Host web.example.com` seul, donc `hostname: None`) n'était pas
    /// reconnu comme doublon d'une session `PuTTY` visant le même nom, faute du
    /// repli hostname-sinon-alias qu'applique déjà `hosts_health`. Le candidat
    /// était proposé coché et l'import créait une seconde entrée
    /// `web.example.com-2` pour le même serveur.
    #[test]
    fn scan_voit_le_doublon_d_un_hote_declare_sans_hostname() {
        let _g = with_ssh_config("Host web.example.com\n  User adrien\n");
        let dir =
            std::env::temp_dir().join(format!("avash-import-sanshost-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("web.example.com"),
            "HostName=web.example.com\nUserName=adrien\nProtocol=ssh\n",
        )
        .unwrap();
        let bilan = import_scan(Some(dir.display().to_string())).unwrap();
        assert_eq!(bilan.candidats.len(), 1);
        let c = &bilan.candidats[0];
        assert_eq!(c.doublon.as_deref(), Some("web.example.com"));
        assert_eq!(
            c.host.alias, "web.example.com-2",
            "l'alias libre est tout de même proposé"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Même cas, variante casse : la comparaison du repli est insensible à la
    /// casse (noms DNS insensibles), comme la branche bureaux et `append_host`.
    /// Un `Host WEB.example.com` / `HostName WEB.EXAMPLE.COM` doit reconnaître
    /// une session `PuTTY` visant `web.example.com`.
    #[test]
    fn scan_voit_le_doublon_sans_hostname_meme_en_casse_mixte() {
        let _g =
            with_ssh_config("Host WEB.example.com\n  HostName WEB.EXAMPLE.COM\n  User adrien\n");
        let dir = std::env::temp_dir().join(format!("avash-import-casse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("web.example.com"),
            "HostName=web.example.com\nUserName=adrien\nProtocol=ssh\n",
        )
        .unwrap();
        let bilan = import_scan(Some(dir.display().to_string())).unwrap();
        assert_eq!(bilan.candidats.len(), 1);
        assert_eq!(
            bilan.candidats[0].doublon.as_deref(),
            Some("WEB.example.com")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Trouvé par l'audit du 7 septembre 2026 : un `~/.ssh/config` illisible
    /// (non UTF-8) faisait partir `pris` vide, si bien qu'`import_apply`
    /// écrasait les collisions d'alias et collait les blocs à un fichier cru
    /// vide. L'import doit refuser proprement et ne rien écrire.
    #[test]
    fn apply_refuse_une_config_non_lisible_et_n_ecrit_rien() {
        let _g = with_ssh_config("");
        let path = avash::repertoire_personnel()
            .unwrap()
            .join(".ssh")
            .join("config");
        let octets = b"# R\xe9seau\nHost a\n    IdentityFile ~/.ssh/k";
        std::fs::write(&path, octets).unwrap();
        let hotes = vec![HoteAImporter {
            host: avash::SshHost {
                alias: "b".into(),
                hostname: Some("10.0.0.2".into()),
                ..Default::default()
            },
            ppk: None,
        }];
        let e = import_apply(hotes, Vec::new()).unwrap_err();
        assert!(e.contains("Impossible de lire"), "message inattendu : {e}");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            octets,
            "le fichier illisible a été modifié par un import qui aurait dû refuser"
        );
    }

    /// L'écriture passe par `append_host` : les hôtes se relisent, avec leur
    /// dossier, et une collision d'alias est résolue au lieu d'échouer.
    #[test]
    fn apply_ecrit_les_hotes_avec_leur_dossier_et_renomme_les_collisions() {
        let _g = with_ssh_config("Host db\n  HostName 10.0.0.9\n");
        let hotes = vec![
            HoteAImporter {
                host: avash::SshHost {
                    alias: "db".into(),
                    hostname: Some("10.0.0.10".into()),
                    folder: "Clients/Acme".into(),
                    ..Default::default()
                },
                ppk: None,
            },
            HoteAImporter {
                host: avash::SshHost {
                    alias: "web acme".into(),
                    hostname: Some("web.acme.fr".into()),
                    port: Some(2222),
                    ..Default::default()
                },
                ppk: None,
            },
        ];
        let bureaux = vec![avash::import::BureauImporte {
            source: avash::import::Source::MobaXterm,
            nom_origine: "Bureau".into(),
            name: "Bureau".into(),
            host: "10.0.0.9".into(),
            port: 3389,
            user: "adrien".into(),
            folder: "Clients".into(),
        }];
        let bilan = import_apply(hotes, bureaux).unwrap();
        assert_eq!(
            (bilan.hotes, bilan.bureaux, bilan.cles_converties),
            (2, 1, 0)
        );
        assert!(
            bilan.avertissements.is_empty(),
            "{:?}",
            bilan.avertissements
        );
        let bureaux_relus = avash::rdphost::load_hosts().unwrap();
        assert_eq!(bureaux_relus.len(), 1);
        assert_eq!(
            (
                bureaux_relus[0].name.as_str(),
                bureaux_relus[0].host.as_str(),
                bureaux_relus[0].folder.as_str()
            ),
            ("Bureau", "10.0.0.9", "Clients")
        );
        let relus = avash::parse_ssh_config().unwrap();
        let aliases: Vec<&str> = relus.iter().map(|h| h.alias.as_str()).collect();
        assert_eq!(aliases, vec!["db", "db-2", "web-acme"]);
        assert_eq!(relus[1].folder, "Clients/Acme");
        assert_eq!(relus[2].port, Some(2222));
    }

    /// Génère une vraie `.ppk` sans phrase de passe dans `dir`. Rend son chemin.
    #[cfg(unix)]
    fn ppk_de_test(dir: &std::path::Path, nom: &str) -> String {
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join(nom);
        // Sans `--new-passphrase`, puttygen demande une phrase au terminal ; un
        // fichier vide vaut « aucune » (même astuce que le test cœur).
        let gen = std::process::Command::new("puttygen")
            .args(["-t", "ed25519", "-q", "--new-passphrase", "/dev/null", "-o"])
            .arg(&p)
            .stdin(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(gen.success());
        p.display().to_string()
    }

    /// Trouvé par l'audit du 7 septembre 2026 : dix sessions partageant la même
    /// `.ppk` (une clé pour tous les serveurs) ne voyaient la clé convertie que
    /// pour le premier hôte, les autres étant écrits sans `IdentityFile` avec un
    /// avertissement chacun. La clé convertie est désormais réutilisée.
    #[cfg(unix)]
    #[test]
    fn une_meme_ppk_partagee_est_convertie_une_fois_et_reutilisee() {
        if !avash::import::puttygen_disponible() {
            eprintln!("puttygen absent : conversion partagée non testée ici");
            return;
        }
        let _g = with_ssh_config("");
        let dir = std::env::temp_dir().join(format!("avash-ppk-partagee-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ppk = ppk_de_test(&dir, "cle.ppk");
        let hote = |alias: &str| HoteAImporter {
            host: avash::SshHost {
                alias: alias.into(),
                hostname: Some("10.0.0.1".into()),
                ..Default::default()
            },
            ppk: Some(ppk.clone()),
        };
        let bilan = import_apply(vec![hote("un"), hote("deux")], Vec::new()).unwrap();
        assert_eq!(
            bilan.cles_converties, 1,
            "une seule conversion pour la clé partagée"
        );
        assert!(
            bilan.avertissements.is_empty(),
            "{:?}",
            bilan.avertissements
        );
        let relus = avash::parse_ssh_config().unwrap();
        let ids: Vec<Option<&str>> = relus.iter().map(|h| h.identity_file.as_deref()).collect();
        assert_eq!(ids.len(), 2, "{ids:?}");
        assert!(
            ids[0].is_some_and(|s| !s.is_empty()),
            "le premier hôte a bien la clé : {ids:?}"
        );
        assert_eq!(
            ids[0], ids[1],
            "les deux hôtes pointent la même clé convertie : {ids:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Deux `.ppk` distinctes de même nom de fichier ne doivent jamais faire
    /// pointer deux hôtes vers la même clé : la mémoire d'import est clée sur le
    /// chemin source. La seconde ne pouvant s'écrire sans écraser la première
    /// (même tige, même destination), elle est signalée, jamais partagée.
    #[cfg(unix)]
    #[test]
    fn deux_ppk_de_meme_nom_mais_de_sources_distinctes_ne_partagent_pas_la_cle() {
        if !avash::import::puttygen_disponible() {
            eprintln!("puttygen absent : test non joué ici");
            return;
        }
        let _g = with_ssh_config("");
        let base = std::env::temp_dir().join(format!("avash-ppk-tige-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let ppk_a = ppk_de_test(&base.join("a"), "cle.ppk");
        let ppk_b = ppk_de_test(&base.join("b"), "cle.ppk");
        let hotes = vec![
            HoteAImporter {
                host: avash::SshHost {
                    alias: "prod".into(),
                    hostname: Some("10.0.0.1".into()),
                    ..Default::default()
                },
                ppk: Some(ppk_a),
            },
            HoteAImporter {
                host: avash::SshHost {
                    alias: "dev".into(),
                    hostname: Some("10.0.0.2".into()),
                    ..Default::default()
                },
                ppk: Some(ppk_b),
            },
        ];
        let bilan = import_apply(hotes, Vec::new()).unwrap();
        // Une seule des deux clés peut s'écrire (même tige = même destination) :
        // l'autre est signalée, jamais silencieusement partagée.
        assert_eq!(bilan.cles_converties, 1, "{bilan:?}");
        assert_eq!(bilan.avertissements.len(), 1, "{:?}", bilan.avertissements);
        let relus = avash::parse_ssh_config().unwrap();
        let ids: Vec<Option<String>> = relus.iter().map(|h| h.identity_file.clone()).collect();
        assert!(
            ids[0].as_deref().is_some_and(|s| !s.is_empty()),
            "le premier hôte a sa clé : {ids:?}"
        );
        assert!(
            ids[1].is_none(),
            "le second hôte ne récupère pas la clé du premier : {ids:?}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Trouvé par l'audit du 7 septembre 2026 : un bureau `MobaXterm` sans
    /// utilisateur (signet RDP qui demande les identifiants à la connexion, cas
    /// ordinaire, refusé par `RdpHost::validate`) faisait échouer `import_apply`
    /// APRÈS l'écriture des hôtes SSH. L'erreur remontait, le front laissait la
    /// modale ouverte, et un second clic renommait les hôtes déjà écrits (alias
    /// suffixés `-2`) : cinq doublons dans `~/.ssh/config`. Un bureau invalide
    /// devient désormais un avertissement et la boucle continue.
    #[test]
    fn un_bureau_sans_utilisateur_n_interrompt_pas_l_import() {
        let _g = with_ssh_config("");
        let hotes = vec![HoteAImporter {
            host: avash::SshHost {
                alias: "web".into(),
                hostname: Some("10.0.0.7".into()),
                ..Default::default()
            },
            ppk: None,
        }];
        let bureaux = vec![avash::import::BureauImporte {
            source: avash::import::Source::MobaXterm,
            nom_origine: "Bureau".into(),
            name: "Bureau".into(),
            host: "10.0.0.9".into(),
            port: 3389,
            user: String::new(),
            folder: String::new(),
        }];
        let bilan = import_apply(hotes, bureaux).unwrap();
        assert_eq!(bilan.hotes, 1, "l'hôte SSH est bien compté");
        assert_eq!(
            bilan.bureaux, 0,
            "le bureau sans utilisateur n'est pas écrit"
        );
        assert_eq!(bilan.avertissements.len(), 1, "{:?}", bilan.avertissements);
        assert!(
            bilan.avertissements[0].contains("Bureau"),
            "l'avertissement nomme le bureau : {:?}",
            bilan.avertissements
        );
        let relus = avash::parse_ssh_config().unwrap();
        assert_eq!(relus.len(), 1, "l'hôte valide est écrit une fois");
        assert!(
            avash::rdphost::load_hosts().unwrap().is_empty(),
            "aucun bureau invalide n'est enregistré"
        );
    }

    /// Même porte que le bureau sans utilisateur : `validate` refuse aussi une
    /// adresse à espace (elle casserait la clé du fichier d'empreintes RDP, donc
    /// le TOFU). Le parseur ne filtrant que l'hôte vide, un tel signet arrivait
    /// jusqu'à l'écriture et interrompait l'import de la même façon.
    #[test]
    fn un_bureau_a_l_adresse_invalide_n_interrompt_pas_l_import() {
        let _g = with_ssh_config("");
        let hotes = vec![HoteAImporter {
            host: avash::SshHost {
                alias: "web".into(),
                hostname: Some("10.0.0.7".into()),
                ..Default::default()
            },
            ppk: None,
        }];
        let bureaux = vec![avash::import::BureauImporte {
            source: avash::import::Source::MobaXterm,
            nom_origine: "Bureau".into(),
            name: "Bureau".into(),
            host: "mon serveur".into(),
            port: 3389,
            user: "adrien".into(),
            folder: String::new(),
        }];
        let bilan = import_apply(hotes, bureaux).unwrap();
        assert_eq!((bilan.hotes, bilan.bureaux), (1, 0));
        assert_eq!(bilan.avertissements.len(), 1, "{:?}", bilan.avertissements);
        assert!(
            bilan.avertissements[0].contains("Bureau"),
            "{:?}",
            bilan.avertissements
        );
    }

    /// La boucle SSH interrompait aussi l'import au premier hôte refusé
    /// (`append_host` rejette un saut de ligne dans le hostname, tentative
    /// d'injection de directive), perdant le compte des hôtes déjà écrits et
    /// rejouant la même duplication au second essai. Un hôte refusé devient un
    /// avertissement, les suivants sont quand même écrits.
    #[test]
    fn un_hote_ssh_refuse_n_interrompt_pas_les_suivants() {
        let _g = with_ssh_config("");
        let hotes = vec![
            HoteAImporter {
                host: avash::SshHost {
                    alias: "mauvais".into(),
                    hostname: Some("10.0.0.1\nProxyCommand touch /tmp/injecte".into()),
                    ..Default::default()
                },
                ppk: None,
            },
            HoteAImporter {
                host: avash::SshHost {
                    alias: "bon".into(),
                    hostname: Some("10.0.0.2".into()),
                    ..Default::default()
                },
                ppk: None,
            },
        ];
        let bilan = import_apply(hotes, Vec::new()).unwrap();
        assert_eq!(bilan.hotes, 1, "seul l'hôte valide est écrit");
        assert_eq!(bilan.avertissements.len(), 1, "{:?}", bilan.avertissements);
        let relus = avash::parse_ssh_config().unwrap();
        let aliases: Vec<&str> = relus.iter().map(|h| h.alias.as_str()).collect();
        assert_eq!(
            aliases,
            vec!["bon"],
            "le mauvais hôte est ignoré, pas écrit"
        );
    }

    /// Trouvé par l'audit du 7 septembre 2026 : un signet RDP sans utilisateur
    /// traversait le scan sans marque et était proposé coché, puis interrompait
    /// l'import à l'écriture. Le scan porte désormais le défaut sur le candidat.
    #[test]
    fn un_signet_rdp_sans_utilisateur_est_signale_au_scan() {
        let _g = with_ssh_config("");
        let fichier =
            std::env::temp_dir().join(format!("avash-scan-bureau-{}.ini", std::process::id()));
        std::fs::write(&fichier, "[Bookmarks]\nBureau=#91#4%10.0.0.9%3389%%\n").unwrap();
        let bilan = import_scan(Some(fichier.display().to_string())).unwrap();
        assert_eq!(bilan.bureaux.len(), 1);
        let b = &bilan.bureaux[0];
        assert!(b.doublon.is_none(), "aucun bureau existant ici");
        assert_eq!(
            b.remarques.len(),
            1,
            "le défaut est signalé : {:?}",
            b.remarques
        );
        assert!(
            b.remarques[0].contains("utilisateur"),
            "la remarque cite l'utilisateur manquant : {:?}",
            b.remarques
        );
        let _ = std::fs::remove_file(&fichier);
    }

    #[test]
    fn un_chemin_illisible_est_une_erreur_claire() {
        let e = import_scan(Some("/nulle/part/MobaXterm.ini".into())).unwrap_err();
        assert!(e.contains("Lecture de"), "{e}");
    }
}
