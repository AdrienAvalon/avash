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
    // Pas d'os-release : repli sur `uname -s` (Unix) ou `ver` (Windows).
    // Trouvé par l'audit du 7 septembre 2026 : sur un OpenSSH Windows dont le
    // shell est cmd.exe, la sonde imprime d'abord des erreurs sur stderr (cmd
    // échoue à créer `\dev\null`, ou ne connaît ni `cat` ni `uname`), et stderr
    // est mêlé à stdout par `executer_borne` ; la ligne « Microsoft Windows … »
    // n'est donc plus la première. On balaie toutes les lignes et l'on retient
    // la première classable, en ignorant le bruit — même robustesse que la
    // branche os-release ci-dessus.
    for ligne in text.lines() {
        let ligne = ligne.trim();
        let lower = ligne.to_lowercase();
        // `linux`/`darwin` en égalité stricte (sortie exacte de `uname -s`) :
        // un `contains` attraperait « GNU/Linux » ou un chemin dans un message
        // d'erreur. `windows`/`bsd` par sous-chaîne, seules formes stables de
        // `ver` et des `uname` BSD (FreeBSD, OpenBSD, NetBSD, DragonFly).
        let id = if lower.contains("windows") {
            "windows"
        } else if lower == "darwin" {
            "darwin"
        } else if lower.contains("bsd") {
            return Some(OsInfo {
                id: lower,
                like: vec!["bsd".into()],
                pretty: ligne.to_string(),
            });
        } else if lower == "linux" {
            "linux"
        } else {
            continue;
        };
        return Some(OsInfo {
            id: id.into(),
            like: Vec::new(),
            pretty: ligne.to_string(),
        });
    }
    None
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

    // Trouvé par l'audit du 7 septembre 2026 : sur un OpenSSH Windows dont le
    // shell est cmd.exe, `2>/dev/null` de la sonde fait échouer cmd (il tente
    // de créer `\dev\null`), qui imprime « Le chemin d'accès spécifié est
    // introuvable. » sur stderr (une fois par commande) avant que `ver` ne
    // s'exécute. stderr étant mêlé à stdout, la sortie commence par ce bruit ;
    // ne classer que la première ligne rendait `None`, donc aucun logo Windows.
    #[test]
    fn ver_precede_d_erreurs_cmd_donne_windows() {
        let out = "Le chemin d'accès spécifié est introuvable.\r\nLe chemin d'accès spécifié est introuvable.\r\n\r\nMicrosoft Windows [version 10.0.22631.4037]\r\n";
        assert_eq!(parse_probe_output(out).unwrap().id, "windows");
    }

    // Variante anglaise du même cas (locale par défaut d'un poste Windows) ;
    // « Version » avec majuscule, la forme réelle de `ver`.
    #[test]
    fn ver_precede_d_erreurs_cmd_anglais_donne_windows() {
        let out = "The system cannot find the path specified.\r\nThe system cannot find the path specified.\r\n\r\nMicrosoft Windows [Version 10.0.22631.4037]\r\n";
        assert_eq!(parse_probe_output(out).unwrap().id, "windows");
    }

    // Même défaut côté sh : un shell distant peut baver sur stderr avant
    // `uname` (locale absente, message de connexion) ; la ligne utile n'est
    // alors plus la première.
    #[test]
    fn uname_precede_d_un_bruit_stderr_donne_linux() {
        let out = "bash: warning: setlocale: LC_ALL: cannot change locale\nLinux\n";
        assert_eq!(parse_probe_output(out).unwrap().id, "linux");
    }

    #[test]
    fn sortie_vide_ou_inconnue_ne_donne_rien() {
        assert!(parse_probe_output("").is_none());
        assert!(parse_probe_output("cat: /etc/os-release: No such file").is_none());
        // Régression : du bruit sur plusieurs lignes, sans aucune ligne
        // classable, reste `None` (sinon on afficherait un logo au hasard).
        assert!(
            parse_probe_output(
                "Le chemin d'accès spécifié est introuvable.\r\nThe system cannot find the path specified.\r\n"
            )
            .is_none()
        );
    }
}
