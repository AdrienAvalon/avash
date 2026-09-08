//! Le filtre de motif des directives `Include` (`glob_match`), fonction pure
//! exercée directement — contrairement à `expand_include`, qui fait des E/S
//! disque et n'est pas fuzzable en l'état. L'entrée est coupée au premier octet
//! NUL : ce qui précède est le motif, ce qui suit le nom de fichier. La
//! propriété tenue par `-timeout` : aucune entrée ne fait partir `glob_match` en
//! retour arrière exponentiel (le cas `*a*a…*b` sur un long nom `aaaa…`).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (motif, nom) = match data.iter().position(|&b| b == 0) {
        Some(i) => (&data[..i], &data[i + 1..]),
        None => (data, &data[..0]),
    };
    let motif = String::from_utf8_lossy(motif);
    let nom = String::from_utf8_lossy(nom);
    // On veut surtout qu'il rende la main (pas de panique, pas de boucle) : le
    // résultat lui-même n'est pas contraint, sauf l'invariant trivial ci-dessous.
    let _ = avash::glob_match(&motif, &nom);
    // Un motif sans joker ne correspond qu'à lui-même : garde-fou minimal contre
    // une régression qui casserait la correspondance littérale.
    if !motif.contains(['*', '?']) {
        assert!(
            avash::glob_match(&motif, &motif),
            "motif littéral ne correspond pas à lui-même : {motif:?}"
        );
    }
});
