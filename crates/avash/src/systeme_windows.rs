//! Les outils de Windows qu'Avash lance (`icacls`, `reg`, `whoami`).
//!
//! Audit du 27 novembre 2026 (WI-9), trois défauts propres à Windows :
//!
//! 1. `avash.exe` est une application sans console : lancer un outil en
//!    mode console sans `CREATE_NO_WINDOW` faisait clignoter une fenêtre noire
//!    à la génération d'une clé, à la création de `~/.ssh` et à l'analyse de
//!    l'import `PuTTY`.
//! 2. `Command::new("icacls")` cherche d'abord dans le dossier de
//!    l'application, puis dans `System32` : pour la version portable, c'est le
//!    dossier d'extraction, où un `reg.exe` déposé serait exécuté. Les outils
//!    sont lancés par leur chemin absolu dans le répertoire système.
//! 3. `icacls … /grant:r <nom>:(F)` recevait le nom de compte sans domaine :
//!    un compte local homonyme d'un compte du domaine, ou un compte Entra ID,
//!    se résolvait mal ou pas du tout, et la génération de clé échouait. Le
//!    droit est accordé par le SID du compte (`*S-1-5-21-…`), sans ambiguïté.

#[cfg(any(windows, test))]
use std::path::PathBuf;

/// Le chemin d'un outil du système : `<système>\<nom>.exe`. `systeme` est le
/// répertoire système rendu par Windows ; à défaut, `C:\Windows\System32`.
#[cfg(any(windows, test))]
fn chemin_outil(systeme: Option<PathBuf>, nom: &str) -> PathBuf {
    systeme
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows\System32"))
        .join(format!("{nom}.exe"))
}

/// Le répertoire système (`C:\Windows\System32`), tel que Windows le donne :
/// pas par `SystemRoot`, que l'environnement de l'utilisateur peut détourner.
#[cfg(windows)]
fn repertoire_systeme() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt as _;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetSystemDirectoryW(tampon: *mut u16, taille: u32) -> u32;
    }
    let mut tampon = vec![0_u16; 260];
    let taille = u32::try_from(tampon.len()).ok()?;
    // SAFETY: `tampon` est un tableau de `taille` unités UTF-16 initialisées,
    // vivant pendant l'appel ; l'API n'écrit jamais au-delà de `taille` et rend
    // le nombre d'unités écrites (sans le zéro final), ou une longueur plus
    // grande que `taille` si le tampon ne suffit pas, cas écarté ci-dessous.
    let n = unsafe { GetSystemDirectoryW(tampon.as_mut_ptr(), taille) };
    let n = usize::try_from(n).ok()?;
    if n == 0 || n >= tampon.len() {
        return None;
    }
    tampon.truncate(n);
    Some(PathBuf::from(std::ffi::OsString::from_wide(&tampon)))
}

/// Une commande qui lance l'outil système `nom` par son chemin absolu, sans
/// fenêtre de console.
#[cfg(windows)]
#[must_use]
pub fn commande(nom: &str) -> std::process::Command {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut c = std::process::Command::new(chemin_outil(repertoire_systeme(), nom));
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

/// Le SID d'une ligne `whoami /user /fo csv /nh` :
/// `"poste\ada","S-1-5-21-…-1001"`. `None` si la ligne n'en porte pas.
#[cfg(any(windows, test))]
fn sid_de_whoami(sortie: &str) -> Option<String> {
    let ligne = sortie.lines().find(|l| !l.trim().is_empty())?;
    let dernier = ligne.rsplit(',').next()?.trim().trim_matches('"');
    let valide = dernier.starts_with("S-1-")
        && dernier[4..]
            .split('-')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    valide.then(|| dernier.to_owned())
}

/// Le SID du compte qui fait tourner Avash, par `whoami /user`.
#[cfg(windows)]
#[must_use]
pub fn sid_courant() -> Option<String> {
    let sortie = commande("whoami")
        .args(["/user", "/fo", "csv", "/nh"])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !sortie.status.success() {
        return None;
    }
    sid_de_whoami(&String::from_utf8_lossy(&sortie.stdout))
}

#[cfg(test)]
mod tests {
    use super::{chemin_outil, sid_de_whoami};
    use std::path::PathBuf;

    /// Audit du 27 novembre 2026 (WI-9) : les outils partent du répertoire
    /// système, jamais du dossier de l'application.
    #[test]
    fn un_outil_systeme_est_lance_par_son_chemin_absolu() {
        let p = chemin_outil(Some(PathBuf::from("X:/Win/System32")), "icacls");
        assert_eq!(p, PathBuf::from("X:/Win/System32").join("icacls.exe"));
        let defaut = chemin_outil(None, "reg");
        assert!(defaut.to_string_lossy().ends_with("reg.exe"));
        assert!(defaut.to_string_lossy().starts_with(r"C:\Windows\System32"));
    }

    /// Audit du 27 novembre 2026 (WI-9) : le droit d'`icacls` se donne par le
    /// SID, lu dans la sortie de `whoami /user`.
    #[test]
    fn le_sid_se_lit_dans_la_sortie_de_whoami() {
        assert_eq!(
            sid_de_whoami("\"poste\\ada\",\"S-1-5-21-1004336348-1177238915-682003330-1001\"\r\n"),
            Some("S-1-5-21-1004336348-1177238915-682003330-1001".to_owned())
        );
        assert_eq!(
            sid_de_whoami("\"AzureAD\\Jérôme Dupont\",\"S-1-12-1-1-2-3-4\""),
            Some("S-1-12-1-1-2-3-4".to_owned())
        );
        assert_eq!(sid_de_whoami(""), None);
        assert_eq!(sid_de_whoami("\"x\",\"S-1-5-21-12a\""), None);
        assert_eq!(sid_de_whoami("\"x\",\"pas un sid\""), None);
    }
}
