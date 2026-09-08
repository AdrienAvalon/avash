//! Vérifie que les commentaires de documentation (`///`) sont rattachés à
//! l'élément qu'ils décrivent, et non à celui qui les suit.
//!
//! Trouvé par l'audit du 7 septembre 2026 : le découpage de `main.rs` en
//! modules (commit 488851f) a laissé plusieurs blocs `///` accrochés à
//! l'élément suivant. `rustdoc` les publiait alors sur le mauvais item : la
//! justification TOFU (« premier contact ») du fichier de confiance
//! `rdp_known_hosts` (`chemin_empreintes`) documentait `chemin_canal_graphique`
//! (qui rend `Option` sans jamais échouer sur ce point), et deux blocs
//! documentaient un `const` ou une `struct` au lieu de la fonction voisine ;
//! deux autres décrivaient un item disparu du module. Ce test lit le source et
//! affirme le rattachement, ce qu'aucun test de comportement ne pouvait voir.

/// Extrait le bloc `///` contigu qui précède immédiatement la première ligne
/// contenant `ancre`, en traversant d'éventuels attributs (`#[test]`,
/// `#[must_use]`) posés entre le doc et l'item.
fn doc_avant(source: &str, ancre: &str) -> String {
    let lignes: Vec<&str> = source.lines().collect();
    let idx = lignes
        .iter()
        .position(|l| l.contains(ancre))
        .unwrap_or_else(|| panic!("ancre absente du source : {ancre}"));
    let mut doc: Vec<&str> = Vec::new();
    for i in (0..idx).rev() {
        let ligne = lignes[i].trim_start();
        if let Some(reste) = ligne.strip_prefix("///") {
            doc.push(reste.trim());
        } else if ligne.starts_with("#[") {
            // Un attribut entre le doc et l'item ne coupe pas le rattachement.
            continue;
        } else {
            break;
        }
    }
    doc.reverse();
    doc.join("\n")
}

#[test]
fn le_paragraphe_tofu_documente_le_fichier_de_confiance_pas_le_canal_graphique() {
    let src = include_str!("../src/empreintes.rs");
    let canal = doc_avant(src, "pub fn chemin_canal_graphique(");
    assert!(
        !canal.to_lowercase().contains("premier contact"),
        "la justification TOFU ne doit pas documenter chemin_canal_graphique : {canal:?}"
    );
    assert!(
        canal.contains("canal graphique"),
        "chemin_canal_graphique doit garder sa propre phrase : {canal:?}"
    );
    let empreintes = doc_avant(src, "fn chemin_empreintes(");
    assert!(
        empreintes.contains("premier contact"),
        "la justification TOFU doit documenter chemin_empreintes : {empreintes:?}"
    );
}

#[test]
fn la_premiere_entree_fait_foi_porte_sa_propre_documentation() {
    let src = include_str!("../src/empreintes.rs");
    let premiere = doc_avant(src, "fn la_premiere_entree_fait_foi(");
    assert!(
        premiere.contains("première qui fait foi"),
        "la_premiere_entree_fait_foi doit porter le doc « première qui fait foi » : {premiere:?}"
    );
    let avash_home = doc_avant(src, "fn avash_home_detourne_le_fichier_de_confiance(");
    assert!(
        !avash_home.contains("première qui fait foi"),
        "le doc de la_premiere_entree ne doit pas rester sur avash_home : {avash_home:?}"
    );
}

#[test]
fn le_marqueur_nla_ne_traine_pas_le_commentaire_orphelin_du_dessin() {
    let src = include_str!("../src/connexion.rs");
    let nla = doc_avant(src, "const NLA_INDISPONIBLE");
    assert!(
        !nla.contains("FRAME"),
        "NLA_INDISPONIBLE ne doit pas hériter du commentaire orphelin sur le dessin : {nla:?}"
    );
    assert!(
        nla.to_lowercase().contains("marqueur"),
        "NLA_INDISPONIBLE doit garder sa propre description : {nla:?}"
    );
}

#[test]
fn l_annonce_egfx_et_le_delai_ont_chacun_leur_documentation() {
    let src = include_str!("../src/session.rs");
    let annonce = doc_avant(src, "fn annonce_egfx(");
    assert!(
        annonce.contains("capacités graphiques"),
        "annonce_egfx doit porter le doc « annonce de capacités graphiques » : {annonce:?}"
    );
    let delai = doc_avant(src, "const DELAI_CANAL_PRET");
    assert!(
        delai.contains("presse-papiers"),
        "DELAI_CANAL_PRET doit garder sa propre description du délai : {delai:?}"
    );
    assert!(
        !delai.contains("capacités graphiques"),
        "le doc d'annonce_egfx ne doit pas rester sur DELAI_CANAL_PRET : {delai:?}"
    );
}

#[test]
fn la_struct_poste_ne_traine_pas_le_commentaire_orphelin() {
    let src = include_str!("../src/acces_local.rs");
    let poste = doc_avant(src, "pub struct Poste");
    assert!(
        !poste.contains("Ce qu'une session a donné"),
        "Poste ne doit pas hériter d'un commentaire orphelin d'un item disparu : {poste:?}"
    );
    assert!(
        poste.contains("poste de travail"),
        "Poste doit garder sa propre description : {poste:?}"
    );
}
