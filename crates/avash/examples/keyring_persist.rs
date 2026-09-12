//! Verifie qu'un secret survit d'un processus a l'autre.
//! Usage : `keyring_persist save|load|forget [compte]`

// Programme d'essai lancé à la main : un décor qui échoue doit s'arrêter net,
// d'où les `unwrap` et `expect`, que le lint `unwrap_used` signalerait.
#![allow(clippy::unwrap_used, clippy::expect_used)]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let acc = args
        .get(2)
        .map_or("avash-persistance@test:22", String::as_str);
    match args.get(1).map(String::as_str) {
        Some("save") => println!("{:?}", avash::secrets::save(acc, "persiste")),
        Some("load") => println!(
            "{:?}",
            avash::secrets::load(acc).map(|p| format!("<{} car.>", p.len()))
        ),
        Some("forget") => println!("{:?}", avash::secrets::forget(acc)),
        _ => println!("save|load|forget [compte]"),
    }
}
