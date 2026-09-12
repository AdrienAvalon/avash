//! Ligne de commande : options, mot de passe lu sur l'entrée standard, découpage domaine/utilisateur, disposition clavier, résolution.

// Ne sert qu'à la détection de la disposition clavier sous Unix (kxkbrc de KDE).
#[cfg(unix)]
use crate::empreintes::repertoire_configuration;
use anyhow::{Context, Result};

pub struct Args {
    pub host: String,
    pub port: u16,
    pub(crate) user: String,
    pub(crate) pass: String,
    pub(crate) domain: Option<String>,
    /// L'utilisateur a accepté de se passer de NLA pour ce serveur.
    pub(crate) sans_nla: bool,
    /// L'utilisateur a accepté les suites TLS héritées du système pour ce
    /// serveur (voir `tls_herite`).
    pub(crate) tls_herite: bool,
    pub(crate) layout: u32,
    /// Fichier du magnétoscope (`--enregistrer`, ou `AVASH_RDP_ENREGISTRER`
    /// dans l'environnement : l'interface ne passe pas cette option, la
    /// variable permet d'enregistrer une session depuis l'application normale).
    pub(crate) enregistrer: Option<String>,
    /// Plafond de l'enregistrement en octets (`AVASH_RDP_ENREGISTRER_PLAFOND`).
    pub(crate) plafond_enregistrement: u64,
    pub(crate) width: u16,
    pub(crate) height: u16,
    /// `--scale` : échelle DPI annoncée au serveur (MS-RDPBCGR `desktopScaleFactor`,
    /// 100..500), pour qu'il rende son interface plus grande quand on négocie une
    /// définition en pixels PHYSIQUES sur un écran HiDPI. 0 = non renseigné (le
    /// serveur rend à 100 %). Ajouté par l'audit du 7 septembre 2026 : sans lui,
    /// doubler la définition à 200 % donnait un texte net mais deux fois plus
    /// petit. Le serveur l'ignore de toute façon sous 512×384.
    pub(crate) desktop_scale_factor: u32,
    pub(crate) shot: Option<String>,
    /// `--vnc` : le serveur parle RFB, pas RDP. L'utilisateur devient
    /// facultatif (l'authentification VNC classique n'a qu'un mot de passe) et
    /// le port par défaut est 5900.
    pub vnc: bool,
    /// `--sans-son` : ne pas annoncer le canal audio (réglage de l'interface).
    pub(crate) sans_son: bool,
    /// `--lecteur <dossier>` : ce dossier du poste est servi au bureau distant
    /// comme lecteur « Avash » (redirection de lecteur, MS-RDPEFS).
    pub(crate) lecteur: Option<String>,
}

impl Args {
    /// Le processus vit-il pour son parent ? Tout lancement sauf la capture
    /// d'écran (`--shot`), qui se joue à la main ou par un script fermant
    /// l'entrée standard juste après le mot de passe. Voir `main.rs`.
    #[must_use]
    pub fn lie_au_parent(&self) -> bool {
        self.shot.is_none()
    }
}

/// Options qui prennent une valeur. Une clé consomme toujours l'argument qui
/// la suit, même s'il commence par un tiret : c'est une valeur, jamais un
/// drapeau (audit du 12 septembre 2026, C-injection-1).
const OPTIONS_A_VALEUR: &[&str] = &[
    "--host",
    "--port",
    "-u",
    "--username",
    "--domain",
    "--layout",
    "--width",
    "--height",
    "--scale",
    "--shot",
    "--enregistrer",
    "--lecteur",
];

/// Drapeaux, reconnus seulement en position de clé.
const DRAPEAUX: &[&str] = &["--vnc", "--sans-nla", "--tls-herite", "--sans-son"];

/// La ligne de commande lue position par position.
///
/// Trouvé par l'audit du 12 septembre 2026 (C-injection-1) : l'ancien
/// mini-parseur cherchait un drapeau à toutes les positions, valeurs comprises.
/// Un utilisateur nommé `--tls-herite` (un `rdp.yaml` importé suffit) faisait
/// accepter les suites TLS héritées sans le consentement que ce drapeau
/// représente, un hôte nommé `--sans-nla` faisait renoncer à NLA. Désormais
/// une clé consomme sa valeur, un drapeau n'est reconnu qu'en position de clé,
/// et tout argument inconnu est refusé : c'est ce qui rend visible un `-p`
/// resté dans un script.
struct Pa {
    valeurs: Vec<(String, String)>,
    drapeaux: Vec<String>,
}

impl Pa {
    fn analyser(args: &[String]) -> Result<Self> {
        let mut valeurs = Vec::new();
        let mut drapeaux = Vec::new();
        let mut suite = args.iter();
        while let Some(a) = suite.next() {
            if a == "-p" || a == "--password" {
                // Audit du 12 septembre 2026 (C-secrets-3) : en argument, le mot
                // de passe se lit dans /proc/<pid>/cmdline et le gestionnaire
                // des tâches. Le refus ne répète pas la valeur qui suit.
                anyhow::bail!(
                    "{a} n'est plus accepté : le mot de passe se lit sur l'entrée standard \
                     (première ligne), jamais en argument, où tout compte du poste le lirait \
                     dans la liste des processus."
                );
            }
            if OPTIONS_A_VALEUR.contains(&a.as_str()) {
                let v = suite
                    .next()
                    .with_context(|| format!("{a} attend une valeur"))?;
                valeurs.push((a.clone(), v.clone()));
            } else if DRAPEAUX.contains(&a.as_str()) {
                drapeaux.push(a.clone());
            } else if a.starts_with('-') {
                anyhow::bail!("argument inconnu : {a}");
            } else {
                // Une valeur orpheline n'est pas répétée : ce pourrait être un
                // secret tapé au mauvais endroit.
                anyhow::bail!("valeur inattendue, sans option qui la précède");
            }
        }
        Ok(Self { valeurs, drapeaux })
    }

    /// La première valeur donnée pour cette clé.
    fn opt(&self, k: &str) -> Option<String> {
        self.valeurs
            .iter()
            .find(|(cle, _)| cle == k)
            .map(|(_, v)| v.clone())
    }

    fn drapeau(&self, k: &str) -> bool {
        self.drapeaux.iter().any(|d| d == k)
    }

    fn req2(&self, k1: &str, k2: &str) -> Result<String> {
        self.opt(k1)
            .or_else(|| self.opt(k2))
            .with_context(|| format!("argument requis : {k1}/{k2}"))
    }
}

/// Mot de passe : la première ligne de l'entrée standard. Le parent le
/// transmet ainsi pour ne pas l'exposer dans /proc/<pid>/cmdline ; `--shot`
/// aussi (`printf '%s\n' "$MDP" | avash-rdp --shot …`). L'option `-p` a
/// disparu (audit du 12 septembre 2026, C-secrets-3).
fn lire_mot_de_passe(entree: &mut impl std::io::BufRead) -> Result<String> {
    let mut line = String::new();
    entree
        .read_line(&mut line)
        .context("lecture du mot de passe sur stdin")?;
    Ok(line.trim_end_matches(['\n', '\r']).to_string())
}

pub fn parse_args() -> Result<Args> {
    let a = Pa::analyser(&std::env::args().skip(1).collect::<Vec<_>>())?;
    let pass = lire_mot_de_passe(&mut std::io::stdin().lock())?;
    parse_args_de_pa(&a, pass, disposition_detectee)
}

/// Variante testable : les arguments et le mot de passe sont fournis, plutôt
/// que lus dans l'environnement et sur l'entrée standard.
///
/// Trouvé par l'audit du 7 septembre 2026 : sans `--layout`, chaque appel de
/// test partait sonder la disposition du poste (kxkbrc du répertoire de
/// configuration, spawn `localectl`), ce qu'aucun de ces tests n'affirme. On
/// injecte donc une disposition fixe (`us`) plutôt que la détection réelle.
#[cfg(test)]
pub(crate) fn parse_args_de(args: &[&str], pass: &str) -> Result<Args> {
    let pa = Pa::analyser(&args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())?;
    parse_args_de_pa(&pa, pass.to_owned(), || 0x0000_0409)
}

/// `detecter` fournit la disposition clavier de repli quand `--layout` manque.
/// En production c'est `disposition_detectee` (qui sonde le poste) ; les tests
/// passent une valeur fixe pour rester déterministes et hors hôte.
fn parse_args_de_pa(a: &Pa, pass: String, detecter: impl FnOnce() -> u32) -> Result<Args> {
    let vnc = a.drapeau("--vnc");
    let host = a.opt("--host").context("argument requis : --host")?;
    // Audit du 12 septembre 2026 (C-sidecar-12) : l'adresse est inscrite telle
    // quelle dans `rdp_known_hosts` (« hôte:port empreinte », une ligne par
    // hôte). Une espace y désarme le TOFU, un saut de ligne y écrit une ligne
    // arbitraire. Le cœur la valide déjà (`RdpHost::validate`), mais un
    // lancement manuel ou par `AVASH_RDP_BIN` passait à côté : le processus la
    // juge lui-même.
    anyhow::ensure!(
        !host.is_empty() && !host.chars().any(|c| c.is_whitespace() || c.is_control()),
        "adresse refusée ({host:?}) : vide, ou porteuse d'une espace ou d'un caractère de contrôle"
    );
    Ok(Args {
        host,
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
        tls_herite: a.drapeau("--tls-herite"),
        sans_son: a.drapeau("--sans-son"),
        lecteur: a.opt("--lecteur").filter(|l| !l.trim().is_empty()),
        layout: a
            .opt("--layout")
            .and_then(|v| analyser_disposition(&v))
            .unwrap_or_else(detecter),
        width: a
            .opt("--width")
            .and_then(|s| s.parse().ok())
            .unwrap_or(1280),
        height: a
            .opt("--height")
            .and_then(|s| s.parse().ok())
            .unwrap_or(800),
        desktop_scale_factor: a.opt("--scale").and_then(|s| s.parse().ok()).unwrap_or(0),
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

/// Traduit l'identifiant de source de saisie HIToolbox de macOS
/// (« com.apple.keylayout.French ») en identifiant RDP de disposition.
///
/// Trouvé par l'audit du 7 septembre 2026 : macOS n'a ni XKB, ni kxkbrc, ni
/// localectl, si bien que la branche `cfg(unix)` de `disposition_detectee` n'y
/// détectait jamais rien et rendait 0 — un Mac AZERTY sur xrdp gardait le
/// défaut « a » → « q ». HIToolbox expose la disposition courante sous
/// `AppleCurrentKeyboardLayoutInputSourceID` ; on ne garde que le dernier
/// segment du nom (« French », « French-PC »…) et on le mappe vers un code
/// XKB que `disposition_pour_code` sait traduire.
///
/// Compilée aussi sous `test` pour que le mapping soit vérifiable hors macOS.
#[cfg(any(target_os = "macos", test))]
fn disposition_macos_depuis_id(id: &str) -> Option<u32> {
    let nom = id.rsplit('.').next()?.trim();
    let code = match nom {
        "French" | "French-PC" | "French-numerical" => "fr",
        "Belgian" => "be",
        "Canadian" | "Canadian-CSA" => "ca",
        "SwissFrench" | "SwissGerman" => "ch",
        "German" => "de",
        "Austrian" => "at",
        "US" | "USExtended" | "ABC" => "us",
        "British" | "British-PC" => "gb",
        "Spanish" | "Spanish-ISO" => "es",
        "Italian" | "Italian-Pro" => "it",
        "Portuguese" => "pt",
        "Brazilian" => "br",
        "Dutch" => "nl",
        "Danish" => "dk",
        "Norwegian" => "no",
        "Swedish" | "Swedish-Pro" => "se",
        "Finnish" => "fi",
        "Polish" | "PolishPro" => "pl",
        "Czech" | "Czech-QWERTY" => "cz",
        "Russian" => "ru",
        "Turkish" | "Turkish-QWERTY" | "Turkish-Standard" => "tr",
        _ => return None,
    };
    disposition_pour_code(code)
}

/// Délai laissé à une sonde de la disposition clavier (localectl, defaults,
/// reg) : bien plus qu'il n'en faut à un poste en bonne santé (quelques
/// dizaines de millisecondes), bien moins qu'un onglet figé.
const DELAI_SONDE: std::time::Duration = std::time::Duration::from_secs(2);

/// Sortie standard d'une sonde du poste, ou `None` si elle ne se lance pas ou
/// ne rend pas la main dans `delai`.
///
/// Trouvé par l'audit du 12 septembre 2026 (C-SIL-13) : `localectl status` était
/// lancé et attendu sans borne, avant tout réseau. Un systemd-localed qui ne
/// répond pas (bus système saturé, conteneur sans systemd qui attend le délai
/// de D-Bus) laissait l'onglet en « connexion » sans un mot. La sortie est lue
/// par un fil ; passé le délai, la sonde est tuée et l'on se passe d'elle,
/// comme de toute sonde qui échoue. Qu'elle ait fini ou non, l'enfant est
/// récolté : pas de processus zombie derrière nous.
fn sortie_bornee(
    commande: &mut std::process::Command,
    delai: std::time::Duration,
) -> Option<Vec<u8>> {
    use std::io::Read as _;
    use std::process::Stdio;
    let mut enfant = commande
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let Some(mut sortie) = enfant.stdout.take() else {
        let _ = enfant.kill();
        let _ = enfant.wait();
        return None;
    };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut octets = Vec::new();
        let lu = sortie.read_to_end(&mut octets).map(|_| octets);
        let _ = tx.send(lu);
    });
    let recu = rx.recv_timeout(delai);
    let _ = enfant.kill();
    let _ = enfant.wait();
    recu.ok()?.ok()
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
    // macOS : la disposition native vient de HIToolbox, que les sources unix
    // ci-dessous ignorent. On l'interroge en premier pour qu'elle prime sur un
    // éventuel XKB_DEFAULT_LAYOUT égaré.
    #[cfg(target_os = "macos")]
    {
        if let Some(v) = sortie_bornee(
            std::process::Command::new("defaults").args([
                "read",
                "com.apple.HIToolbox",
                "AppleCurrentKeyboardLayoutInputSourceID",
            ]),
            DELAI_SONDE,
        )
        .and_then(|o| String::from_utf8(o).ok())
        .and_then(|t| disposition_macos_depuis_id(t.trim()))
        {
            return v;
        }
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
        if let Some(v) = sortie_bornee(
            std::process::Command::new("localectl").arg("status"),
            DELAI_SONDE,
        )
        .and_then(|o| String::from_utf8(o).ok())
        .and_then(|t| {
            t.lines()
                .find_map(|l| l.trim().strip_prefix("X11 Layout:"))
                .and_then(disposition_pour_code)
        }) {
            return v;
        }
    }
    #[cfg(windows)]
    {
        if let Some(v) = sortie_bornee(
            std::process::Command::new("reg").args([
                "query",
                r"HKCU\Keyboard Layout\Preload",
                "/v",
                "1",
            ]),
            DELAI_SONDE,
        )
        .and_then(|o| String::from_utf8(o).ok())
        .and_then(|t| {
            t.split_whitespace()
                .last()
                .and_then(|v| u32::from_str_radix(v, 16).ok())
        }) {
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
mod tests_detection_injectee {
    use super::{parse_args_de_pa, Pa};

    fn pa(args: &[&str]) -> Pa {
        Pa::analyser(&args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>()).unwrap()
    }

    /// Trouvé par l'audit du 7 septembre 2026 : `parse_args_de` sondait la
    /// disposition du poste (AVASH_RDP_LAYOUT, XKB_DEFAULT_LAYOUT, le fichier
    /// `kxkbrc` du répertoire de configuration, puis un spawn de `localectl`)
    /// à chaque appel sans `--layout`, alors qu'aucun de ces tests n'affirme
    /// rien sur `layout` : travail inutile, dépendant de l'hôte, coûteux. La
    /// détection est désormais un paramètre. Quand `--layout` est fourni, elle
    /// ne doit pas être invoquée du tout.
    #[test]
    fn la_detection_n_est_pas_invoquee_quand_layout_est_donne() {
        let a = pa(&["--host", "h", "-u", "x", "--layout", "de"]);
        let args =
            parse_args_de_pa(&a, "s".to_owned(), || panic!("détection appelée à tort")).unwrap();
        assert_eq!(args.layout, 0x0000_0407);
    }

    /// Sans `--layout`, la disposition vient de la détection injectée, jamais
    /// d'un sondage du poste : les tests restent déterministes et hors hôte.
    #[test]
    fn sans_layout_la_disposition_vient_de_la_detection_injectee() {
        let a = pa(&["--host", "h", "-u", "x"]);
        let args = parse_args_de_pa(&a, "s".to_owned(), || 0xABCD).unwrap();
        assert_eq!(args.layout, 0xABCD);
    }
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
mod tests_echelle {
    use super::parse_args_de;

    /// HiDPI (audit du 7 septembre 2026) : quand l'interface négocie une
    /// définition en pixels physiques sur un écran à 200 %, elle passe `--scale
    /// 200` pour que le serveur rende son interface deux fois plus grande. Sans
    /// cette annonce, le texte distant serait net mais deux fois plus petit.
    #[test]
    fn l_echelle_est_lue_depuis_scale() {
        let a = parse_args_de(&["--host", "h", "-u", "x", "--scale", "200"], "s").unwrap();
        assert_eq!(a.desktop_scale_factor, 200);
    }

    /// Sans `--scale`, on n'annonce rien (0) : le serveur rend à 100 %, comme
    /// avant le correctif. C'est le comportement de repli pour un écran standard.
    #[test]
    fn sans_scale_l_echelle_vaut_zero() {
        let a = parse_args_de(&["--host", "h", "-u", "x"], "s").unwrap();
        assert_eq!(a.desktop_scale_factor, 0);
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
mod tests_disposition_macos {
    use super::disposition_macos_depuis_id;

    /// Trouvé par l'audit du 7 septembre 2026 : sur macOS, `disposition_detectee`
    /// ne connaissait que XKB_DEFAULT_LAYOUT, kxkbrc et localectl — trois sources
    /// absentes du système (la branche `cfg(unix)` couvre pourtant le Mac) — et
    /// rendait toujours 0, si bien qu'un Mac AZERTY sur xrdp gardait le défaut
    /// « a » → « q » corrigé ailleurs. HIToolbox nomme la disposition
    /// « com.apple.keylayout.French » ; on la traduit désormais.
    #[test]
    fn l_identifiant_hitoolbox_donne_le_bon_layout() {
        assert_eq!(
            disposition_macos_depuis_id("com.apple.keylayout.French"),
            Some(0x0000_040C)
        );
        assert_eq!(
            disposition_macos_depuis_id("com.apple.keylayout.French-PC"),
            Some(0x0000_040C)
        );
        assert_eq!(
            disposition_macos_depuis_id("com.apple.keylayout.German"),
            Some(0x0000_0407)
        );
        assert_eq!(
            disposition_macos_depuis_id("com.apple.keylayout.British"),
            Some(0x0000_0809)
        );
        assert_eq!(
            disposition_macos_depuis_id("com.apple.keylayout.US"),
            Some(0x0000_0409)
        );
        assert_eq!(
            disposition_macos_depuis_id("com.apple.keylayout.Belgian"),
            Some(0x0000_080C)
        );
    }

    #[test]
    fn un_identifiant_inconnu_ou_vide_ne_donne_rien() {
        // Une méthode de saisie (japonais via Kotoeri) n'est pas une disposition
        // de touches ; mieux vaut le défaut du serveur qu'une valeur inventée.
        assert_eq!(
            disposition_macos_depuis_id("com.apple.inputmethod.Kotoeri.Japanese"),
            None
        );
        assert_eq!(
            disposition_macos_depuis_id("com.apple.keylayout.Klingon"),
            None
        );
        assert_eq!(disposition_macos_depuis_id(""), None);
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

#[cfg(test)]
mod tests_ligne_de_commande {
    use super::{parse_args_de, sortie_bornee};

    /// Trouvé par l'audit du 12 septembre 2026 (C-injection-1) : le mini-parseur
    /// cherchait un drapeau à TOUTES les positions, valeurs comprises. Un
    /// `rdp.yaml` importé dont l'utilisateur vaut `--tls-herite` faisait
    /// renoncer aux suites modernes sans consentement, un hôte nommé
    /// `--sans-nla` renonçait à NLA. Une clé consomme sa valeur, et un drapeau
    /// n'est reconnu qu'en position de clé.
    #[test]
    fn une_valeur_d_option_n_est_jamais_un_drapeau() {
        let a = parse_args_de(&["--host", "--sans-nla", "-u", "--tls-herite"], "p").unwrap();
        assert_eq!(a.host, "--sans-nla");
        assert_eq!(a.user, "--tls-herite");
        assert!(
            !a.sans_nla && !a.tls_herite && !a.vnc && !a.sans_son,
            "une valeur a été prise pour un drapeau"
        );
        // Contrôle positif : en position de clé, le drapeau reste un drapeau.
        let a = parse_args_de(
            &["--host", "h", "--sans-nla", "-u", "x", "--tls-herite"],
            "p",
        )
        .unwrap();
        assert!(a.sans_nla && a.tls_herite);
    }

    /// Trouvé par l'audit du 12 septembre 2026 (C-secrets-3) : `-p` et
    /// `--password` restaient acceptés (« utile pour `--shot` »), et tout usage
    /// manuel exposait le mot de passe dans `/proc/<pid>/cmdline` et le
    /// gestionnaire des tâches. Il ne se lit plus que sur l'entrée standard, et
    /// l'option est refusée avec un message qui le dit, sans répéter la valeur.
    #[test]
    fn le_mot_de_passe_n_est_jamais_lu_des_arguments() {
        for cle in ["-p", "--password"] {
            let Err(e) = parse_args_de(&["--host", "h", "-u", "x", cle, "secret"], "") else {
                panic!("{cle} a été accepté en argument");
            };
            let m = format!("{e:#}");
            assert!(m.contains("entrée standard"), "{m}");
            assert!(
                !m.contains("secret"),
                "le refus répète le mot de passe : {m}"
            );
        }
    }

    /// Une faute de frappe (`--hsot`) ou une option disparue ne passe plus en
    /// silence : c'est ce qui fait qu'un `-p` resté dans un script se voit.
    #[test]
    fn un_argument_inconnu_ou_sans_valeur_est_refuse() {
        assert!(parse_args_de(&["--hsot", "h", "-u", "x"], "p").is_err());
        assert!(parse_args_de(&["--host", "h", "-u"], "p").is_err());
        assert!(parse_args_de(&["--host", "h", "-u", "x", "reste"], "p").is_err());
    }

    /// Trouvé par l'audit du 12 septembre 2026 (C-sidecar-12) : le processus
    /// s'en remettait au cœur (`RdpHost::validate`) pour l'adresse qu'il inscrit
    /// dans `rdp_known_hosts`. Un lancement manuel (`AVASH_RDP_BIN`) contournait
    /// la garde : une espace désarme le TOFU, un saut de ligne écrit une ligne
    /// arbitraire dans le fichier de confiance.
    #[test]
    fn une_adresse_a_blanc_est_refusee_par_le_sidecar_lui_meme() {
        for h in ["a b", "a\tb", "a\nb", "a\rb", "a\0b", ""] {
            assert!(
                parse_args_de(&["--host", h, "-u", "u"], "p").is_err(),
                "{h:?} a été accepté"
            );
        }
        assert!(parse_args_de(&["--host", "srv.exemple", "-u", "u"], "p").is_ok());
        assert!(parse_args_de(&["--host", "fe80::1", "-u", "u"], "p").is_ok());
    }

    /// Trouvé par l'audit du 12 septembre 2026 (C-SIL-13) : `localectl status`
    /// était lancé et attendu sans borne pour deviner la disposition, avant
    /// tout réseau ; un systemd-localed qui ne répond pas laissait l'onglet en
    /// « connexion » sans un mot. La sonde est abandonnée passé son délai.
    #[cfg(unix)]
    #[test]
    fn une_sonde_qui_ne_rend_pas_la_main_est_abandonnee() {
        let debut = std::time::Instant::now();
        let sortie = sortie_bornee(
            std::process::Command::new("sleep").arg("6"),
            std::time::Duration::from_millis(200),
        );
        assert!(sortie.is_none(), "une sonde trop lente ne rend rien");
        assert!(
            debut.elapsed() < std::time::Duration::from_secs(3),
            "la sonde a été attendue {:?}",
            debut.elapsed()
        );
    }

    #[cfg(unix)]
    #[test]
    fn une_sonde_rapide_rend_sa_sortie() {
        let s = sortie_bornee(
            std::process::Command::new("sh").args(["-c", "echo 'X11 Layout: fr'"]),
            std::time::Duration::from_secs(5),
        )
        .expect("une sortie");
        assert_eq!(String::from_utf8(s).unwrap().trim(), "X11 Layout: fr");
    }
}
