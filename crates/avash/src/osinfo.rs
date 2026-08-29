//! Identification du systeme distant, pour afficher son logo dans la liste
//! des hotes. Source : `/etc/os-release` (Linux), `uname -s` a defaut
//! (BSD, macOS), `ver` sur Windows.

use serde::Serialize;

/// Commande envoyee sur un canal exec separe, juste apres l'ouverture d'une
/// session. `||` est compris par sh comme par cmd.exe.
pub const PROBE_COMMAND: &str = "cat /etc/os-release 2>/dev/null || uname -s 2>/dev/null || ver";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct OsInfo {
    /// `ID` d'os-release (`arch`, `debian`, `ubuntu`…), ou un nom derive
    /// (`darwin`, `freebsd`, `windows`). Toujours en minuscules.
    pub id: String,
    /// `ID_LIKE` : familles dont derive la distribution (`cachyos` -> `arch`).
    pub like: Vec<String>,
    /// `PRETTY_NAME`, ou ce qu'on a de plus lisible.
    pub pretty: String,
}

/// Interprete la sortie de [`PROBE_COMMAND`]. Ne rend `None` que si rien
/// n'est exploitable.
#[must_use]
pub fn parse_probe_output(out: &str) -> Option<OsInfo> {
    let text = out.trim();
    if text.is_empty() {
        return None;
    }
    if text.contains('=') {
        let mut info = OsInfo::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let v = v.trim().trim_matches('"').trim_matches('\'');
            match k.trim() {
                "ID" => info.id = v.to_lowercase(),
                "ID_LIKE" => info.like = v.split_whitespace().map(str::to_lowercase).collect(),
                "PRETTY_NAME" => info.pretty = v.to_string(),
                "NAME" if info.pretty.is_empty() => info.pretty = v.to_string(),
                _ => {}
            }
        }
        if !info.id.is_empty() {
            if info.pretty.is_empty() {
                info.pretty = info.id.clone();
            }
            return Some(info);
        }
    }
    // Pas d'os-release : premiere ligne de uname / ver.
    let first = text.lines().next()?.trim();
    let lower = first.to_lowercase();
    let id = if lower.contains("windows") {
        "windows"
    } else if lower == "darwin" {
        "darwin"
    } else if lower.contains("bsd") {
        return Some(OsInfo {
            id: lower.clone(),
            like: vec!["bsd".into()],
            pretty: first.to_string(),
        });
    } else if lower == "linux" {
        "linux"
    } else {
        return None;
    };
    Some(OsInfo {
        id: id.into(),
        like: Vec::new(),
        pretty: first.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_release_classique() {
        let out = "NAME=\"Debian GNU/Linux\"\nID=debian\nPRETTY_NAME=\"Debian GNU/Linux 12 (bookworm)\"\nVERSION_ID=\"12\"\n";
        let i = parse_probe_output(out).unwrap();
        assert_eq!(i.id, "debian");
        assert!(i.like.is_empty());
        assert_eq!(i.pretty, "Debian GNU/Linux 12 (bookworm)");
    }

    #[test]
    fn id_like_donne_la_famille_en_minuscules() {
        let out = "ID=CachyOS\nID_LIKE=\"Arch\"\nNAME=CachyOS\n";
        let i = parse_probe_output(out).unwrap();
        assert_eq!(i.id, "cachyos");
        assert_eq!(i.like, vec!["arch"]);
        assert_eq!(i.pretty, "CachyOS", "NAME sert de repli a PRETTY_NAME");
    }

    #[test]
    fn uname_pour_bsd_et_macos() {
        assert_eq!(parse_probe_output("Darwin\n").unwrap().id, "darwin");
        let bsd = parse_probe_output("FreeBSD").unwrap();
        assert_eq!(bsd.id, "freebsd");
        assert_eq!(bsd.like, vec!["bsd"]);
    }

    #[test]
    fn ver_pour_windows() {
        let i = parse_probe_output("\nMicrosoft Windows [version 10.0.22631.4037]\n").unwrap();
        assert_eq!(i.id, "windows");
    }

    #[test]
    fn sortie_vide_ou_inconnue_ne_donne_rien() {
        assert!(parse_probe_output("").is_none());
        assert!(parse_probe_output("cat: /etc/os-release: No such file").is_none());
    }
}
