//! Connexions RDP enregistrees, dans `~/.config/avash/rdp.yaml`.
//!
//! `~/.ssh/config` est propre au SSH (OpenSSH ne connait pas le RDP) : les
//! bureaux distants ont donc leur propre fichier. Le mot de passe, lui, va
//! dans le trousseau systeme (comme pour SSH), sous un identifiant prefixe
//! `rdp:` pour ne pas entrer en collision avec un compte SSH du meme hote.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Le protocole d'un bureau distant. Les deux partagent le fichier, le
/// formulaire, le trousseau et le processus qui les sert ; seul le dialogue
/// avec le serveur change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Protocole {
    #[default]
    Rdp,
    Vnc,
}

impl Protocole {
    /// Port par défaut du protocole.
    #[must_use]
    pub fn port_par_defaut(self) -> u16 {
        match self {
            Protocole::Rdp => 3389,
            Protocole::Vnc => 5900,
        }
    }

    /// Lit « rdp » ou « vnc » ; tout autre texte (ou rien) vaut RDP, le
    /// protocole des fichiers écrits avant l'arrivée du VNC.
    #[must_use]
    pub fn depuis(texte: Option<&str>) -> Self {
        match texte.map(str::trim) {
            Some("vnc") => Protocole::Vnc,
            _ => Protocole::Rdp,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RdpHost {
    pub id: String,
    /// Libelle affiche ; a defaut, `user@host`.
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub width: u16,
    pub height: u16,
    /// Dossier de rangement Avash (ex. « prod/web »), vide = racine.
    #[serde(default)]
    pub folder: String,
    /// L'utilisateur a accepté que ce serveur se passe d'authentification
    /// réseau (NLA). Faux par défaut, y compris pour un fichier écrit par une
    /// version antérieure : on ne relâche jamais une garde en silence.
    #[serde(default)]
    pub sans_nla: bool,
    /// RDP sauf mention contraire : un fichier antérieur n'a pas ce champ.
    #[serde(default)]
    pub protocole: Protocole,
    /// Dossier du poste servi au bureau distant comme lecteur « Avash »
    /// (redirection de lecteur). Absent : rien n'est partagé.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partage: Option<String>,
}

impl RdpHost {
    #[must_use]
    pub fn new(name: &str, host: &str, port: u16, user: &str, width: u16, height: u16) -> Self {
        let host = host.trim().to_string();
        let user = user.trim().to_string();
        let name = {
            let n = name.trim();
            if !n.is_empty() {
                n.to_string()
            } else if user.is_empty() {
                host.clone()
            } else {
                format!("{user}@{host}")
            }
        };
        Self {
            id: format!("r-{:016x}", rand::random::<u64>()),
            name,
            host,
            port,
            user,
            width,
            height,
            folder: String::new(),
            sans_nla: false,
            protocole: Protocole::Rdp,
            partage: None,
        }
    }

    /// Le même bureau, en VNC.
    #[must_use]
    pub fn en(mut self, protocole: Protocole) -> Self {
        self.protocole = protocole;
        self
    }

    /// Identifiant du mot de passe de ce bureau dans le trousseau.
    #[must_use]
    pub fn compte_trousseau(&self) -> String {
        keyring_account_pour(self.protocole, &self.user, &self.host, self.port)
    }

    pub fn validate(&self) -> Result<()> {
        if self.host.trim().is_empty() {
            bail!("L'adresse du serveur RDP est vide.");
        }
        // Le fichier d'empreintes `rdp_known_hosts` est en « clé empreinte »,
        // découpé au premier espace. Une adresse contenant une espace produit
        // une ligne qu'on ne retrouve jamais : chaque connexion redevient un
        // premier contact, l'empreinte est réécrite en fin de fichier, et un
        // changement de certificat n'est plus jamais détecté — le TOFU est
        // neutralisé sans que rien ne le signale. Un saut de ligne, lui,
        // permettrait d'y écrire une ligne arbitraire.
        if self.host.contains([' ', '\t', '\n', '\r', '\0']) {
            bail!("L'adresse du serveur RDP contient un caractère interdit (espace ou saut de ligne).");
        }
        // L'authentification VNC classique n'a qu'un mot de passe : le nom
        // d'utilisateur y est facultatif.
        if self.protocole == Protocole::Rdp && self.user.trim().is_empty() {
            bail!("L'utilisateur RDP est vide.");
        }
        Ok(())
    }
}

/// Identifiant trousseau d'un compte RDP (distinct des comptes SSH).
#[must_use]
pub fn keyring_account(user: &str, host: &str, port: u16) -> String {
    keyring_account_pour(Protocole::Rdp, user, host, port)
}

/// Identifiant trousseau d'un compte de bureau distant, préfixé par son
/// protocole : un serveur VNC et un serveur RDP sur la même machine n'ont
/// pas le même mot de passe.
#[must_use]
pub fn keyring_account_pour(protocole: Protocole, user: &str, host: &str, port: u16) -> String {
    let prefixe = match protocole {
        Protocole::Rdp => "rdp",
        Protocole::Vnc => "vnc",
    };
    format!("{prefixe}:{}@{}:{}", user.trim(), host.trim(), port)
}

/// `~/.config/avash/rdp.yaml`
#[must_use]
pub fn hosts_path() -> PathBuf {
    crate::repertoire_configuration()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("avash")
        .join("rdp.yaml")
}

pub fn load_hosts() -> Result<Vec<RdpHost>> {
    load_hosts_from(&hosts_path())
}

/// Un fichier absent n'est pas une erreur : c'est l'etat initial.
pub fn load_hosts_from(path: &Path) -> Result<Vec<RdpHost>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("Lecture de {}", path.display())),
    };
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let tous: Vec<RdpHost> =
        serde_yaml::from_str(&text).with_context(|| format!("{} est illisible", path.display()))?;
    // Un fichier écrit par une version antérieure à la validation d'adresse — ou
    // édité à la main — peut contenir une adresse à espace ou à saut de ligne.
    // La laisser passer casserait la clé du fichier d'empreintes RDP, donc le
    // TOFU, sans que rien ne le signale. Mieux vaut écarter l'entrée.
    Ok(tous.into_iter().filter(|h| h.validate().is_ok()).collect())
}

/// Ecriture atomique : un plantage en cours d'ecriture ne tronque pas le fichier.
pub fn save_hosts_to(path: &Path, hosts: &[RdpHost]) -> Result<()> {
    crate::ecrire_atomiquement(path, serde_yaml::to_string(hosts)?.as_bytes())
}

pub fn upsert_host_in(path: &Path, host: RdpHost) -> Result<Vec<RdpHost>> {
    host.validate()?;
    let mut all = load_hosts_from(path)?;
    match all.iter_mut().find(|h| h.id == host.id) {
        Some(slot) => *slot = host,
        None => all.push(host),
    }
    save_hosts_to(path, &all)?;
    Ok(all)
}

pub fn remove_host_in(path: &Path, id: &str) -> Result<Vec<RdpHost>> {
    let mut all = load_hosts_from(path)?;
    all.retain(|h| h.id != id);
    save_hosts_to(path, &all)?;
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        std::env::temp_dir().join(format!(
            "avash-rdp-{}-{:?}.yaml",
            std::process::id(),
            std::thread::current().id()
        ))
    }

    #[test]
    fn new_derive_le_nom_et_un_id_unique() {
        let a = RdpHost::new("", "10.0.0.1", 3389, "admin", 1280, 800);
        assert_eq!(a.name, "admin@10.0.0.1");
        let b = RdpHost::new("Prod", " 10.0.0.1 ", 3389, "admin", 1280, 800);
        assert_eq!(b.name, "Prod");
        assert_eq!(b.host, "10.0.0.1", "hote rogne");
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn keyring_account_prefixe_rdp() {
        assert_eq!(
            keyring_account("adm", "10.0.0.1", 3389),
            "rdp:adm@10.0.0.1:3389"
        );
    }

    /// Le fichier d'empreintes RDP est en « clé empreinte », découpé au
    /// premier espace : une adresse à espace produit une ligne qu'on ne
    /// retrouve jamais, donc un premier contact perpétuel — le TOFU cesse de
    /// protéger sans que rien ne le dise. Un saut de ligne permettrait d'y
    /// écrire une ligne arbitraire.
    #[test]
    fn validate_refuse_une_adresse_qui_casserait_les_empreintes() {
        for mauvais in [
            "hote avec espace",
            "hote\tab",
            "hote\nautre 0000",
            "hote\rx",
        ] {
            let mut h = RdpHost::new("", "10.0.0.1", 3389, "u", 0, 0);
            h.host = mauvais.to_owned();
            assert!(
                h.validate().is_err(),
                "adresse acceptée alors qu'elle casse rdp_known_hosts : {mauvais:?}"
            );
        }
        // Une adresse normale reste acceptée (IPv6 littéral compris).
        let mut ok = RdpHost::new("", "x", 3389, "u", 0, 0);
        ok.host = "[2001:db8::1]".to_owned();
        assert!(ok.validate().is_ok());
    }

    #[test]
    fn validate_refuse_hote_ou_user_vide() {
        assert!(RdpHost::new("x", " ", 3389, "u", 1, 1).validate().is_err());
        assert!(RdpHost::new("x", "h", 3389, " ", 1, 1).validate().is_err());
        assert!(RdpHost::new("x", "h", 3389, "u", 1, 1).validate().is_ok());
    }

    /// L'authentification VNC classique n'a qu'un mot de passe : un bureau
    /// VNC sans utilisateur est valide, et son nom par défaut est l'adresse.
    #[test]
    fn un_bureau_vnc_se_passe_d_utilisateur() {
        let v = RdpHost::new("", "10.0.0.9", 5900, "", 0, 0).en(Protocole::Vnc);
        assert!(v.validate().is_ok());
        assert_eq!(v.name, "10.0.0.9");
        assert_eq!(v.compte_trousseau(), "vnc:@10.0.0.9:5900");
        // Même adresse, même port : un compte RDP et un compte VNC ne se
        // marchent pas dessus dans le trousseau.
        let r = RdpHost::new("", "10.0.0.9", 5900, "", 0, 0);
        assert_ne!(r.compte_trousseau(), v.compte_trousseau());
        assert!(r.validate().is_err(), "le RDP exige un utilisateur");
    }

    /// Un fichier écrit avant le VNC n'a pas de champ `protocole` : il se
    /// relit en RDP, et un fichier avec le champ le garde.
    #[test]
    fn le_protocole_est_rdp_par_defaut_et_survit_a_l_aller_retour() {
        let ancien =
            "- id: r-1\n  name: A\n  host: h\n  port: 3389\n  user: u\n  width: 0\n  height: 0\n";
        let lus: Vec<RdpHost> = serde_yaml::from_str(ancien).unwrap();
        assert_eq!(lus[0].protocole, Protocole::Rdp);
        let v = RdpHost::new("V", "h", 5900, "", 0, 0).en(Protocole::Vnc);
        let texte = serde_yaml::to_string(std::slice::from_ref(&v)).unwrap();
        assert!(texte.contains("protocole: vnc"), "{texte}");
        let relus: Vec<RdpHost> = serde_yaml::from_str(&texte).unwrap();
        assert_eq!(relus, vec![v]);
        assert_eq!(Protocole::depuis(Some("vnc")), Protocole::Vnc);
        assert_eq!(Protocole::depuis(Some("autre")), Protocole::Rdp);
        assert_eq!(Protocole::depuis(None), Protocole::Rdp);
        assert_eq!(Protocole::Vnc.port_par_defaut(), 5900);
    }

    #[test]
    fn persistance_aller_retour() {
        let p = temp();
        let _ = std::fs::remove_file(&p);
        assert!(load_hosts_from(&p).unwrap().is_empty());
        let a = RdpHost::new("A", "10.0.0.1", 3389, "u", 1280, 800);
        let mut b = RdpHost::new("B", "10.0.0.2", 3390, "v", 1920, 1080);
        upsert_host_in(&p, a.clone()).unwrap();
        let all = upsert_host_in(&p, b.clone()).unwrap();
        assert_eq!(all, vec![a.clone(), b.clone()]);
        assert_eq!(load_hosts_from(&p).unwrap(), all, "relecture identique");
        // Remplacement par id ; le dossier partagé survit à l'aller-retour, et
        // n'apparaît dans le fichier que pour le bureau qui en a un.
        b.width = 2560;
        b.partage = Some("/srv/echange".to_owned());
        let all = upsert_host_in(&p, b.clone()).unwrap();
        assert_eq!(all, vec![a.clone(), b.clone()]);
        assert_eq!(
            load_hosts_from(&p).unwrap(),
            all,
            "le dossier partagé est relu"
        );
        let brut = std::fs::read_to_string(&p).unwrap();
        assert_eq!(brut.matches("partage").count(), 1, "{brut}");
        let all = remove_host_in(&p, &a.id).unwrap();
        assert_eq!(all, vec![b]);
        let _ = std::fs::remove_file(&p);
    }
}
