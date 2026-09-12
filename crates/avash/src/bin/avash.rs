//! Avash CLI v0.1 — liste les hôtes de ~/.ssh/config.
//! Usage : avash [list|run ALIAS CMD]

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
            // Trouvé par l'audit du 7 septembre 2026 : `args.get(3..)` renvoie
            // `Some(&[])` dès que `args[2]` existe, donc l'ancien
            // `ok_or_else("Commande manquante")` était mort et `avash run prod`
            // exécutait `""` sur le serveur (sortie vide, code 0). On refuse
            // aussi une commande faite uniquement d'arguments vides ou d'espaces
            // (`avash run prod ""`). Ce garde-fou passe avant toute résolution
            // d'hôte : `avash run prod` échoue sans tentative réseau.
            let command = args
                .get(3..)
                .filter(|s| !s.iter().all(|a| a.trim().is_empty()))
                .map(|s| s.join(" "))
                .ok_or_else(|| anyhow::anyhow!("Usage : avash run ALIAS 'commande'"))?;
            // `resoudre_hote` : `avash run` doit appliquer les valeurs par
            // défaut d'un `Host *` (User, IdentityFile, Port), comme `ssh`.
            let host = avash::resoudre_hote(alias)
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
    // Contrat K5 de l'audit du 12 septembre 2026 : `parse_ssh_config` rend une
    // liste vide pour un fichier absent. `avash list` garde son conseil et son
    // code 1 dans ce cas : on le lui dit comme une erreur.
    let chemin = avash::ssh_config_path();
    let lu = parse_ssh_config().and_then(|h| {
        if h.is_empty() && !chemin.exists() {
            Err(anyhow::anyhow!("{} n'existe pas", chemin.display()))
        } else {
            Ok(h)
        }
    });
    let hosts = match lu {
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
        // Résolu (blocs à motif compris) : la liste montre l'utilisateur, le
        // port et le rebond effectifs — ceux avec lesquels `avash run` et `ssh`
        // se connectent — au lieu de « ? » quand ils viennent d'un `Host *`.
        let h = avash::resoudre_hote(&h.alias).unwrap_or_else(|| h.clone());
        println!("{}", ligne_hote(&h));
    }
}

/// Compose la ligne que `avash list` imprime pour un hôte.
///
/// Sortie de `cmd_list` par l'audit du 9 septembre 2026 : tant que le format
/// vivait au milieu d'une boucle qui imprime, aucun test ne pouvait le lire, et
/// la relecture a montré qu'on retirait les appels à `sans_controle` sans
/// qu'une assertion du dépôt ne rougisse. `sans_controle` passe sur tout ce qui
/// vient du fichier : Avash refuse désormais d'écrire un caractère de contrôle,
/// mais rien ne garantit que le `~/.ssh/config` lu vienne de lui. Un `HostName
/// srv\x1b]0;PWNED\x07` posé par un autre outil rejouait sa séquence à chaque
/// `avash list`, sans que personne ouvre le fichier. Le port, lui, est un
/// entier : rien à neutraliser.
fn ligne_hote(h: &avash::SshHost) -> String {
    let cible = format!(
        "{}@{}:{}",
        avash::sans_controle(h.user.as_deref().unwrap_or("?")),
        avash::sans_controle(h.hostname.as_deref().unwrap_or(h.alias.as_str())),
        h.port.map_or_else(|| "22".into(), |p| p.to_string())
    );
    let rebond = h
        .proxy_jump
        .as_deref()
        .map(|j| format!("  (via {})", avash::sans_controle(j)))
        .unwrap_or_default();
    format!(
        "  • {:<20} → {cible}{rebond}",
        avash::sans_controle(&h.alias)
    )
}

async fn cmd_run(host: avash::SshHost, command: String) -> anyhow::Result<()> {
    let addr = host.hostname.clone().unwrap_or_else(|| host.alias.clone());
    let auth = avash::ssh::ClientAuth {
        user: host
            .user
            .clone()
            .unwrap_or_else(avash::ssh::current_username),
        key_path: host.identity_file.as_deref().map(avash::developper_tilde),
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

#[cfg(test)]
mod tests_affichage {
    use super::ligne_hote;

    #[test]
    fn la_ligne_de_avash_list_ne_rejoue_aucun_caractere_de_controle() {
        // Trouvé par l'audit du 9 septembre 2026. Avash refuse désormais
        // d'écrire un caractère de contrôle dans `~/.ssh/config`, mais rien ne
        // garantit que le fichier lu vienne de lui : un `HostName
        // srv\x1b]0;PWNED\x07` posé par un import maison, un éditeur ou des
        // dotfiles partagés rejouait sa séquence à chaque `avash list`, sans
        // que personne ouvre le fichier (titre de fenêtre réécrit,
        // presse-papiers manipulé par OSC 52). La composition de la ligne est
        // sortie de `cmd_list` exprès : la relecture du 9 septembre a montré
        // qu'on pouvait retirer les quatre appels à `sans_controle` sans
        // qu'une seule assertion du dépôt rougisse.
        let piege = avash::SshHost {
            alias: "prod\u{1b}]0;PWNED\u{7}".into(),
            hostname: Some("srv\u{1b}]0;PWNED\u{7}".into()),
            user: Some("root\u{7f}".into()),
            proxy_jump: Some("bastion\u{9b}".into()),
            port: Some(2222),
            ..Default::default()
        };
        let ligne = ligne_hote(&piege);
        // ESC, BEL, DEL et un C1 (0x9B, CSI sur un octet) : chacun doit être
        // tombé, quel que soit le champ d'où il vient.
        assert!(
            !ligne.chars().any(char::is_control),
            "il reste un caractère de contrôle dans la ligne : {ligne:?}"
        );
        // Le texte reste montré : on neutralise, on ne censure pas, sans quoi
        // l'utilisateur ne verrait pas qu'un champ est piégé.
        assert!(ligne.contains("PWNED"), "ligne : {ligne:?}");
    }

    #[test]
    fn la_ligne_de_avash_list_reste_lisible_pour_un_hote_ordinaire() {
        // Le pendant du cas précédent : neutraliser ne doit rien changer à
        // l'affichage courant, accents compris.
        let sain = avash::SshHost {
            alias: "prod".into(),
            hostname: Some("prod.exemple.com".into()),
            user: Some("adrien".into()),
            port: Some(2222),
            ..Default::default()
        };
        assert_eq!(
            ligne_hote(&sain),
            "  • prod                 → adrien@prod.exemple.com:2222"
        );
        // Sans utilisateur ni port, `avash list` montre « ? » et 22 ; le
        // rebond, lui, s'ajoute entre parenthèses.
        let par_defaut = avash::SshHost {
            alias: "relais-été".into(),
            proxy_jump: Some("bastion".into()),
            ..Default::default()
        };
        assert_eq!(
            ligne_hote(&par_defaut),
            "  • relais-été           → ?@relais-été:22  (via bastion)"
        );
    }
}
