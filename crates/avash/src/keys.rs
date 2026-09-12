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
    /// Permissions du fichier prive, en octal (ex. "600"), ou `None` là où le
    /// système n'a pas de bits de permission (Windows, cibles exotiques) : les
    /// droits y passent par une ACL et il n'y a pas de « 600 » à afficher.
    ///
    /// Trouvé par l'audit du 7 septembre 2026 : rendre une chaîne sentinelle
    /// « - » hors Unix faisait afficher au front « - ⚠ OpenSSH exige 600 » sur
    /// chaque clé, y compris une clé qu'Avash venait de restreindre par
    /// `icacls`. `None` (sérialisé `null`) laisse le front distinguer
    /// « inconnu » de « incorrect ».
    pub mode: Option<String>,
}

/// Repertoire `~/.ssh`, cree au besoin avec les droits qu'OpenSSH exige.
pub fn ssh_dir() -> Result<PathBuf> {
    let home =
        crate::repertoire_personnel().ok_or_else(|| anyhow!("Répertoire personnel introuvable"))?;
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

/// Arguments d'`icacls` restreignant un chemin à son seul propriétaire.
///
/// Windows n'a pas de bits de permission : l'équivalent est une liste de
/// contrôle d'accès. `/inheritance:r` coupe l'héritage du dossier parent —
/// sans quoi la clé reste lisible par tout ce à quoi ce dossier donne accès —
/// puis `/grant:r` rétablit un droit unique et complet pour l'utilisateur.
///
/// **`/grant:r` et le couple compte/permission sont DEUX arguments distincts.**
/// Ils étaient collés (`/grant:rutilisateur:F`), ce qu'`icacls` rejette :
/// « Invalid parameter ». Toute création de clé SSH échouait donc sous Windows,
/// et personne ne l'avait vu — cette fonction n'avait aucun test, en dépit du
/// commentaire qui prétendait le contraire.
///
/// Pour un **répertoire**, `(OI)(CI)` fait porter le droit sur ce qu'il
/// contiendra : l'héritage venant d'être coupé, un fichier créé ensuite dans
/// `~/.ssh` n'hériterait sinon d'aucune autorisation.
#[must_use]
pub fn icacls_args(path: &str, user: &str, repertoire: bool) -> Vec<String> {
    let droits = if repertoire { "(OI)(CI)F" } else { "(F)" };
    vec![
        path.to_owned(),
        "/inheritance:r".to_owned(),
        "/grant:r".to_owned(),
        format!("{user}:{droits}"),
    ]
}

/// Sous Windows, on pose une ACL au lieu de bits de permission.
///
/// Sans elle, une clé privée hérite des droits de son dossier : sur un poste
/// partagé elle peut être lisible par d'autres, et OpenSSH pour Windows refuse
/// purement et simplement une clé dont les droits sont trop larges.
/// Le `mode` Unix sert d'intention : tout ce qui est plus restrictif que
/// « lisible par le groupe » devient « propriétaire seul ».
#[cfg(windows)]
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    // 0o077 = bits accordés au groupe et aux autres. S'ils sont nuls, le fichier
    // est privé et mérite une ACL restreinte.
    if mode & 0o077 != 0 {
        return Ok(()); // fichier public (clé .pub) : rien à restreindre
    }
    let user = crate::ssh::current_username();
    let chemin = path.to_string_lossy().to_string();
    let sortie = std::process::Command::new("icacls")
        .args(icacls_args(&chemin, &user, path.is_dir()))
        .output()
        .with_context(|| format!("Restriction des droits sur {}", path.display()))?;
    if !sortie.status.success() {
        return Err(anyhow!(
            "Droits non restreints sur {} : {}",
            path.display(),
            String::from_utf8_lossy(&sortie.stderr).trim()
        ));
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn set_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

// Sous Unix, `mode_of` renvoie toujours `Some` ; le `Some` reste nécessaire
// pour que la signature colle à la variante non-Unix (qui rend `None`) et au
// champ `KeyEntry.mode`.
#[cfg(unix)]
#[allow(clippy::unnecessary_wraps)]
fn mode_of(path: &Path) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;
    Some(std::fs::metadata(path).map_or_else(
        |_| "?".into(),
        |m| format!("{:o}", m.permissions().mode() & 0o777),
    ))
}

/// Hors Unix, il n'y a pas de bits de permission à rendre : `None` plutôt
/// qu'une sentinelle « - » que le front prenait pour des droits incorrects.
#[cfg(not(unix))]
fn mode_of(_path: &Path) -> Option<String> {
    None
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
        // Audit du 12 septembre 2026 (C-SIL-13) : une `.pub` illisible (droits,
        // octets non UTF-8) donnait une clé sans bouton « copier » ni
        // déploiement, sans un mot. La liste la montre toujours, et le journal
        // dit pourquoi.
        let public_line = match std::fs::read_to_string(&path) {
            Ok(s) => Some(s.trim().to_string()),
            Err(e) => {
                tracing::warn!(
                    "Clé publique {} illisible : {e}. La clé est listée sans \
                     ligne publique, donc sans copie ni déploiement.",
                    path.display()
                );
                None
            }
        };
        out.push(KeyEntry {
            public_line,
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
    // Le nom finit dans un chemin : pas de traversee ni de separateur, et sous
    // Windows aucun piège NTFS. Trouvé par l'audit du 7 septembre 2026 :
    // « travail:pro » créait le flux de données alternatif « pro » du fichier
    // « travail » et y logeait la clé privée à l'insu du panneau ; un nom réservé
    // (CON, NUL, COM1…) ou un point/espace final retombait sur un autre fichier.
    // On partage le prédicat de `sftp` (source unique, déjà durci pour Windows).
    if !crate::sftp::nom_d_entree_sur(name) {
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

    // Format OpenSSH natif (et non PKCS#8) : c'est celui qu'attendent
    // ssh-agent, OpenSSH et les autres outils de l'ecosysteme.
    let pair = russh::keys::PrivateKey::random(
        // rand 0.10 : meme version que celle utilisee par ssh-key, sans quoi
        // les traits rand_core ne concordent pas.
        &mut rand::rng(),
        russh::keys::Algorithm::Ed25519,
    )
    .map_err(|e| anyhow!("Génération de la clé ed25519 impossible : {e}"))?;

    let pem = pair
        .to_openssh(russh::keys::ssh_key::LineEnding::LF)
        .context("Encodage de la clé privée")?;
    // La clé privée naît en 0600, et jamais avec l'umask : `fs::write` la
    // créait lisible par tous puis `set_mode` la resserrait — la fenêtre était
    // brève, mais c'est précisément le défaut que `ecrire_atomiquement` ferme
    // pour les fichiers de configuration, et une clé privée le mérite encore
    // plus. `create_new` double la garde d'existence ci-dessus : même une course
    // avec un autre processus n'écrasera pas une clé.
    ecrire_prive(&private, pem.as_bytes())?;
    // Sous Windows, les droits sont une liste de contrôle d'accès posée après
    // coup ; sous Unix, `ecrire_prive` a déjà fait le nécessaire.
    set_mode(&private, 0o600)?;

    let mut pubkey = pair.public_key().clone();
    // Le commentaire identifie la machine d'origine dans authorized_keys.
    let comment = comment.trim();
    if !comment.is_empty() {
        pubkey.set_comment(comment);
    }
    let line = pubkey.to_openssh().context("Encodage de la clé publique")?;
    let line = format!("{}\n", line.trim_end());
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

/// Écrit un fichier qui n'existe pas encore, lisible par son seul propriétaire
/// dès sa création.
fn ecrire_prive(path: &Path, contenu: &[u8]) -> Result<()> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut f = options
        .open(path)
        .with_context(|| format!("Création de {}", path.display()))?;
    f.write_all(contenu)
        .with_context(|| format!("Écriture de {}", path.display()))?;
    f.sync_all()
        .with_context(|| format!("Synchronisation de {}", path.display()))
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
    // La ligne part dans un shell distant, entre apostrophes. Un saut de ligne
    // ou un octet nul ne peut pas être cité sans casser la commande : on les
    // refuse. Trouvé par l'audit du 7 septembre 2026 : l'octet nul manquait au
    // filtre (seuls `\n`, `\r`, `'` y étaient), passait la garde et se faisait
    // tronquer par le shell distant, posant une ligne différente de l'affichée.
    if line.contains('\n') || line.contains('\r') || line.contains('\0') {
        return Err(anyhow!("Clé publique malformée : caractère interdit."));
    }
    if !line.starts_with("ssh-") && !line.starts_with("ecdsa-") {
        return Err(anyhow!(
            "Ceci ne ressemble pas à une clé publique OpenSSH : {}",
            // Aperçu en caractères, pas en octets : audit du 12 septembre
            // 2026 (C-panique-3), `&line[..24]` paniquait quand le 24e octet
            // tombait au milieu d'un caractère accentué, et `key_deploy`
            // restait sans réponse.
            line.chars().take(24).collect::<String>()
        ));
    }
    // L'apostrophe est légitime dans un commentaire (« clé d'Adrien », ou un
    // compte Windows « O'Brien » dans le commentaire par défaut) : la refuser
    // rendait non déployable une clé qu'Avash venait de générer. On l'échappe
    // plutôt (« ' » → « '\'' »), comme `citer()` (sftp) et `shellQuote` (web) le
    // font déjà. `grep -qxF '…'` et `printf '%s\n' '…'` reçoivent tous deux la
    // même chaîne réassemblée, donc l'idempotence de `grep` tient. Trouvé par
    // l'audit du 7 septembre 2026.
    let citee = line.replace('\'', "'\\''");
    Ok(format!(
        "set -e; \
         mkdir -p ~/.ssh && chmod 700 ~/.ssh; \
         touch ~/.ssh/authorized_keys && chmod 600 ~/.ssh/authorized_keys; \
         grep -qxF '{citee}' ~/.ssh/authorized_keys \
           && echo AVASH_DEJA_PRESENTE \
           || {{ printf '%s\\n' '{citee}' >> ~/.ssh/authorized_keys && echo AVASH_AJOUTEE; }}"
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

    /// `icacls` rejette `/grant:r` collé au compte : ce sont deux arguments.
    ///
    /// La forme fautive — `/grant:rutilisateur:F` — répondait « Invalid
    /// parameter » et faisait échouer toute création de clé SSH sous Windows.
    /// Ce test tourne sur n'importe quel système : il n'exécute pas `icacls`,
    /// il vérifie la ligne de commande qu'on lui destine.
    #[test]
    fn icacls_recoit_grant_et_compte_en_arguments_separes() {
        let a = icacls_args("C:\\Users\\x\\.ssh\\id_ed25519", "x", false);
        assert_eq!(
            a[0], "C:\\Users\\x\\.ssh\\id_ed25519",
            "le chemin vient en premier"
        );
        assert_eq!(
            a[1], "/inheritance:r",
            "l'héritage du dossier parent doit être coupé"
        );
        assert_eq!(a[2], "/grant:r", "« /grant:r » est un argument à lui seul");
        assert_eq!(
            a[3], "x:(F)",
            "compte et permission forment l'argument suivant"
        );
        assert!(
            !a.iter()
                .any(|arg| arg.starts_with("/grant:r") && arg.len() > "/grant:r".len()),
            "compte collé à /grant:r — icacls répond « Invalid parameter » : {a:?}"
        );
    }

    /// Un répertoire doit transmettre le droit à ce qu'il contiendra : après
    /// `/inheritance:r`, un fichier créé dans ~/.ssh n'hériterait de rien.
    #[test]
    fn un_repertoire_recoit_les_marqueurs_d_heritage() {
        let dossier = icacls_args("C:\\Users\\x\\.ssh", "x", true);
        assert_eq!(dossier[3], "x:(OI)(CI)F");
        let fichier = icacls_args("C:\\Users\\x\\.ssh\\cle", "x", false);
        assert_eq!(fichier[3], "x:(F)", "un fichier n'a rien à transmettre");
    }

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
    fn deploy_command_neutralise_une_injection_shell() {
        // Un saut de ligne ou un octet nul ne peut pas être cité sans casser la
        // commande : on les refuse.
        for mechant in [
            "ssh-ed25519 AAA\nrm -rf ~",
            "ssh-ed25519 AAA\r\nwhoami",
            "ssh-ed25519 AAA\0cache",
        ] {
            assert!(
                deploy_command(mechant).is_err(),
                "devrait etre refuse : {mechant:?}"
            );
        }
        // L'apostrophe, elle, est légitime dans un commentaire : on ne la refuse
        // plus, on l'échappe (« ' » → « '\'' »), si bien qu'une tentative de
        // sortie du quoting reste inoffensive et qu'aucune apostrophe brute ne
        // clôt le littéral. Trouvé par l'audit du 7 septembre 2026.
        let cmd = deploy_command("ssh-ed25519 AAA' ; rm -rf ~ ; echo '").unwrap();
        assert!(
            cmd.contains("'\\''"),
            "l'apostrophe doit être échappée : {cmd}"
        );
        assert!(
            !cmd.contains("AAA' ; rm"),
            "aucune apostrophe brute ne doit rester dans la commande : {cmd}"
        );
    }

    /// L'octet nul n'était pas dans le filtre (seuls `\n`, `\r`, `'` l'étaient) :
    /// il passait la garde et se faisait tronquer par le shell distant, posant
    /// dans `authorized_keys` une ligne différente de celle affichée. Trouvé par
    /// l'audit du 7 septembre 2026.
    #[test]
    fn deploy_command_refuse_l_octet_nul() {
        assert!(deploy_command("ssh-ed25519 AAAAC3Nz cle\0cachee").is_err());
    }

    /// Boucle complète : une clé générée avec un commentaire contenant une
    /// apostrophe (« clé d'Adrien », naturel en français ; ou un compte Windows
    /// « O'Brien » dans le commentaire par défaut) doit rester déployable par
    /// Avash. Avant le correctif, `deploy_command` refusait toute apostrophe et
    /// rendait non déployable une clé qu'Avash venait pourtant de créer.
    /// Trouvé par l'audit du 7 septembre 2026.
    #[test]
    fn une_cle_generee_avec_apostrophe_reste_deployable() {
        let _h = temp_home();
        let k = generate("id_apostrophe", "clé d'Adrien").unwrap();
        let ligne = k.public_line.expect("ligne publique attendue");
        assert!(
            ligne.contains("clé d'Adrien"),
            "le commentaire doit être posé tel quel : {ligne}"
        );
        let cmd = deploy_command(&ligne).expect("la clé générée doit être déployable");
        assert!(cmd.contains("authorized_keys"), "{cmd}");
        assert!(
            cmd.contains("'\\''"),
            "l'apostrophe doit être échappée : {cmd}"
        );
    }

    /// Audit du 12 septembre 2026 (C-panique-3) : un `.pub` au contenu
    /// accentué (une note, une clé mal collée) faisait paniquer l'aperçu de
    /// l'erreur, découpé à un indice d'octet au milieu d'un « é ».
    /// Audit du 12 septembre 2026 (C-couv-3.7) : le 0700 de `~/.ssh` n'était
    /// asserté nulle part ; `generate_produit_une_paire_utilisable` créait le
    /// répertoire sans lire ses droits. Test de couverture : il passe sur le
    /// code actuel et garde la promesse.
    #[cfg(unix)]
    #[test]
    fn le_repertoire_ssh_nait_en_0700() {
        use std::os::unix::fs::PermissionsExt as _;
        let garde = crate::testutil::temp_home();
        let ssh = garde.dir().join(".ssh");
        assert!(!ssh.exists(), "le décor : un profil sans ~/.ssh");
        generate("k", "c").unwrap();
        assert_eq!(
            std::fs::metadata(&ssh).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    /// Audit du 12 septembre 2026 (C-SIL-13) : une clé publique illisible
    /// n'est plus avalée en silence, le journal la nomme.
    #[test]
    fn une_cle_publique_illisible_est_signalee_au_journal() {
        let _garde = crate::testutil::temp_home();
        let dir = ssh_dir().unwrap();
        std::fs::write(dir.join("abimee"), b"prive").unwrap();
        std::fs::write(dir.join("abimee.pub"), [0xff, 0xfe, 0xfd]).unwrap();
        let (cles, journal) = crate::testutil::avertissements_pendant(|| list_keys().unwrap());
        let cle = cles.iter().find(|k| k.name == "abimee").expect("listée");
        assert!(cle.public_line.is_none());
        assert!(
            journal.iter().any(|l| l.contains("abimee.pub")),
            "le journal nomme la clé : {journal:?}"
        );
    }

    #[test]
    fn deploy_command_refuse_sans_paniquer_un_texte_accentue() {
        let e = deploy_command("aééééééééééééé").unwrap_err().to_string();
        assert!(e.contains("ne ressemble pas"), "{e}");
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

    /// La règle des noms réservés Windows se calcule sans dépendre du système
    /// hôte : on la vérifie donc partout, y compris en CI Linux. Trouvé par
    /// l'audit du 7 septembre 2026 : `generate` acceptait « nul » ou « com1 »,
    /// qui désignent un périphérique et non un fichier sous Windows.
    #[test]
    fn les_noms_de_peripheriques_windows_sont_reperes() {
        use crate::sftp::nom_reserve_windows;
        for reserve in [
            "con", "CON", "Nul", "aux", "prn", "com1", "LPT9", "com1.txt",
        ] {
            assert!(nom_reserve_windows(reserve), "réservé : {reserve:?}");
        }
        // Un vrai nom de fichier qui commence pareil n'est pas réservé.
        for bon in [
            "console",
            "com0",
            "com10",
            "lpt",
            "aux2",
            "travail",
            "id_ed25519",
        ] {
            assert!(!nom_reserve_windows(bon), "pas réservé : {bon:?}");
        }
    }

    /// Sous Windows, `dir.join("travail:pro")` désigne le flux de données
    /// alternatif « pro » du fichier « travail » : sans garde, `generate` créait
    /// un fichier « travail » de 0 octet portant la clé privée dans un flux
    /// caché, absent de `list_keys`. On refuse « : », les noms réservés et les
    /// points/espaces finaux AVANT toute écriture (audit du 7 septembre 2026).
    /// Test propre à Windows : ailleurs « : » est un caractère de nom valide.
    #[cfg(windows)]
    #[test]
    fn generate_refuse_les_pieges_ntfs_windows() {
        let _h = temp_home();
        // `generate` rogne déjà le nom : l'espace final n'y arrive pas, mais le
        // point final survit au `trim` et doit être refusé.
        for mauvais in ["travail:pro", "nul", "COM1", "com1.txt", "travail."] {
            assert!(
                generate(mauvais, "test").is_err(),
                "devrait etre refuse sous Windows : {mauvais:?}"
            );
            // Rien ne doit rester derrière : ni le fichier visé ni un porteur de
            // flux (« travail » pour « travail:pro »).
            let base = mauvais.split(':').next().unwrap_or(mauvais);
            assert!(
                !ssh_dir().unwrap().join(base).exists(),
                "un fichier « {base} » a été créé alors que le nom était refusé"
            );
        }
    }
    use crate::testutil::temp_home;

    /// Le front (web/cles.ts) décide de l'avertissement « OpenSSH exige 600 »
    /// sur la valeur JSON de `mode` : `"600"` = correct, toute autre chaîne =
    /// avertissement, et `null` = pas d'avertissement (droits gérés hors bits
    /// Unix). Ce test verrouille le contrat de sérialisation qui, hors Unix,
    /// renvoyait la sentinelle « - » prise à tort pour des droits incorrects
    /// (audit du 7 septembre 2026). Il tourne sur n'importe quel système.
    #[test]
    fn le_mode_absent_se_serialise_en_null_pas_en_sentinelle() {
        let entree = |mode: Option<&str>| KeyEntry {
            name: "id".into(),
            path: "/x/id".into(),
            public_line: None,
            mode: mode.map(str::to_owned),
        };
        let sans = serde_json::to_value(entree(None)).unwrap();
        assert!(
            sans["mode"].is_null(),
            "mode absent doit être null (et non « - »), sinon le front affiche « ⚠ » : {sans}"
        );
        let avec = serde_json::to_value(entree(Some("600"))).unwrap();
        assert_eq!(avec["mode"], "600", "un mode Unix reste sa chaîne octale");
    }

    #[test]
    fn generate_produit_une_paire_utilisable() {
        let _h = temp_home();
        let k = generate("id_test", "adrien@avash").unwrap();

        let private = PathBuf::from(&k.path);
        let public = private.with_extension("pub");
        assert!(private.is_file(), "cle privee absente");
        assert!(public.is_file(), "cle publique absente");

        // La clé privée ne doit être lisible que par son propriétaire :
        // OpenSSH refuse de s'en servir autrement. Ce qu'on peut en vérifier
        // dépend du système.
        #[cfg(unix)]
        assert_eq!(k.mode.as_deref(), Some("600"), "droits de la clé privée");
        #[cfg(windows)]
        {
            // Windows n'a pas de bits de permission : `mode` vaut donc `None`
            // (et non une sentinelle « - » que le front prenait pour des droits
            // incorrects, affichant « - ⚠ OpenSSH exige 600 » sur une clé qu'il
            // venait pourtant de restreindre par `icacls`). La restriction passe
            // par une ACL qu'on ne sait pas relire à bon compte ; ce qui est
            // vérifiable, et qui suffit : `generate` ci-dessus a réussi, or il
            // propage l'échec d'`icacls`.
            assert!(k.mode.is_none(), "aucun mode Unix hors Unix : {:?}", k.mode);
        }

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
        russh::keys::load_secret_key(&private, None).expect("cle privee illisible");
    }

    /// La clé privée doit être privée dès sa création, pas après coup : on la
    /// crée en 0600 plutôt que de la resserrer une fois écrite. Ce que le test
    /// peut vérifier, c'est le résultat et le refus d'écraser un fichier posé
    /// entre la vérification d'existence et l'écriture.
    #[cfg(unix)]
    #[test]
    fn ecrire_prive_cree_en_0600_et_refuse_d_ecraser() {
        use std::os::unix::fs::PermissionsExt;
        let _h = temp_home();
        let cible = crate::repertoire_personnel().unwrap().join("secret");
        ecrire_prive(&cible, b"contenu").unwrap();
        let mode = std::fs::metadata(&cible).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "droits à la création : {mode:o}");
        assert_eq!(std::fs::read(&cible).unwrap(), b"contenu");
        // Déjà là : on refuse, on ne tronque pas.
        assert!(ecrire_prive(&cible, b"autre").is_err());
        assert_eq!(
            std::fs::read(&cible).unwrap(),
            b"contenu",
            "l'original est intact"
        );
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
    #[cfg(unix)]
    #[test]
    fn deploy_command_installe_reellement_via_un_shell() {
        // Les autres tests verifient la commande generee ; celui-ci l'execute
        // pour de vrai dans un HOME temporaire, ce qu'aucun test unitaire ne
        // peut faire. Couvre l'idempotence et les droits poses.
        use std::os::unix::fs::PermissionsExt;
        let pub_line = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITEST avash@test";
        let cmd = deploy_command(pub_line).unwrap();
        let home = std::env::temp_dir().join(format!(
            "avash-deploy-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();

        let run = || {
            std::process::Command::new("sh")
                .arg("-c")
                .arg(&cmd)
                .env("HOME", &home)
                .output()
                .unwrap()
        };

        let un = run();
        assert!(interpret_deploy(&String::from_utf8_lossy(&un.stdout))
            .unwrap()
            .contains("installée"));
        // Relance : idempotent, pas de doublon.
        let deux = run();
        assert!(interpret_deploy(&String::from_utf8_lossy(&deux.stdout))
            .unwrap()
            .contains("déjà"));

        let ak = std::fs::read_to_string(home.join(".ssh/authorized_keys")).unwrap();
        assert_eq!(ak.lines().filter(|l| l.contains("TEST")).count(), 1);
        let m = std::fs::metadata(home.join(".ssh/authorized_keys"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(m, 0o600, "authorized_keys doit etre en 600");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// Exécution réelle d'une ligne dont le commentaire porte une apostrophe :
    /// l'échappement doit poser la ligne verbatim (apostrophe comprise) dans
    /// `authorized_keys`, et `grep -qxF` doit la retrouver à la relance sans
    /// doublon. Trouvé par l'audit du 7 septembre 2026 : Avash refusait ce cas.
    #[cfg(unix)]
    #[test]
    fn deploy_command_avec_apostrophe_s_installe_reellement() {
        let pub_line = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAPOS clé d'Adrien";
        let cmd = deploy_command(pub_line).unwrap();
        let home = std::env::temp_dir().join(format!(
            "avash-deploy-apos-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();

        let run = || {
            std::process::Command::new("sh")
                .arg("-c")
                .arg(&cmd)
                .env("HOME", &home)
                .output()
                .unwrap()
        };

        let un = run();
        assert!(interpret_deploy(&String::from_utf8_lossy(&un.stdout))
            .unwrap()
            .contains("installée"));
        // Relance : `grep -qxF` retrouve la ligne réassemblée, donc pas de doublon.
        let deux = run();
        assert!(interpret_deploy(&String::from_utf8_lossy(&deux.stdout))
            .unwrap()
            .contains("déjà"));

        let ak = std::fs::read_to_string(home.join(".ssh/authorized_keys")).unwrap();
        assert_eq!(
            ak.lines().filter(|l| l == &pub_line).count(),
            1,
            "la ligne doit être posée verbatim, apostrophe comprise : {ak:?}"
        );
        let _ = std::fs::remove_dir_all(&home);
    }
}
