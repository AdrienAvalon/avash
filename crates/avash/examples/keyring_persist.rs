//! Verifie qu'un secret survit d'un processus a l'autre.
//! Usage : `keyring_persist save|load|forget [compte]`
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
