//! Les fichiers YAML écrits par les versions déjà installées se relisent à
//! l'identique.
//!
//! Avash a changé de bibliothèque YAML le 22 novembre 2026 : `serde_yaml`
//! (abandonnée en mars 2024, adossée à `unsafe-libyaml`, une traduction
//! mécanique du C) a laissé la place à `serde-saphyr`, maintenue et sans
//! `unsafe`. Ce que l'utilisateur a sur son disque (`rdp.yaml`,
//! `tunnels.yaml`, `snippets.yaml`) a été écrit par l'ancienne : une lecture
//! qui diverge, c'est un bureau, un tunnel ou une commande qui change en
//! silence.
//!
//! Les fichiers de `tests/donnees/*-serde-yaml-0.9.yaml` sont la sortie
//! réelle de `serde_yaml` 0.9.34 pour les valeurs reconstruites ci-dessous,
//! produite lors de la bascule par un essai différentiel (les deux
//! bibliothèques dans le même binaire, lectures croisées dans les deux sens).
//! Ils ne se régénèrent pas : ils sont le témoin de ce que l'ancienne écrivait.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use avash::rdphost::{Protocole, RdpHost};
use avash::snippet::Snippet;
use avash::tunnel::{TunnelDef, TunnelKind};

/// Des chaînes que YAML interprète volontiers autrement que comme du texte :
/// booléens et nuls de YAML 1.1, nombres, dates, indicateurs de structure,
/// espaces de bord, sauts de ligne, caractères de contrôle. Un nom d'extrait
/// ou une commande peut contenir tout cela.
const VACHERIES: &[&str] = &[
    "yes",
    "no",
    "on",
    "off",
    "null",
    "~",
    "true",
    "False",
    "123",
    "012",
    "+1",
    "-1",
    "1e3",
    "0x1F",
    "0o17",
    ".inf",
    ".nan",
    "1.0",
    "2026-11-22",
    "a: b",
    "a:b",
    "# pas un commentaire",
    "x # y",
    " espace devant",
    "espace derriere ",
    "ligne1\nligne2",
    "ligne1\nligne2\n",
    "\nligne",
    "fin\n\n",
    "'apostrophes'",
    "\"guillemets\"",
    "it's",
    "é日本🙂",
    "::1",
    "{{var}}",
    "{a: 1}",
    "[1, 2]",
    "- tiret",
    "-",
    "!tag",
    "&ancre",
    "*alias",
    "%dir",
    "@at",
    "`back`",
    "",
    " ",
    "\t",
    "a\tb",
    "a\r\nb",
    "---",
    "...",
    "|",
    ">",
    "| bloc",
    "? cle",
    ": val",
    ",",
    "a\\b",
    "\\n",
    "\u{0085}",
    "\u{feff}bom",
    "nul\u{0}",
    "\u{7f}",
    "\u{1b}[31m",
    "  indent\n    plus\n",
    "a\n\n\nb",
    "trailing:",
    "http://x.y:8080/a?b=c#d",
    "C:\\Users\\x",
    "<<",
    "=",
    "0",
    "00",
    "1_000",
    "1:30",
    "y",
    "n",
    "N",
];

fn extraits() -> Vec<Snippet> {
    VACHERIES
        .iter()
        .enumerate()
        .map(|(i, s)| Snippet {
            id: format!("id{i}"),
            name: (*s).to_string(),
            command: (*s).to_string(),
            run: i % 2 == 0,
            category: (*s).to_string(),
        })
        .collect()
}

fn bureaux() -> Vec<RdpHost> {
    let mut a = RdpHost::new(
        "Bureau compta",
        "srv-compta.exemple.fr",
        3389,
        "EXEMPLE\\alice",
        1920,
        1080,
    );
    a.id = "a1".into();
    a.folder = "prod/bureaux".into();
    let mut b = RdpHost::new("", "2001:db8::10", 3390, "bob@exemple.fr", 3440, 1440);
    b.id = "b2".into();
    b.sans_nla = true;
    b.tls_herite = true;
    b.partage = Some("/home/bob/Échanges : été".into());
    let mut c = RdpHost::new("vnc: labo #3", "10.0.0.7", 5900, "", 1024, 768);
    c.id = "c3".into();
    c.protocole = Protocole::Vnc;
    vec![a, b, c]
}

fn tunnels() -> Vec<TunnelDef> {
    let mut l = TunnelDef::new(
        "web-1",
        TunnelKind::Local,
        8080,
        "127.0.0.1",
        80,
        "interface: admin",
    );
    l.id = "t1".into();
    let mut r = TunnelDef::new("db-1", TunnelKind::Remote, 0, "::1", 5432, "");
    r.id = "t2".into();
    let mut d = TunnelDef::new(
        "rebond",
        TunnelKind::Dynamic,
        1080,
        "",
        0,
        "SOCKS « maison »",
    );
    d.id = "t3".into();
    vec![l, r, d]
}

fn temoin(nom: &str) -> String {
    let chemin = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/donnees")
        .join(nom);
    std::fs::read_to_string(&chemin).unwrap_or_else(|e| panic!("{} : {e}", chemin.display()))
}

#[test]
fn les_extraits_ecrits_par_l_ancienne_bibliotheque_se_relisent_a_l_identique() {
    let relus: Vec<Snippet> =
        serde_saphyr::from_str(&temoin("snippets-serde-yaml-0.9.yaml")).unwrap();
    let attendus = extraits();
    assert_eq!(relus.len(), attendus.len());
    for (relu, attendu) in relus.iter().zip(&attendus) {
        assert_eq!(relu, attendu);
    }
}

#[test]
fn les_bureaux_ecrits_par_l_ancienne_bibliotheque_se_relisent_a_l_identique() {
    let relus: Vec<RdpHost> = serde_saphyr::from_str(&temoin("rdp-serde-yaml-0.9.yaml")).unwrap();
    assert_eq!(relus, bureaux());
}

#[test]
fn les_tunnels_ecrits_par_l_ancienne_bibliotheque_se_relisent_a_l_identique() {
    let relus: Vec<TunnelDef> =
        serde_saphyr::from_str(&temoin("tunnels-serde-yaml-0.9.yaml")).unwrap();
    assert_eq!(relus, tunnels());
}

/// Ce que la nouvelle bibliothèque écrit, elle le relit : aucune des chaînes
/// piégeuses ne change de valeur ni de type en passant par le disque.
#[test]
fn un_aller_retour_ne_change_aucune_chaine_piegeuse() {
    let attendus = extraits();
    let texte = serde_saphyr::to_string(&attendus).unwrap();
    let relus: Vec<Snippet> = serde_saphyr::from_str(&texte).unwrap();
    assert_eq!(relus, attendus);
    let texte = serde_saphyr::to_string(&bureaux()).unwrap();
    assert_eq!(
        serde_saphyr::from_str::<Vec<RdpHost>>(&texte).unwrap(),
        bureaux()
    );
    let texte = serde_saphyr::to_string(&tunnels()).unwrap();
    assert_eq!(
        serde_saphyr::from_str::<Vec<TunnelDef>>(&texte).unwrap(),
        tunnels()
    );
}

/// La seule divergence trouvée par l'essai différentiel, gardée ici pour
/// qu'elle reste connue et bornée. `serde_yaml` suivait YAML 1.1, où U+2028 et
/// U+2029 sont des fins de ligne : il les écrivait dans une chaîne entre
/// apostrophes suivies de l'indentation de continuation (deux espaces).
/// `serde-saphyr` suit YAML 1.2, où ce sont des caractères ordinaires : il
/// garde les deux espaces. Une valeur ancienne contenant l'un de ces deux
/// séparateurs (texte collé depuis un traitement de texte) gagne donc deux
/// espaces après lui à la première relecture, une seule fois : la nouvelle
/// écriture l'échappe (`\L`, `\P`) et se relit exactement.
#[test]
fn un_separateur_de_ligne_unicode_ancien_gagne_deux_espaces_une_seule_fois() {
    let ancien = "- 'a\u{2028}  b'\n";
    let relu: Vec<String> = serde_saphyr::from_str(ancien).unwrap();
    assert_eq!(relu, ["a\u{2028}  b"]);
    let reecrit = serde_saphyr::to_string(&relu).unwrap();
    let rerelu: Vec<String> = serde_saphyr::from_str(&reecrit).unwrap();
    assert_eq!(rerelu, relu);
}

/// Ce que la bombe développerait si rien ne l'arrêtait : un arbre de listes.
#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
#[allow(dead_code)]
enum Arbre {
    Feuille(String),
    Branche(Vec<Arbre>),
}

/// Un fichier de configuration piégé ne doit pas pouvoir épuiser la mémoire :
/// la « bombe d'alias » (onze niveaux, chacun répète dix fois le précédent,
/// soit cent milliards de feuilles une fois développée) est refusée par le
/// budget du lecteur au lieu d'être développée.
#[test]
fn une_bombe_d_alias_est_refusee_sans_etre_developpee() {
    use std::fmt::Write as _;
    let mut bombe = String::from("a: &a [x, x, x, x, x, x, x, x, x, x]\n");
    let mut precedent = 'a';
    for suivant in 'b'..='k' {
        let alias = format!("*{precedent}");
        let dix = [alias.as_str(); 10].join(", ");
        writeln!(bombe, "{suivant}: &{suivant} [{dix}]").unwrap();
        precedent = suivant;
    }
    let debut = std::time::Instant::now();
    let lu: Result<std::collections::BTreeMap<String, Arbre>, _> = serde_saphyr::from_str(&bombe);
    assert!(lu.is_err(), "la bombe d'alias a été développée");
    assert!(
        debut.elapsed() < std::time::Duration::from_secs(10),
        "refus trop lent"
    );
}
