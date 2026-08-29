//! Combien de messages IPC economise le regroupement ?
//!
//! Rejoue une trace realiste de blocs SSH (relevee par `pty_probe` contre un
//! vrai serveur) et compte les emissions avec et sans regroupement.

fn main() {
    const FLUSH: usize = 16 * 1024; // seuil d'ecoulement, cf. open_on_target

    // Tailles reellement observees a l'ouverture d'un shell fish distant.
    let trace: Vec<usize> = vec![
        83, 1, 374, 282, 123, 127, 167, 374, 2068, 38, 4, 58, 101, 12, 7, 3, 45, 9, 2, 220,
    ];
    // Puis un `cat` d'un fichier : beaucoup de blocs moyens.
    let mut blocs = trace.clone();
    blocs.extend(std::iter::repeat_n(1400usize, 500));
    let total: usize = blocs.iter().sum();

    let sans = blocs.len();
    let mut avec = 0usize;
    let mut acc = 0usize;
    for b in &blocs {
        acc += b;
        if acc >= FLUSH {
            avec += 1;
            acc = 0;
        }
    }
    if acc > 0 {
        avec += 1;
    }

    println!(
        "  volume simule      : {:.0} Ko en {} blocs SSH",
        total as f64 / 1024.0,
        blocs.len()
    );
    println!("  messages IPC sans  : {sans}");
    println!("  messages IPC avec  : {avec}");
    println!(
        "  reduction          : {:.0} %",
        100.0 * (1.0 - avec as f64 / sans as f64)
    );
    println!();
    println!("  (le regroupement temporel de 8 ms n'est pas simule ici :");
    println!("   en interactif il fusionne aussi les rafales de petits blocs)");
}
