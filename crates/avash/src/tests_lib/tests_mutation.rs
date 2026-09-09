use super::{parse_config_str, split_proxy_jump};

/// Générateur déterministe (LCG) : une suite rejouable, aucun crate de plus.
struct Graine(u64);
impl Graine {
    fn suivant(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
    fn entre(&mut self, borne: usize) -> usize {
        (self.suivant() as usize) % borne.max(1)
    }
}

const SOUCHE: &str = "\
# Configuration réaliste, avec ce que le parseur sait lire.
Include ~/.ssh/conf.d/*.conf
Host prod
  HostName 10.0.0.7
  User adrien
  Port 2222
  IdentityFile ~/.ssh/id_ed25519
  ProxyJump bastion, relais:2200
  #Tags: prod, web
  #Folder: clients/acme
Host bastion
  HostName bastion.exemple.net
  User rebond
Match host *.interne
  ProxyJump none
Host *
  ServerAliveInterval 30
";

/// Fragments qui visent les chemins du parseur : mots-clés, séparateurs,
/// commentaires porteurs de sens, valeurs vides, octets hostiles.
const FRAGMENTS: &[&str] = &[
    "Host ",
    "Host\t",
    "Match ",
    "Include ",
    "ProxyJump ",
    "#Tags: ",
    "#Folder: ",
    "Port ",
    "Port 99999",
    "Port 0",
    ":0",
    "\n",
    "\r\n",
    "\0",
    "  ",
    "=",
    "\"",
    "*",
    "?",
    ",",
    "../",
    "é",
    "\u{feff}",
    "\u{2028}",
    "none",
];

fn muter(graine: &mut Graine, base: &str) -> String {
    let mut octets: Vec<u8> = base.as_bytes().to_vec();
    for _ in 0..=graine.entre(6) {
        match graine.entre(6) {
            0 if !octets.is_empty() => {
                let i = graine.entre(octets.len());
                octets[i] = graine.suivant() as u8;
            }
            1 if !octets.is_empty() => {
                let i = graine.entre(octets.len());
                octets.truncate(i);
            }
            2 => {
                let i = graine.entre(octets.len() + 1);
                let f = FRAGMENTS[graine.entre(FRAGMENTS.len())];
                octets.splice(i..i, f.bytes());
            }
            3 if octets.len() > 2 => {
                let a = graine.entre(octets.len());
                let b = a + graine.entre(octets.len() - a);
                let bloc: Vec<u8> = octets[a..b].to_vec();
                let i = graine.entre(octets.len());
                octets.splice(i..i, bloc);
            }
            4 if octets.len() > 2 => {
                let a = graine.entre(octets.len());
                let b = a + graine.entre(octets.len() - a);
                octets.drain(a..b);
            }
            _ => {
                let i = graine.entre(octets.len() + 1);
                let n = 1 + graine.entre(64);
                octets.splice(i..i, std::iter::repeat_n(b'A', n));
            }
        }
    }
    // Le parseur reçoit une `&str` (conversion lossy des octets mutés). À ne
    // pas confondre avec `read_to_string`, qui n'est PAS lossy : il rend
    // `Err(InvalidData)` sur un octet non UTF-8. Ce banc n'exerce donc que
    // `parse_config_str`, pas le vrai comportement de lecture ; le refus
    // d'un `config` non UTF-8 est couvert par les tests de `save_tests`.
    String::from_utf8_lossy(&octets).into_owned()
}

/// Aucune mutation ne fait paniquer le parseur, et ce qu'il rend reste
/// cohérent : un alias jamais vide, un port jamais nul, des rebonds sans
/// espace autour.
#[test]
fn aucun_fichier_mute_ne_fait_paniquer_le_parseur() {
    let mut graine = Graine(0x5eed_0002_0926);
    let mut hotes_vus = 0usize;
    for _ in 0..2_000 {
        let contenu = muter(&mut graine, SOUCHE);
        let hotes = parse_config_str(&contenu);
        for h in &hotes {
            assert!(!h.alias.is_empty(), "alias vide pour :\n{contenu}");
            assert!(!h.alias.contains(['\n', '\r']), "alias multiligne");
            assert_ne!(h.port, Some(0), "port nul accepté pour :\n{contenu}");
            if let Some(pj) = &h.proxy_jump {
                for hop in split_proxy_jump(pj) {
                    assert_eq!(hop.host.trim(), hop.host, "rebond non rogné : {hop:?}");
                    assert!(
                        !hop.host.starts_with('['),
                        "crochet IPv6 gardé dans l'hôte : {hop:?}"
                    );
                }
            }
        }
        hotes_vus += hotes.len();
    }
    assert!(
        hotes_vus > 0,
        "les mutations ont tué toutes les entrées : test sans portée"
    );
}

/// Trouvé par cargo-fuzz en quelques secondes, là où 2 000 mutations
/// n'avaient jamais produit « Port 0 » : OpenSSH le refuse, nous le
/// lisions. Un port nul n'est pas un port, ni pour l'hôte ni pour un
/// rebond.
#[test]
fn le_port_zero_n_est_pas_un_port() {
    let hotes = parse_config_str("Host a\n  HostName a.local\n  Port 0\n");
    assert_eq!(hotes.len(), 1);
    assert_eq!(hotes[0].port, None);
    let hops = split_proxy_jump("relais:0, autre:2200");
    assert_eq!(hops.len(), 2);
    assert_eq!(hops[0].host, "relais:0");
    assert_eq!(hops[0].port, None);
    assert_eq!(hops[1].port, Some(2200));
}

/// Les mutations sont rejouables : deux exécutions rendent la même suite.
#[test]
fn la_suite_de_mutations_est_deterministe() {
    let a: Vec<String> = {
        let mut g = Graine(42);
        (0..20).map(|_| muter(&mut g, SOUCHE)).collect()
    };
    let b: Vec<String> = {
        let mut g = Graine(42);
        (0..20).map(|_| muter(&mut g, SOUCHE)).collect()
    };
    assert_eq!(a, b);
}
