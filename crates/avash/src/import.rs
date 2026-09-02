//! Import de sessions depuis `PuTTY` et `MobaXterm`.
//!
//! L'argument d'adoption le plus direct : un utilisateur qui retrouve ses
//! connexions sans les ressaisir reste. Les deux formats sont lisibles :
//!
//! - **`PuTTY`** range une session par fichier `clé=valeur` dans
//!   `~/.putty/sessions/` (nom de fichier encodé en `%XX`), ou par clé de
//!   registre sous `HKCU\Software\SimonTatham\PuTTY\Sessions` sous Windows,
//!   avec les mêmes noms de valeurs (`HostName`, `PortNumber`, `UserName`,
//!   `PublicKeyFile`, `Protocol`).
//! - **`MobaXterm`** garde tout dans `MobaXterm.ini`, sections `[Bookmarks]` et
//!   `[Bookmarks_N]` (dossier dans `SubRep`), une ligne par session :
//!   `Nom=#109#0%hôte%port%utilisateur%…` pour SSH — le champ 14 porte la clé
//!   privée. Les autres types (`#91` RDP, `#98` telnet) sont comptés, pas
//!   repris.
//!
//! Rien ici n'écrit : on rend des candidats, l'interface les montre, et
//! `append_host` fait le reste. Les clés `.ppk` ne sont pas reprises — OpenSSH
//! ne les lit pas — et le candidat le dit.

use crate::SshHost;
use std::path::Path;

/// D'où vient une session importable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Putty,
    MobaXterm,
}

/// Une session lue chez un autre outil, prête à être proposée.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionImportee {
    pub source: Source,
    /// Le nom tel que l'outil d'origine l'affiche.
    pub nom_origine: String,
    pub host: SshHost,
    /// Ce qui n'a pas pu être repris (clé `.ppk`, mandataire…), en clair.
    pub remarques: Vec<String>,
}

/// Bilan d'une lecture : les sessions reprises, et ce qui a été laissé.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct Lecture {
    pub sessions: Vec<SessionImportee>,
    /// Sessions d'un autre protocole (RDP, telnet, série…), non reprises.
    pub ignorees: usize,
}

// ---------- Alias ----------

/// Transforme un nom de session en alias acceptable pour `~/.ssh/config` :
/// pas d'espace, de joker ni de saut de ligne. Les espaces deviennent des
/// tirets, le reste des caractères interdits est retiré.
#[must_use]
pub fn alias_depuis_nom(nom: &str) -> String {
    let mut alias = String::new();
    for c in nom.trim().chars() {
        match c {
            c if c.is_whitespace() => alias.push('-'),
            '*' | '?' | '!' | '\n' | '\r' | '\0' | '"' | '\'' | '\\' => {}
            c => alias.push(c),
        }
    }
    // Deux tirets à la suite ne disent rien de plus qu'un seul.
    while alias.contains("--") {
        alias = alias.replace("--", "-");
    }
    alias.trim_matches('-').to_string()
}

/// Un alias qui n'entre en collision avec aucun de ceux déjà pris : le nom
/// tel quel, sinon `nom-2`, `nom-3`…
#[must_use]
pub fn alias_libre(base: &str, pris: &[String]) -> String {
    let base = if base.is_empty() { "importe" } else { base };
    if !pris.iter().any(|p| p == base) {
        return base.to_string();
    }
    (2..=u32::MAX)
        .map(|n| format!("{base}-{n}"))
        .find(|c| !pris.iter().any(|p| p == c))
        .expect("quatre milliards de suffixes ne sont jamais tous pris")
}

fn clé_reprise(chemin: &str, remarques: &mut Vec<String>) -> Option<String> {
    let chemin = chemin.trim();
    if chemin.is_empty() {
        return None;
    }
    if chemin.to_ascii_lowercase().ends_with(".ppk") {
        remarques.push(format!(
            "Clé PuTTY non reprise ({chemin}) : OpenSSH ne lit pas le format .ppk, convertissez-la avec puttygen."
        ));
        return None;
    }
    Some(chemin.to_string())
}

// ---------- PuTTY ----------

/// Décode un nom de fichier de session `PuTTY` (`prod%20web` → `prod web`).
#[must_use]
pub fn decoder_nom_putty(nom: &str) -> String {
    let octets = nom.as_bytes();
    let mut out = Vec::with_capacity(octets.len());
    let mut i = 0;
    while i < octets.len() {
        if octets[i] == b'%' && i + 2 < octets.len() {
            if let Ok(v) = u8::from_str_radix(&nom[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(octets[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Lit une session `PuTTY` depuis ses paires `clé=valeur` (fichier
/// `~/.putty/sessions/<nom>`). Rend `None` si ce n'est pas une session SSH ou
/// si l'hôte manque.
#[must_use]
pub fn parse_putty_session(nom: &str, contenu: &str) -> Option<SessionImportee> {
    let paires = contenu.lines().filter_map(|l| {
        let (k, v) = l.split_once('=')?;
        Some((k.trim(), v.trim()))
    });
    session_putty_depuis(nom, paires)
}

/// Le cœur commun aux deux supports (fichier, registre).
fn session_putty_depuis<'a>(
    nom: &str,
    paires: impl Iterator<Item = (&'a str, &'a str)>,
) -> Option<SessionImportee> {
    let mut hostname = None;
    let mut port = None;
    let mut user = None;
    let mut cle = None;
    let mut protocole = None;
    let mut mandataire = None;
    for (k, v) in paires {
        match k {
            "HostName" => hostname = Some(v.to_string()),
            "PortNumber" => port = v.parse::<u16>().ok(),
            "UserName" => user = Some(v.to_string()),
            "PublicKeyFile" => cle = Some(v.to_string()),
            "Protocol" => protocole = Some(v.to_ascii_lowercase()),
            "ProxyMethod" => mandataire = v.parse::<u32>().ok(),
            _ => {}
        }
    }
    // « Default Settings » n'est pas une session ; un protocole autre que SSH
    // ne nous concerne pas.
    if nom == "Default Settings" || protocole.as_deref().is_some_and(|p| p != "ssh") {
        return None;
    }
    let hostname = hostname.filter(|h| !h.is_empty())?;
    let mut remarques = Vec::new();
    // PuTTY accepte `user@hôte` dans le champ hôte.
    let (user, hostname) = match hostname.split_once('@') {
        Some((u, h)) if user.as_deref().is_none_or(str::is_empty) => {
            (Some(u.to_string()), h.to_string())
        }
        _ => (user, hostname),
    };
    if mandataire.is_some_and(|m| m != 0) {
        remarques.push("Mandataire PuTTY non repris : à traduire en ProxyJump à la main.".into());
    }
    let identity_file = cle.and_then(|c| clé_reprise(&c, &mut remarques));
    Some(SessionImportee {
        source: Source::Putty,
        nom_origine: nom.to_string(),
        host: SshHost {
            alias: alias_depuis_nom(nom),
            hostname: Some(hostname),
            user: user.filter(|u| !u.is_empty()),
            port: port.filter(|p| *p != 0),
            identity_file,
            ..SshHost::default()
        },
        remarques,
    })
}

/// Toutes les sessions d'un répertoire `~/.putty/sessions`.
#[must_use]
pub fn putty_sessions_dans(dir: &Path) -> Lecture {
    let mut lecture = Lecture::default();
    let Ok(entrees) = std::fs::read_dir(dir) else {
        return lecture;
    };
    let mut fichiers: Vec<_> = entrees.flatten().map(|e| e.path()).collect();
    fichiers.sort();
    for chemin in fichiers {
        let Some(nom) = chemin.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Ok(contenu) = std::fs::read_to_string(&chemin) else {
            continue;
        };
        let nom = decoder_nom_putty(nom);
        match parse_putty_session(&nom, &contenu) {
            Some(s) => lecture.sessions.push(s),
            None if nom != "Default Settings" => lecture.ignorees += 1,
            None => {}
        }
    }
    lecture
}

/// Emplacement des sessions `PuTTY` sous Unix.
#[must_use]
pub fn repertoire_putty() -> Option<std::path::PathBuf> {
    crate::repertoire_personnel().map(|h| h.join(".putty").join("sessions"))
}

/// Lit la sortie de `reg query HKCU\Software\SimonTatham\PuTTY\Sessions /s`,
/// telle que Windows l'imprime : une ligne par clé, puis ses valeurs
/// indentées (`Nom    REG_SZ    valeur`, `REG_DWORD` en hexadécimal).
#[must_use]
pub fn parse_reg_query(sortie: &str) -> Lecture {
    let mut lecture = Lecture::default();
    let mut courante: Option<(String, Vec<(String, String)>)> = None;
    let clore = |courante: &mut Option<(String, Vec<(String, String)>)>, lecture: &mut Lecture| {
        if let Some((nom, paires)) = courante.take() {
            let paires: Vec<(&str, &str)> = paires
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            match session_putty_depuis(&nom, paires.into_iter()) {
                Some(s) => lecture.sessions.push(s),
                None if nom != "Default Settings" => lecture.ignorees += 1,
                None => {}
            }
        }
    };
    for ligne in sortie.lines() {
        if ligne.is_empty() {
            continue;
        }
        if !ligne.starts_with(' ') {
            clore(&mut courante, &mut lecture);
            // La clé racine elle-même n'a pas de nom de session.
            let Some((_, nom)) = ligne.rsplit_once("\\Sessions\\") else {
                continue;
            };
            courante = Some((decoder_nom_putty(nom.trim()), Vec::new()));
            continue;
        }
        let Some((_, paires)) = courante.as_mut() else {
            continue;
        };
        let champs: Vec<&str> = ligne.trim().splitn(3, "    ").collect();
        if champs.len() < 3 {
            continue;
        }
        let (nom, kind, valeur) = (champs[0].trim(), champs[1].trim(), champs[2].trim());
        let valeur = if kind == "REG_DWORD" {
            u32::from_str_radix(valeur.trim_start_matches("0x"), 16)
                .map(|v| v.to_string())
                .unwrap_or_default()
        } else {
            valeur.to_string()
        };
        paires.push((nom.to_string(), valeur));
    }
    clore(&mut courante, &mut lecture);
    lecture
}

/// Les sessions `PuTTY` du registre Windows, par `reg query`.
#[cfg(windows)]
#[must_use]
pub fn putty_sessions_registre() -> Lecture {
    let sortie = std::process::Command::new("reg")
        .args(["query", r"HKCU\Software\SimonTatham\PuTTY\Sessions", "/s"])
        .output();
    match sortie {
        Ok(o) if o.status.success() => parse_reg_query(&String::from_utf8_lossy(&o.stdout)),
        _ => Lecture::default(),
    }
}

// ---------- MobaXterm ----------

/// Le chemin d'une clé tel que `MobaXterm` l'écrit : `_CurrentDrive_` pour le
/// lecteur système, antislashs Windows.
fn chemin_moba(brut: &str) -> String {
    brut.replace("_CurrentDrive_", "C").trim().to_string()
}

/// Lit un `MobaXterm.ini` (ou un export `.mxtsessions`, même forme).
#[must_use]
pub fn parse_mobaxterm_ini(contenu: &str) -> Lecture {
    let mut lecture = Lecture::default();
    let mut dans_signets = false;
    let mut dossier = String::new();
    for ligne in contenu.lines() {
        let ligne = ligne.trim_end_matches('\r');
        if let Some(section) = ligne.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            dans_signets = section == "Bookmarks" || section.starts_with("Bookmarks_");
            dossier.clear();
            continue;
        }
        if !dans_signets || ligne.trim().is_empty() {
            continue;
        }
        let Some((nom, valeur)) = ligne.split_once('=') else {
            continue;
        };
        match nom {
            "SubRep" => dossier = valeur.trim().replace('\\', "/"),
            "ImgNum" => {}
            _ => {
                let Some(reste) = valeur.strip_prefix('#') else {
                    continue;
                };
                let Some((type_session, champs)) = reste.split_once('#') else {
                    continue;
                };
                if type_session != "109" {
                    lecture.ignorees += 1;
                    continue;
                }
                let champs: Vec<&str> = champs.split('#').next().unwrap_or("").split('%').collect();
                let hostname = champs.get(1).map(|s| s.trim()).unwrap_or_default();
                if hostname.is_empty() {
                    lecture.ignorees += 1;
                    continue;
                }
                let mut remarques = Vec::new();
                let identity_file = champs
                    .get(14)
                    .filter(|s| !s.trim().is_empty())
                    .map(|s| chemin_moba(s))
                    .and_then(|c| clé_reprise(&c, &mut remarques));
                if champs
                    .get(19)
                    .is_some_and(|p| p.trim() != "0" && !p.trim().is_empty())
                {
                    remarques.push(
                        "Mandataire MobaXterm non repris : à traduire en ProxyJump à la main."
                            .into(),
                    );
                }
                let nom = nom.trim();
                lecture.sessions.push(SessionImportee {
                    source: Source::MobaXterm,
                    nom_origine: nom.to_string(),
                    host: SshHost {
                        alias: alias_depuis_nom(nom),
                        hostname: Some(hostname.to_string()),
                        user: champs
                            .get(3)
                            .map(|s| s.trim())
                            .filter(|s| !s.is_empty())
                            .map(str::to_string),
                        port: champs
                            .get(2)
                            .and_then(|p| p.trim().parse::<u16>().ok())
                            .filter(|p| *p != 0),
                        identity_file,
                        folder: dossier.clone(),
                        ..SshHost::default()
                    },
                    remarques,
                });
            }
        }
    }
    lecture
}

/// Emplacements habituels de `MobaXterm.ini` (Windows) ; sous Unix, l'outil
/// n'existe pas et le fichier est à désigner.
#[must_use]
pub fn chemins_mobaxterm() -> Vec<std::path::PathBuf> {
    let mut v = Vec::new();
    if let Some(appdata) = std::env::var_os("APPDATA") {
        v.push(
            std::path::PathBuf::from(appdata)
                .join("MobaXterm")
                .join("MobaXterm.ini"),
        );
    }
    if let Some(docs) = dirs::document_dir() {
        v.push(docs.join("MobaXterm").join("MobaXterm.ini"));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn un_nom_de_session_devient_un_alias_sur() {
        assert_eq!(alias_depuis_nom("prod web"), "prod-web");
        assert_eq!(alias_depuis_nom("  serveur  *test?  "), "serveur-test");
        assert_eq!(
            alias_depuis_nom("a\nb"),
            "a-b",
            "un saut de ligne vaut un espace"
        );
        assert_eq!(
            alias_libre("prod", &["prod".into(), "prod-2".into()]),
            "prod-3"
        );
        assert_eq!(alias_libre("", &[]), "importe");
    }

    #[test]
    fn le_nom_de_fichier_putty_est_decode() {
        assert_eq!(decoder_nom_putty("prod%20web"), "prod web");
        assert_eq!(decoder_nom_putty("Default%20Settings"), "Default Settings");
        assert_eq!(
            decoder_nom_putty("100%"),
            "100%",
            "un % isolé reste tel quel"
        );
        assert_eq!(decoder_nom_putty("caf%C3%A9"), "café");
    }

    #[test]
    fn une_session_putty_donne_un_hote_complet() {
        let s = parse_putty_session(
            "prod web",
            "HostName=10.0.0.7\nPortNumber=2222\nUserName=adrien\nProtocol=ssh\nPublicKeyFile=/home/a/.ssh/id_ed25519\nTermType=xterm\n",
        )
        .unwrap();
        assert_eq!(s.source, Source::Putty);
        assert_eq!(s.host.alias, "prod-web");
        assert_eq!(s.host.hostname.as_deref(), Some("10.0.0.7"));
        assert_eq!(s.host.port, Some(2222));
        assert_eq!(s.host.user.as_deref(), Some("adrien"));
        assert_eq!(
            s.host.identity_file.as_deref(),
            Some("/home/a/.ssh/id_ed25519")
        );
        assert!(s.remarques.is_empty());
    }

    #[test]
    fn putty_ppk_et_mandataire_sont_signales_pas_repris() {
        let s = parse_putty_session(
            "bastion",
            "HostName=adrien@bastion.exemple.net\nProtocol=ssh\nPublicKeyFile=C:\\Users\\a\\cle.ppk\nProxyMethod=5\n",
        )
        .unwrap();
        assert_eq!(s.host.user.as_deref(), Some("adrien"), "user@hôte de PuTTY");
        assert_eq!(s.host.hostname.as_deref(), Some("bastion.exemple.net"));
        assert!(s.host.identity_file.is_none());
        assert_eq!(s.remarques.len(), 2, "{:?}", s.remarques);
        assert!(
            s.remarques.iter().any(|r| r.contains(".ppk")),
            "{:?}",
            s.remarques
        );
    }

    #[test]
    fn putty_ignore_les_reglages_par_defaut_et_les_autres_protocoles() {
        assert!(parse_putty_session("Default Settings", "HostName=x\n").is_none());
        assert!(parse_putty_session("telnet", "HostName=x\nProtocol=telnet\n").is_none());
        assert!(parse_putty_session("vide", "Protocol=ssh\n").is_none());
    }

    #[test]
    fn un_repertoire_putty_est_parcouru_dans_l_ordre() {
        let dir = std::env::temp_dir().join(format!("avash-putty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Default%20Settings"), "HostName=\n").unwrap();
        std::fs::write(dir.join("b%20serveur"), "HostName=b.local\nProtocol=ssh\n").unwrap();
        std::fs::write(dir.join("a"), "HostName=a.local\n").unwrap();
        std::fs::write(dir.join("serie"), "Protocol=serial\nSerialLine=COM1\n").unwrap();
        let l = putty_sessions_dans(&dir);
        let noms: Vec<&str> = l.sessions.iter().map(|s| s.nom_origine.as_str()).collect();
        assert_eq!(noms, vec!["a", "b serveur"]);
        assert_eq!(l.ignorees, 1, "la session série est comptée, pas reprise");
        assert!(putty_sessions_dans(&dir.join("absent")).sessions.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn la_sortie_de_reg_query_est_relue() {
        let sortie = "\r\nHKEY_CURRENT_USER\\Software\\SimonTatham\\PuTTY\\Sessions\r\n\r\nHKEY_CURRENT_USER\\Software\\SimonTatham\\PuTTY\\Sessions\\prod%20web\r\n    HostName    REG_SZ    10.0.0.7\r\n    PortNumber    REG_DWORD    0x8ae\r\n    UserName    REG_SZ    adrien\r\n    Protocol    REG_SZ    ssh\r\n    PublicKeyFile    REG_SZ    C:\\Users\\a\\cle.ppk\r\n\r\nHKEY_CURRENT_USER\\Software\\SimonTatham\\PuTTY\\Sessions\\Default%20Settings\r\n    HostName    REG_SZ    \r\n\r\nHKEY_CURRENT_USER\\Software\\SimonTatham\\PuTTY\\Sessions\\raw\r\n    HostName    REG_SZ    1.2.3.4\r\n    Protocol    REG_SZ    raw\r\n";
        let l = parse_reg_query(sortie);
        assert_eq!(l.sessions.len(), 1, "{l:?}");
        let s = &l.sessions[0];
        assert_eq!(s.nom_origine, "prod web");
        assert_eq!(s.host.port, Some(2222), "REG_DWORD hexadécimal");
        assert_eq!(s.host.user.as_deref(), Some("adrien"));
        assert!(s.host.identity_file.is_none());
        assert_eq!(l.ignorees, 1);
    }

    const MOBA: &str = "[Bookmarks]\r\nSubRep=\r\nImgNum=42\r\nDeck=#109#0%192.168.137.40%22%deck%%0%0%%%%%0%0%0%%%-1%0%0%0%%1080%%0%0%1%#MobaFont%10%0%0%-1%15%236,236,236%30,30,30%180,180,192%0%-1%0%%xterm%-1%0%_Std_Colors_0_%80%24%0%1%-1%<none>%%0%0%-1%-1#0# #-1\r\nBureau=#91#4%10.0.0.9%3389%adrien%...\r\n\r\n[Bookmarks_1]\r\nSubRep=Clients\\Acme\r\nImgNum=41\r\nweb acme=#109#0%web.acme.fr%2222%%%-1%-1%%%%%0%0%0%_CurrentDrive_:\\Users\\a\\.ssh\\id_ed25519%%-1%0%0%0%%1080%%0%0%1%#MobaFont%10#0# #-1\r\n";

    #[test]
    fn un_ini_mobaxterm_donne_les_sessions_ssh_avec_leur_dossier() {
        let l = parse_mobaxterm_ini(MOBA);
        assert_eq!(l.sessions.len(), 2, "{l:?}");
        assert_eq!(l.ignorees, 1, "le bureau RDP est compté");
        let d = &l.sessions[0];
        assert_eq!(d.source, Source::MobaXterm);
        assert_eq!(
            (
                d.host.alias.as_str(),
                d.host.hostname.as_deref(),
                d.host.port,
                d.host.user.as_deref()
            ),
            ("Deck", Some("192.168.137.40"), Some(22), Some("deck"))
        );
        assert_eq!(d.host.folder, "");
        let w = &l.sessions[1];
        assert_eq!(w.nom_origine, "web acme");
        assert_eq!(w.host.alias, "web-acme");
        assert_eq!(w.host.port, Some(2222));
        assert!(w.host.user.is_none());
        assert_eq!(w.host.folder, "Clients/Acme");
        assert_eq!(
            w.host.identity_file.as_deref(),
            Some("C:\\Users\\a\\.ssh\\id_ed25519")
        );
    }

    #[test]
    fn un_ini_sans_signets_ne_donne_rien() {
        let l = parse_mobaxterm_ini(
            "[Misc]\nTruc=1\n[Bookmarks]\nSubRep=\nImgNum=42\nVide=#109#0%%22%%\n",
        );
        assert!(l.sessions.is_empty());
        assert_eq!(l.ignorees, 1);
    }
}
