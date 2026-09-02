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
#[tauri::command]
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
    let existants = avash::parse_ssh_config().unwrap_or_default();
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
            bureaux.push(BureauCandidat { bureau: b, doublon });
        }
        for s in lecture.sessions {
            let mut host = s.host;
            host.alias = avash::import::alias_libre(&host.alias, &pris);
            pris.push(host.alias.clone());
            let doublon = existants
                .iter()
                .find(|e| {
                    e.hostname == host.hostname
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
#[tauri::command]
pub fn import_apply(
    hosts: Vec<HoteAImporter>,
    bureaux: Vec<avash::import::BureauImporte>,
) -> Result<BilanApply, String> {
    let mut pris: Vec<String> = avash::parse_ssh_config()
        .unwrap_or_default()
        .into_iter()
        .map(|h| h.alias)
        .collect();
    let mut bilan = BilanApply::default();
    let dossier_cles = avash::repertoire_personnel().map(|h| h.join(".ssh"));
    for HoteAImporter { mut host, ppk } in hosts {
        host.alias =
            avash::import::alias_libre(&avash::import::alias_depuis_nom(&host.alias), &pris);
        if let Some(ppk) = ppk.filter(|p| !p.trim().is_empty()) {
            match dossier_cles.as_deref() {
                Some(dir) if avash::import::puttygen_disponible() => {
                    match avash::import::convertir_ppk(std::path::Path::new(&ppk), dir) {
                        Ok(cle) => {
                            host.identity_file = Some(cle.display().to_string());
                            bilan.cles_converties += 1;
                        }
                        Err(e) => bilan
                            .avertissements
                            .push(format!("{} : clé non convertie ({e:#})", host.alias)),
                    }
                }
                _ => bilan.avertissements.push(format!(
                    "{} : clé PuTTY non reprise (puttygen absent)",
                    host.alias
                )),
            }
        }
        avash::append_host(&host).map_err(|e| format!("{} : {e:#}", host.alias))?;
        pris.push(host.alias);
        bilan.hotes += 1;
    }
    let chemin_bureaux = avash::rdphost::hosts_path();
    for b in bureaux {
        let mut h = avash::rdphost::RdpHost::new(&b.name, &b.host, b.port, &b.user, 1280, 800);
        h.folder = b.folder;
        h.validate().map_err(|e| format!("{} : {e:#}", b.name))?;
        avash::rdphost::upsert_host_in(&chemin_bureaux, h)
            .map_err(|e| format!("{} : {e:#}", b.name))?;
        bilan.bureaux += 1;
    }
    Ok(bilan)
}

#[cfg(test)]
mod tests_import {
    use super::{import_apply, import_scan, HoteAImporter};
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

    #[test]
    fn un_chemin_illisible_est_une_erreur_claire() {
        let e = import_scan(Some("/nulle/part/MobaXterm.ini".into())).unwrap_err();
        assert!(e.contains("Lecture de"), "{e}");
    }
}
