//! Debit du decodeur UTF-8 : il traverse chaque octet de sortie du terminal.
use std::time::Instant;

fn main() {
    // Sortie realiste : melange d'ASCII, d'accents et de sequences ANSI.
    let ligne = "\x1b[32mavalon\x1b[m@\x1b[36mcachyos\x1b[m ~ » déjà vu — 100 % ✓\r\n";
    let source: Vec<u8> = ligne.repeat(20_000).into_bytes();
    let total = source.len();

    for taille in [64usize, 512, 4096, 16384] {
        let mut d = avash_ui_lib::commands::Utf8Stream::default();
        let t = Instant::now();
        let mut sortie = 0usize;
        for bloc in source.chunks(taille) {
            sortie += d.push(bloc).len();
        }
        let dt = t.elapsed();
        let debit = total as f64 / dt.as_secs_f64() / 1e6;
        println!(
            "  blocs de {taille:>5} o : {:>7.1} Mo/s  ({:.1} ms pour {:.1} Mo, {sortie} car.)",
            debit,
            dt.as_secs_f64() * 1e3,
            total as f64 / 1e6
        );
    }
}
