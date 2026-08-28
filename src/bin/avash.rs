//! Avash CLI v0.1 — liste les hôtes de ~/.ssh/config.
//! Usage : avash list

use avash::parse_ssh_config;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("list") | None => {
            let hosts = parse_ssh_config()?;
            println!("{:-<62}", "");
            println!(" Avash 😼 — {} hôtes trouvés dans ~/.ssh/config", hosts.len());
            println!("{:-<62}", "");
            for h in &hosts {
                let target = format!(
                    "{}@{}:{}",
                    h.user.as_deref().unwrap_or("?"),
                    h.hostname.as_deref().unwrap_or(h.alias.as_str()),
                    h.port.map(|p| p.to_string()).unwrap_or_else(|| "22".into())
                );
                let jump = h
                    .proxy_jump
                    .as_ref()
                    .map(|j| format!("  (via {})", j))
                    .unwrap_or_default();
                println!("  • {:<20} → {}{}", h.alias, target, jump);
            }
            Ok(())
        }
        Some(other) => {
            eprintln!("Commande inconnue : {other}. Usage : avash [list]");
            std::process::exit(2);
        }
    }
}