//! Ligne de commande : options, mot de passe lu sur l'entrée standard, découpage domaine/utilisateur, disposition clavier, résolution.

use crate::empreintes::repertoire_configuration;
use anyhow::{Context, Result};
use std::io::BufRead as _;

pub(crate) struct Args {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) user: String,
    pub(crate) pass: String,
    pub(crate) domain: Option<String>,
    /// L'utilisateur a accepté de se passer de NLA pour ce serveur.
    pub(crate) sans_nla: bool,
    pub(crate) layout: u32,
    /// Fichier du magnétoscope (`--enregistrer`, ou `AVASH_RDP_ENREGISTRER`
    /// dans l'environnement : l'interface ne passe pas cette option, la
    /// variable permet d'enregistrer une session depuis l'application normale).
    pub(crate) enregistrer: Option<String>,
    /// Plafond de l'enregistrement en octets (`AVASH_RDP_ENREGISTRER_PLAFOND`).
    pub(crate) plafond_enregistrement: u64,
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) shot: Option<String>,
    /// `--vnc` : le serveur parle RFB, pas RDP. L'utilisateur devient
    /// facultatif (l'authentification VNC classique n'a qu'un mot de passe) et
    /// le port par défaut est 5900.
    pub(crate) vnc: bool,
    /// `--sans-son` : ne pas annoncer le canal audio (réglage de l'interface).
    pub(crate) sans_son: bool,
}

struct Pa(Vec<String>);

impl Pa {
    fn opt(&self, k: &str) -> Option<String> {
        self.0
            .iter()
            .position(|a| a == k)
            .and_then(|i| self.0.get(i + 1).cloned())
    }
    fn drapeau(&self, k: &str) -> bool {
        self.0.iter().any(|a| a == k)
    }
    fn req2(&self, k1: &str, k2: &str) -> Result<String> {
        self.opt(k1)
            .or_else(|| self.opt(k2))
            .with_context(|| format!("argument requis : {k1}/{k2}"))
    }
}

/// Mot de passe : depuis `-p/--password` s'il est fourni (utile pour `--shot`),
/// sinon lu sur la première ligne de stdin (le parent le transmet ainsi pour
/// ne pas l'exposer dans /proc/<pid>/cmdline).
fn read_password(a: &Pa) -> Result<String> {
    if let Some(p) = a.opt("-p").or_else(|| a.opt("--password")) {
        return Ok(p);
    }
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .context("lecture du mot de passe sur stdin")?;
    Ok(line.trim_end_matches(['\n', '\r']).to_string())
}

pub(crate) fn parse_args() -> Result<Args> {
    let a = Pa(std::env::args().skip(1).collect());
    let pass = read_password(&a)?;
    parse_args_de_pa(&a, pass)
}

/// Variante testable : les arguments et le mot de passe sont fournis, plutôt
/// que lus dans l'environnement et sur l'entrée standard.
#[cfg(test)]
pub(crate) fn parse_args_de(args: &[&str], pass: &str) -> Result<Args> {
    let pa = Pa(args.iter().map(|s| (*s).to_owned()).collect());
    parse_args_de_pa(&pa, pass.to_owned())
}

fn parse_args_de_pa(a: &Pa, pass: String) -> Result<Args> {
    let vnc = a.drapeau("--vnc");
    Ok(Args {
        host: a.opt("--host").context("argument requis : --host")?,
        port: a
            .opt("--port")
            .and_then(|s| s.parse().ok())
            .unwrap_or(if vnc { 5900 } else { 3389 }),
        user: if vnc {
            a.opt("-u")
                .or_else(|| a.opt("--username"))
                .unwrap_or_default()
        } else {
            a.req2("-u", "--username")?
        },
        vnc,
        pass,
        domain: a.opt("--domain"),
        sans_nla: a.drapeau("--sans-nla"),
        sans_son: a.drapeau("--sans-son"),
        layout: a
            .opt("--layout")
            .and_then(|v| analyser_disposition(&v))
            .unwrap_or_else(disposition_detectee),
        width: a
            .opt("--width")
            .and_then(|s| s.parse().ok())
            .unwrap_or(1280),
        height: a
            .opt("--height")
            .and_then(|s| s.parse().ok())
            .unwrap_or(800),
        shot: a.opt("--shot"),
        enregistrer: a
            .opt("--enregistrer")
            .or_else(|| std::env::var("AVASH_RDP_ENREGISTRER").ok())
            .filter(|c| !c.is_empty()),
        plafond_enregistrement: plafond_depuis(
            std::env::var("AVASH_RDP_ENREGISTRER_PLAFOND")
                .ok()
                .as_deref(),
        ),
    })
}

/// Plafond d'un enregistrement, en octets. Le défaut (4 Mio) convient à une
/// fixture de dépôt ; pour capturer un défaut vu à l'usage — des carrés noirs
/// dans une fenêtre qui bouge beaucoup, signalés le 2026-09-03 — il faut
/// plusieurs minutes de flux, donc bien davantage : la variable le fixe.
/// Une valeur illisible ou nulle rend le défaut, pas une erreur : on
/// enregistre quand même.
fn plafond_depuis(valeur: Option<&str>) -> u64 {
    valeur
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&p| p > 0)
        .unwrap_or(crate::magnetoscope::PLAFOND_DEFAUT)
}

/// Sépare un domaine éventuellement collé au nom d'utilisateur.
/// NLA/CredSSP attend le domaine à part : « DOMAINE\\user » ou « user@domaine »
/// sont acceptés par les utilisateurs, on les découpe ici. `--domain` explicite
/// est prioritaire (le nom est alors laissé intact).
pub(crate) fn split_credentials(
    user: &str,
    explicit_domain: Option<&str>,
) -> (String, Option<String>) {
    if let Some(d) = explicit_domain {
        return (user.to_string(), Some(d.to_string()));
    }
    if let Some((dom, name)) = user.split_once('\\') {
        return (name.to_string(), Some(dom.to_string()));
    }
    if let Some((name, dom)) = user.split_once('@') {
        return (name.to_string(), Some(dom.to_string()));
    }
    (user.to_string(), None)
}

/// Identifiant RDP de disposition clavier pour un code XKB (« fr », « de »…).
///
/// RDP transporte des **scancodes**, pas des caractères : c'est le serveur qui
/// les traduit, d'après la disposition que le client annonce. En annonçant 0,
/// avash laissait le serveur choisir — en pratique l'américain. Sur un clavier
/// AZERTY, taper « a » produisait « q ». Signalé par Adrien sur SLED-15.
///
/// Windows ne s'en plaignait pas : il rend `0` par son propre défaut, souvent
/// aligné sur la session. xrdp, lui, retombe sur l'américain.
fn disposition_pour_code(code: &str) -> Option<u32> {
    // Identifiants Microsoft (« Keyboard Identifiers »).
    Some(match code.split([',', '(']).next()?.trim() {
        "fr" => 0x0000_040C,
        "be" => 0x0000_080C,
        "ca" => 0x0000_0C0C,
        "ch" => 0x0000_100C,
        "de" => 0x0000_0407,
        "at" => 0x0000_0C07,
        "us" => 0x0000_0409,
        "gb" | "uk" => 0x0000_0809,
        "es" => 0x0000_040A,
        "it" => 0x0000_0410,
        "pt" => 0x0000_0816,
        "br" => 0x0000_0416,
        "nl" => 0x0000_0413,
        "dk" => 0x0000_0406,
        "no" => 0x0000_0414,
        "se" => 0x0000_041D,
        "fi" => 0x0000_040B,
        "pl" => 0x0000_0415,
        "cz" => 0x0000_0405,
        "ru" => 0x0000_0419,
        "tr" => 0x0000_041F,
        "jp" => 0x0000_0411,
        _ => return None,
    })
}

/// Accepte un identifiant numérique (« 0x40c », « 1036 ») ou un code (« fr »).
fn analyser_disposition(v: &str) -> Option<u32> {
    let v = v.trim();
    if let Some(hex) = v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
        return u32::from_str_radix(hex, 16).ok();
    }
    if let Ok(n) = v.parse::<u32>() {
        return Some(n);
    }
    disposition_pour_code(v)
}

/// Disposition du poste, ou 0 si on ne sait pas — mieux vaut le défaut du
/// serveur qu'une disposition inventée.
fn disposition_detectee() -> u32 {
    if let Some(v) = std::env::var_os("AVASH_RDP_LAYOUT")
        .and_then(|v| v.into_string().ok())
        .and_then(|v| analyser_disposition(&v))
    {
        return v;
    }
    #[cfg(unix)]
    {
        if let Some(v) = std::env::var_os("XKB_DEFAULT_LAYOUT")
            .and_then(|v| v.into_string().ok())
            .and_then(|v| disposition_pour_code(&v))
        {
            return v;
        }
        // KDE garde la disposition de session ici, que localectl ignore.
        if let Some(v) = repertoire_configuration()
            .map(|c| c.join("kxkbrc"))
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| {
                t.lines()
                    .find_map(|l| l.strip_prefix("LayoutList="))
                    .and_then(disposition_pour_code)
            })
        {
            return v;
        }
        if let Some(v) = std::process::Command::new("localectl")
            .arg("status")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|t| {
                t.lines()
                    .find_map(|l| l.trim().strip_prefix("X11 Layout:"))
                    .and_then(disposition_pour_code)
            })
        {
            return v;
        }
    }
    #[cfg(windows)]
    {
        if let Some(v) = std::process::Command::new("reg")
            .args(["query", r"HKCU\Keyboard Layout\Preload", "/v", "1"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|t| {
                t.split_whitespace()
                    .last()
                    .and_then(|v| u32::from_str_radix(v, 16).ok())
            })
        {
            return v;
        }
    }
    0
}

/// Plafond de résolution accepté d'un serveur RDP.
///
/// C'est le serveur qui **confirme** la résolution, et il n'est pas tenu de
/// reprendre celle demandée. Rien ne bornait ce qu'on en faisait :
/// `DecodedImage::new` alloue `largeur × hauteur × 4` octets d'un bloc, soit
/// 17 Gio pour un 65535×65535 annoncé — mort du processus par manque de
/// mémoire, rejouable à volonté par la renégociation `DeactivateAll`. 8192 est
/// déjà la borne appliquée au redimensionnement côté interface.
pub(crate) const TAILLE_MAX: u16 = 8192;

pub(crate) fn taille_sure(w: u16, h: u16) -> anyhow::Result<(u16, u16)> {
    anyhow::ensure!(
        w > 0 && h > 0 && w <= TAILLE_MAX && h <= TAILLE_MAX,
        "Le serveur annonce une résolution inacceptable ({w}x{h})."
    );
    Ok((w, h))
}

#[cfg(test)]
mod tests_vnc {
    use super::parse_args_de;

    /// L'authentification VNC classique n'a qu'un mot de passe : l'utilisateur
    /// n'est plus requis, et le port par défaut change.
    #[test]
    fn en_vnc_l_utilisateur_est_facultatif_et_le_port_vaut_5900() {
        let a = parse_args_de(&["--vnc", "--host", "h"], "s").unwrap();
        assert!(a.vnc);
        assert_eq!(a.port, 5900);
        assert_eq!(a.user, "");
        assert_eq!(a.pass, "s");
        let a = parse_args_de(&["--vnc", "--host", "h", "--port", "5901", "-u", "x"], "s").unwrap();
        assert_eq!((a.port, a.user.as_str()), (5901, "x"));
    }

    #[test]
    fn en_rdp_l_utilisateur_reste_requis_et_le_port_vaut_3389() {
        assert!(parse_args_de(&["--host", "h"], "s").is_err());
        let a = parse_args_de(&["--host", "h", "-u", "x"], "s").unwrap();
        assert!(!a.vnc);
        assert_eq!(a.port, 3389);
    }
}

#[cfg(test)]
mod tests_taille {
    use super::taille_sure;

    /// C'est le serveur qui confirme la résolution, et rien ne l'oblige à
    /// reprendre celle demandée. `DecodedImage::new` alloue largeur × hauteur × 4
    /// d'un bloc : 17 Gio pour un 65535×65535 annoncé, rejouable à volonté par
    /// renégociation. Ce plafond n'avait aucun test — et les tests du sidecar ne
    /// tournaient nulle part, ce qui n'aurait rien changé.
    #[test]
    fn une_resolution_deraisonnable_est_refusee() {
        for (w, h) in [(0, 1), (1, 0), (0, 0), (8193, 1), (1, 8193), (65535, 65535)] {
            assert!(
                taille_sure(w, h).is_err(),
                "résolution acceptée alors qu'elle ne devrait pas : {w}x{h}"
            );
        }
    }

    #[test]
    fn les_resolutions_courantes_passent() {
        for (w, h) in [(1, 1), (1920, 1080), (3440, 1440), (8192, 8192)] {
            assert_eq!(taille_sure(w, h).unwrap(), (w, h), "{w}x{h} refusée à tort");
        }
    }
}

#[cfg(test)]
mod tests_disposition {
    use super::{analyser_disposition, disposition_pour_code};

    #[test]
    fn les_dispositions_courantes_sont_reconnues() {
        assert_eq!(disposition_pour_code("fr"), Some(0x0000_040C));
        assert_eq!(disposition_pour_code("de"), Some(0x0000_0407));
        assert_eq!(disposition_pour_code("us"), Some(0x0000_0409));
        assert_eq!(disposition_pour_code("be"), Some(0x0000_080C));
    }

    #[test]
    fn une_liste_xkb_ne_retient_que_la_premiere() {
        // KDE écrit « LayoutList=fr,us » quand deux dispositions coexistent.
        assert_eq!(disposition_pour_code("fr,us"), Some(0x0000_040C));
        // Et setxkbmap rend parfois « fr(azerty) ».
        assert_eq!(disposition_pour_code("fr(azerty)"), Some(0x0000_040C));
    }

    #[test]
    fn une_disposition_inconnue_ne_donne_rien() {
        // Mieux vaut le défaut du serveur qu'une disposition inventée.
        assert_eq!(disposition_pour_code("klingon"), None);
        assert_eq!(disposition_pour_code(""), None);
    }

    #[test]
    fn l_argument_accepte_hexa_decimal_et_code() {
        assert_eq!(analyser_disposition("0x40c"), Some(0x40C));
        assert_eq!(analyser_disposition("1036"), Some(1036));
        assert_eq!(analyser_disposition(" fr "), Some(0x0000_040C));
        assert_eq!(analyser_disposition("n'importe quoi"), None);
    }
}

#[cfg(test)]
mod tests_identifiants {
    use super::split_credentials;

    /// NLA/CredSSP attend le domaine à part ; l'utilisateur, lui, le tape
    /// comme il l'a toujours fait. Les deux formes courantes sont découpées, et
    /// `--domain` explicite laisse le nom intact.
    #[test]
    fn les_deux_formes_de_domaine_sont_decoupees() {
        assert_eq!(
            split_credentials("TEST\\adrien", None),
            ("adrien".to_owned(), Some("TEST".to_owned()))
        );
        assert_eq!(
            split_credentials("adrien@exemple.local", None),
            ("adrien".to_owned(), Some("exemple.local".to_owned()))
        );
    }

    #[test]
    fn sans_domaine_le_nom_reste_entier() {
        assert_eq!(
            split_credentials("adrien", None),
            ("adrien".to_owned(), None)
        );
    }

    #[test]
    fn un_domaine_explicite_prime_et_laisse_le_nom_tel_quel() {
        // Un compte contenant un « @ » légitime ne doit pas être redécoupé
        // quand l'appelant a déjà dit le domaine.
        assert_eq!(
            split_credentials("adrien@exemple.local", Some("AUTRE")),
            ("adrien@exemple.local".to_owned(), Some("AUTRE".to_owned()))
        );
    }
}

#[cfg(test)]
mod tests_enregistrement {
    use super::plafond_depuis;
    use crate::magnetoscope::PLAFOND_DEFAUT;

    /// Le plafond vient de l'environnement ; tout ce qui n'est pas un entier
    /// strictement positif rend le défaut, sans jamais empêcher d'enregistrer.
    #[test]
    fn le_plafond_lit_un_entier_et_retombe_sur_le_defaut_sinon() {
        assert_eq!(plafond_depuis(Some("268435456")), 268_435_456);
        assert_eq!(plafond_depuis(Some(" 1024 ")), 1024);
        assert_eq!(plafond_depuis(None), PLAFOND_DEFAUT);
        assert_eq!(plafond_depuis(Some("")), PLAFOND_DEFAUT);
        assert_eq!(plafond_depuis(Some("0")), PLAFOND_DEFAUT);
        assert_eq!(plafond_depuis(Some("beaucoup")), PLAFOND_DEFAUT);
        assert_eq!(plafond_depuis(Some("-5")), PLAFOND_DEFAUT);
    }
}
