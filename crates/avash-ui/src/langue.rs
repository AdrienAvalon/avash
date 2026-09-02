//! Langue imposée par l'environnement.
//!
//! Le front suit un choix mémorisé, sinon la locale du système. Sur une
//! machine où la locale n'est pas installée — les machines d'intégration
//! continue, typiquement —, la webview démarre en anglais quoi que disent
//! `LANG` et `LANGUAGE`. `AVASH_LANGUE=fr` (ou `en`) tranche, avant le premier
//! script de la page : un greffon Tauri injecte `window.__AVASH_LANGUE`, que
//! `lireLangue()` lit en premier après le choix mémorisé.

use tauri::plugin::{Builder, TauriPlugin};
use tauri::Runtime;

/// Le script à injecter pour une valeur d'environnement, ou rien si elle est
/// absente ou inconnue : une valeur inattendue ne doit ni casser la page ni
/// choisir une langue.
#[must_use]
pub fn script(valeur: Option<&str>) -> Option<String> {
    let langue = valeur?.trim().to_ascii_lowercase();
    match langue.as_str() {
        "fr" | "en" => Some(format!("window.__AVASH_LANGUE = \"{langue}\";")),
        _ => None,
    }
}

/// Le greffon : sans `AVASH_LANGUE`, il n'injecte rien.
#[must_use]
pub fn plugin<R: Runtime>() -> TauriPlugin<R> {
    let mut b = Builder::new("langue");
    if let Some(s) = script(std::env::var("AVASH_LANGUE").ok().as_deref()) {
        b = b.js_init_script(s);
    }
    b.build()
}

#[cfg(test)]
mod tests {
    use super::script;

    #[test]
    fn seules_les_deux_langues_connues_donnent_un_script() {
        assert_eq!(
            script(Some("fr")).as_deref(),
            Some("window.__AVASH_LANGUE = \"fr\";")
        );
        assert_eq!(
            script(Some(" EN ")).as_deref(),
            Some("window.__AVASH_LANGUE = \"en\";")
        );
        assert!(script(Some("de")).is_none());
        assert!(
            script(Some("fr\"; alert(1); //")).is_none(),
            "rien n'est injecté tel quel"
        );
        assert!(script(None).is_none());
    }
}
