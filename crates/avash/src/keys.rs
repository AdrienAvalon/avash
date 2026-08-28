//! Generation et deploiement de cles SSH — l'equivalent de `ssh-keygen`
//! et `ssh-copy-id`, sans quitter Avash.

use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};

/// Une cle privee presente dans `~/.ssh`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct KeyEntry {
    pub name: String,
    pub path: String,
    /// Ligne publique complete, telle qu'elle doit atterrir dans
    /// `authorized_keys` cote serveur.
    pub public_line: Option<String>,
    /// Permissions du fichier prive, en octal (ex. "600").
    pub mode: String,
}

/// Repertoire `~/.ssh`, cree au besoin avec les droits qu'OpenSSH exige.
pub fn ssh_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("Répertoire personnel introuvable"))?;
    let dir = home.join(".ssh");
    if !dir.exists() {
        std::fs::create_dir_all(&dir).with_context(|| format!("Création de {}", dir.display()))?;
        set_mode(&dir, 0o700)?;
    }
    Ok(dir)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .with_context(|| format!("Droits {mode:o} sur {}", path.display()))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(()) // Windows gere les ACL differemment.
}

#[cfg(unix)]
fn mode_of(path: &Path) -> String {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| format!("{:o}", m.permissions().mode() & 0o777))
        .unwrap_or_else(|_| "?".into())
}

#[cfg(not(unix))]
fn mode_of(_path: &Path) -> String {
    "-".into()
}

/// Liste les cles privees de `~/.ssh` (celles qui ont un `.pub` associe).
pub fn list_keys() -> Result<Vec<KeyEntry>> {
    let dir = ssh_dir()?;
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).with_context(|| format!("Lecture de {}", dir.display()))? {
        let path = entry?.path();
        if !path.is_file() {
            continue;
        }
        // On part des .pub : une cle privee sans publique n'est pas deployable.
        if path.extension().and_then(|e| e.to_str()) != Some("pub") {
            continue;
        }
        let private = path.with_extension("");
        if !private.is_file() {
            continue;
        }
        let name = private
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        out.push(KeyEntry {
            public_line: std::fs::read_to_string(&path)
                .ok()
                .map(|s| s.trim().to_string()),
            mode: mode_of(&private),
            path: private.to_string_lossy().into_owned(),
            name,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Genere une paire ed25519 dans `~/.ssh/<name>` + `<name>.pub`.
///
/// Refuse d'ecraser une cle existante : perdre une cle privee, c'est perdre
/// l'acces a tous les serveurs qui la connaissent.
pub fn generate(name: &str, comment: &str) -> Result<KeyEntry> {
    let name = name.trim();
    if name.is_empty() {
        return Err(anyhow!("Le nom de la clé est vide."));
    }
    // Le nom finit dans un chemin : pas de traversee ni de separateur.
    if name.contains(['/', '\\', '\0']) || name == "." || name == ".." {
        return Err(anyhow!("Nom de clé invalide : {name}"));
    }
    let dir = ssh_dir()?;
    let private = dir.join(name);
    let public = dir.join(format!("{name}.pub"));
    if private.exists() || public.exists() {
        return Err(anyhow!(
            "Une clé nommée « {name} » existe déjà dans {}. \
             Choisis un autre nom : écraser une clé privée coupe l'accès à \
             tous les serveurs qui la connaissent.",
            dir.display()
        ));
    }

    let pair = russh_keys::key::KeyPair::generate_ed25519()
        .ok_or_else(|| anyhow!("Génération de la clé ed25519 impossible"))?;

    let mut pem = Vec::new();
    russh_keys::encode_pkcs8_pem(&pair, &mut pem).context("Encodage de la clé privée")?;
    std::fs::write(&private, &pem).with_context(|| format!("Écriture de {}", private.display()))?;
    // Avant tout le reste : une cle privee lisible par d'autres est refusee
    // par OpenSSH, et exposee entre-temps.
    set_mode(&private, 0o600)?;

    let pubkey = pair
        .clone_public_key()
        .context("Extraction de la clé publique")?;
    let mut line = Vec::new();
    russh_keys::write_public_key_base64(&mut line, &pubkey)
        .context("Encodage de la clé publique")?;
    let mut line = String::from_utf8(line).context("Clé publique non UTF-8")?;
    let line = {
        let trimmed = line.trim_end();
        let comment = comment.trim();
        line = if comment.is_empty() {
            format!("{trimmed}\n")
        } else {
            format!("{trimmed} {comment}\n")
        };
        line
    };
    std::fs::write(&public, line.as_bytes())
        .with_context(|| format!("Écriture de {}", public.display()))?;
    set_mode(&public, 0o644)?;

    Ok(KeyEntry {
        name: name.to_string(),
        path: private.to_string_lossy().into_owned(),
        public_line: Some(line.trim().to_string()),
        mode: mode_of(&private),
    })
}

/// Commande shell qui installe une cle publique dans `authorized_keys`.
///
/// Reprend ce que fait `ssh-copy-id` : cree `~/.ssh` avec les bons droits,
/// n'ajoute la ligne que si elle est absente (relancer le deploiement ne
/// duplique donc rien), et corrige les permissions qu'OpenSSH exige.
pub fn deploy_command(public_line: &str) -> Result<String> {
    let line = public_line.trim();
    if line.is_empty() {
        return Err(anyhow!("Clé publique vide."));
    }
    // La ligne part dans un shell distant : on interdit tout ce qui pourrait
    // en sortir. Une cle publique OpenSSH n'a jamais besoin de ces caracteres.
    if line.contains('\n') || line.contains('\r') || line.contains('\'') {
        return Err(anyhow!("Clé publique malformée : caractère interdit."));
    }
    if !line.starts_with("ssh-") && !line.starts_with("ecdsa-") {
        return Err(anyhow!(
            "Ceci ne ressemble pas à une clé publique OpenSSH : {}",
            &line[..line.len().min(24)]
        ));
    }
    Ok(format!(
        "set -e; \
         mkdir -p ~/.ssh && chmod 700 ~/.ssh; \
         touch ~/.ssh/authorized_keys && chmod 600 ~/.ssh/authorized_keys; \
         grep -qxF '{line}' ~/.ssh/authorized_keys \
           && echo AVASH_DEJA_PRESENTE \
           || {{ printf '%s\\n' '{line}' >> ~/.ssh/authorized_keys && echo AVASH_AJOUTEE; }}"
    ))
}

/// Interprete la sortie de `deploy_command`.
pub fn interpret_deploy(output: &str) -> Result<&'static str> {
    if output.contains("AVASH_AJOUTEE") {
        Ok("Clé installée sur le serveur.")
    } else if output.contains("AVASH_DEJA_PRESENTE") {
        Ok("La clé était déjà autorisée sur ce serveur.")
    } else {
        Err(anyhow!(
            "Le serveur n'a pas confirmé l'installation. Sortie : {}",
            output.trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deploy_command_est_idempotente() {
        let cmd = deploy_command("ssh-ed25519 AAAAC3Nz adrien@pc").unwrap();
        // grep -qxF garantit qu'un second deploiement n'ajoute pas de doublon.
        assert!(cmd.contains("grep -qxF"), "{cmd}");
        assert!(cmd.contains("AVASH_DEJA_PRESENTE"));
        assert!(cmd.contains("AVASH_AJOUTEE"));
    }

    #[test]
    fn deploy_command_pose_les_droits_exiges_par_openssh() {
        let cmd = deploy_command("ssh-ed25519 AAAAC3Nz").unwrap();
        assert!(
            cmd.contains("chmod 700 ~/.ssh"),
            "OpenSSH refuse un ~/.ssh trop ouvert"
        );
        assert!(cmd.contains("chmod 600 ~/.ssh/authorized_keys"));
    }

    #[test]
    fn deploy_command_refuse_une_injection_shell() {
        // La ligne part dans un shell distant entre apostrophes : une
        // apostrophe ou un saut de ligne permettrait d'en sortir.
        for mechant in [
            "ssh-ed25519 AAA' ; rm -rf ~ ; echo '",
            "ssh-ed25519 AAA\nrm -rf ~",
            "ssh-ed25519 AAA\r\nwhoami",
        ] {
            assert!(
                deploy_command(mechant).is_err(),
                "devrait etre refuse : {mechant:?}"
            );
        }
    }

    #[test]
    fn deploy_command_refuse_ce_qui_n_est_pas_une_cle() {
        assert!(deploy_command("").is_err());
        assert!(deploy_command("   ").is_err());
        assert!(deploy_command("bonjour").is_err());
    }

    #[test]
    fn deploy_command_accepte_les_formats_openssh_courants() {
        for bon in [
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5 a@b",
            "ssh-rsa AAAAB3NzaC1yc2E",
            "ecdsa-sha2-nistp256 AAAAE2VjZHNh",
        ] {
            assert!(deploy_command(bon).is_ok(), "devrait passer : {bon}");
        }
    }

    #[test]
    fn interpret_deploy_distingue_ajout_et_doublon() {
        assert!(interpret_deploy("AVASH_AJOUTEE\n")
            .unwrap()
            .contains("installée"));
        assert!(interpret_deploy("AVASH_DEJA_PRESENTE\n")
            .unwrap()
            .contains("déjà"));
    }

    #[test]
    fn interpret_deploy_signale_une_sortie_inattendue() {
        // Un serveur qui repond autre chose (sudo, shell restreint, quota) ne
        // doit pas passer pour un succes.
        let e = interpret_deploy("Permission denied")
            .unwrap_err()
            .to_string();
        assert!(e.contains("Permission denied"), "{e}");
    }

    #[test]
    fn generate_refuse_un_nom_dangereux() {
        for mauvais in ["../evasion", "a/b", "", "  ", ".", ".."] {
            assert!(
                generate(mauvais, "test").is_err(),
                "devrait etre refuse : {mauvais:?}"
            );
        }
    }
    /// HOME est global au processus : ces tests doivent etre serialises.
    static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct HomeGuard {
        previous: Option<String>,
        dir: PathBuf,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(h) => std::env::set_var("HOME", h),
                None => std::env::remove_var("HOME"),
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn temp_home() -> HomeGuard {
        let lock = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "avash-keys-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let previous = std::env::var("HOME").ok();
        std::env::set_var("HOME", &dir);
        HomeGuard {
            previous,
            dir,
            _lock: lock,
        }
    }

    #[test]
    fn generate_produit_une_paire_utilisable() {
        let _h = temp_home();
        let k = generate("id_test", "adrien@avash").unwrap();

        let private = PathBuf::from(&k.path);
        let public = private.with_extension("pub");
        assert!(private.is_file(), "cle privee absente");
        assert!(public.is_file(), "cle publique absente");

        // La cle privee doit etre lisible par son seul proprietaire :
        // OpenSSH refuse de s'en servir autrement.
        assert_eq!(k.mode, "600", "droits de la cle privee");

        let line = std::fs::read_to_string(&public).unwrap();
        assert!(
            line.starts_with("ssh-ed25519 "),
            "format OpenSSH attendu : {line}"
        );
        assert!(
            line.trim_end().ends_with("adrien@avash"),
            "commentaire absent : {line}"
        );

        // Et elle doit se relire : une cle qu'on ne peut pas recharger ne
        // sert a rien.
        russh_keys::load_secret_key(&private, None).expect("cle privee illisible");
    }

    #[test]
    fn generate_refuse_d_ecraser_une_cle_existante() {
        let _h = temp_home();
        generate("id_unique", "x").unwrap();
        let e = generate("id_unique", "x").unwrap_err().to_string();
        assert!(e.contains("existe déjà"), "{e}");
        // Le message doit expliquer POURQUOI on refuse.
        assert!(e.contains("coupe l'accès"), "{e}");
    }

    #[test]
    fn list_keys_ne_retient_que_les_paires_completes() {
        let _h = temp_home();
        generate("complete", "x").unwrap();
        // Une privee orpheline, sans .pub : non deployable, donc ignoree.
        std::fs::write(ssh_dir().unwrap().join("orpheline"), b"x").unwrap();

        let noms: Vec<_> = list_keys().unwrap().into_iter().map(|k| k.name).collect();
        assert!(noms.contains(&"complete".to_string()), "{noms:?}");
        assert!(!noms.contains(&"orpheline".to_string()), "{noms:?}");
    }

    #[test]
    fn la_cle_generee_est_deployable_telle_quelle() {
        // Boucle complete : ce que generate() produit doit passer la
        // validation de deploy_command sans retouche.
        let _h = temp_home();
        let k = generate("id_boucle", "adrien@pc").unwrap();
        let cmd = deploy_command(k.public_line.as_ref().unwrap()).unwrap();
        assert!(cmd.contains("authorized_keys"));
    }
}
