//! Banc M : décodage RLGR3 (entropie RemoteFX) d'une tuile 64×64 dense, le
//! chemin qui plafonnait un bureau RemoteFX plein écran à 10 i/s avant le
//! lecteur de bits par mots (26 novembre 2026 : 108 → 55 µs par tuile).
//! Imprime une ligne `BANC {json}` que scripts/banc.mjs récolte.

// Programme de mesure : un décor qui échoue doit s'arrêter net, d'où les
// `expect`, que le lint `expect_used` signalerait.
#![allow(clippy::expect_used)]
use std::time::Instant;

use ironrdp::graphics::rlgr::{decode, encode};
use ironrdp::pdu::codecs::rfx::EntropyAlgorithm;

/// Pseudo-hasard reproductible (xorshift), le même que les tests du décodeur.
struct Hasard(u64);

impl Hasard {
    fn jusqua(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) % n
    }
}

/// Coefficients d'une tuile chargée : peu de zéros, beaucoup de petites
/// valeurs, quelques grandes (le profil d'un écran qui change partout).
fn dense(h: &mut Hasard, n: usize) -> Vec<i16> {
    (0..n)
        .map(|_| match h.jusqua(10) {
            0..=1 => 0,
            2..=8 => h.jusqua(33) as i16 - 16,
            _ => h.jusqua(2001) as i16 - 1000,
        })
        .collect()
}

fn main() {
    let mut h = Hasard(0x2222_4444_6666_8888);
    let tuiles: Vec<Vec<u8>> = (0..600)
        .map(|_| {
            let entree = dense(&mut h, 4096);
            let mut tuile = vec![0u8; 4096 * 64 + 256];
            let taille = encode(EntropyAlgorithm::Rlgr3, &entree, &mut tuile).expect("encodage");
            tuile.truncate(taille);
            tuile
        })
        .collect();
    let mut sortie = vec![0i16; 4096];
    let mut echantillons = Vec::new();
    for _ in 0..10 {
        let debut = Instant::now();
        for t in &tuiles {
            decode(EntropyAlgorithm::Rlgr3, t, &mut sortie).expect("décodage");
        }
        echantillons.push(format!(
            "{:.2}",
            debut.elapsed().as_secs_f64() * 1e6 / tuiles.len() as f64
        ));
    }
    println!(
        r#"BANC {{"cle":"M/rlgr3-tuile-dense","unite":"µs","sens":"bas","echantillons":[{}]}}"#,
        echantillons.join(",")
    );
}
