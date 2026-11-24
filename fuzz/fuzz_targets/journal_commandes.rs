//! Préparation d'une commande pour le journal : masquage des secrets (qui
//! découpe la commande en mots, guillemets compris, et la recoud) et
//! nettoyage. Une commande vient du terminal, donc de n'importe quoi.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let commande = String::from_utf8_lossy(data);
    let masquee = avash::journal_commandes::masquer_secrets(&commande);
    // Le masquage ne fait que remplacer des valeurs : il ne vide jamais une
    // commande qui ne l'était pas.
    if !commande.trim().is_empty() {
        assert!(!masquee.trim().is_empty(), "commande vidée par le masquage");
    }
    if let Some(notee) = avash::journal_commandes::preparer(&commande) {
        assert!(!notee.chars().any(char::is_control), "caractère de contrôle noté");
        assert!(
            notee.chars().count() <= avash::journal_commandes::COMMANDE_MAX + 1,
            "commande notée trop longue"
        );
    }
});
