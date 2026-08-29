fn main() {
    let acc = "avash-verification-reelle@test:22";
    match avash::secrets::save(acc, "mot-de-passe-de-test") {
        Ok(()) => println!("  ✓ ecriture dans le trousseau"),
        Err(e) => {
            println!("  ✗ ecriture : {e}");
            return;
        }
    }
    match avash::secrets::load(acc) {
        Some(p) if p == "mot-de-passe-de-test" => println!("  ✓ relecture identique"),
        Some(p) => println!("  ✗ relu different : {p}"),
        None => println!("  ✗ rien relu"),
    }
    let _ = avash::secrets::forget(acc);
    match avash::secrets::load(acc) {
        None => println!("  ✓ suppression effective"),
        Some(_) => println!("  ✗ toujours present apres suppression"),
    }
}
