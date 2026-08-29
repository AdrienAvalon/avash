//! Avash CLI v0.1 — liste les hôtes de ~/.ssh/config.
//! Usage : avash [list|connect ALIAS|run ALIAS CMD]

use avash::parse_ssh_config;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(std::string::String::as_str) {
        Some("list") | None => {
            cmd_list();
            Ok(())
        }
        Some("run") => {
            // run ALIAS CMD… : connexion réelle via russh, exécution, sortie
            let alias = args
                .get(2)
                .ok_or_else(|| anyhow::anyhow!("Usage : avash run ALIAS 'commande'"))?;
            let command = args
                .get(3..)
                .map(|s| s.join(" "))
                .ok_or_else(|| anyhow::anyhow!("Commande manquante"))?;
            let host = avash::parse_ssh_config()?
                .into_iter()
                .find(|h| h.alias == *alias)
                .ok_or_else(|| anyhow::anyhow!("Hôte introuvable : {alias}"))?;
            // Le moteur SSH est tokio : runtime dédié sur ce thread.
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            rt.block_on(cmd_run(host, command))
        }
        Some(other) => {
            eprintln!("Commande inconnue : {other}. Usage : avash [list|run ALIAS CMD]");
            std::process::exit(2);
        }
    }
}

fn cmd_list() {
    let hosts = match parse_ssh_config() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("\n  😼 Avash n'a trouvé aucun ~/.ssh/config lisible.");
            eprintln!("     Crée-le : mkdir -p ~/.ssh && touch ~/.ssh/config");
            eprintln!("     Détail : {e}\n");
            std::process::exit(1);
        }
    };
    println!("{:-<62}", "");
    println!(
        " Avash 😼 — {} hôtes trouvés dans ~/.ssh/config",
        hosts.len()
    );
    println!("{:-<62}", "");
    for h in &hosts {
        let target = format!(
            "{}@{}:{}",
            h.user.as_deref().unwrap_or("?"),
            h.hostname.as_deref().unwrap_or(h.alias.as_str()),
            h.port.map_or_else(|| "22".into(), |p| p.to_string())
        );
        let jump = h
            .proxy_jump
            .as_ref()
            .map(|j| format!("  (via {j})"))
            .unwrap_or_default();
        println!("  • {:<20} → {}{}", h.alias, target, jump);
    }
}

async fn cmd_run(host: avash::SshHost, command: String) -> anyhow::Result<()> {
    let addr = host.hostname.clone().unwrap_or_else(|| host.alias.clone());
    let auth = avash::ssh::ClientAuth {
        user: host
            .user
            .clone()
            .unwrap_or_else(avash::ssh::current_username),
        key_path: host.identity_file.as_ref().map(std::path::PathBuf::from),
        password: None,
    };
    let mut session =
        avash::ssh::AvashSession::connect(&addr, host.port.unwrap_or(22), &auth).await?;
    let (stdout, code) = session.run(&command).await?;
    print!("{stdout}");
    session.disconnect().await?;
    // Un code de sortie Unix tient sur 8 bits. Borner evite le
    // debordement u32 -> i32 signale par clippy, et reflete la realite.
    std::process::exit(i32::from((code & 0xFF) as u8));
}
