//! Banc S6 : débits SFTP par les fonctions de production du cœur, celles que
//! le panneau SFTP appelle (`upload_reprise`, `download_reprise`,
//! `upload_dir_with`, `download_dir_with`), contre un vrai serveur.
//!
//! Usage :
//!   `banc_sftp <hôte> <port> <utilisateur> <clé> <dossier-distant> <répétitions>`
//!
//! Écrit une ligne JSON par mesure sur la sortie standard, que le banc
//! (e2e/banc/sftp.spec.js) verse dans son résultat :
//!   `{"cle":"S6/descente-100mo","unite":"Mo/s","sens":"haut","valeur":153.2}`
//!
//! Chaque aller-retour est vérifié à l'octet près : un débit sur des octets
//! faux ne vaudrait rien. Le dossier distant reçoit un sous-dossier de travail,
//! retiré à la fin.
//!
//! Conception : docs/superpowers/specs/2026-11-27-banc-et-audit-design.md.

// Programme de mesure lancé par le banc : un décor qui échoue doit s'arrêter
// net, d'où les `unwrap` et `expect`, que le lint `unwrap_used` signalerait.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::path::{Path, PathBuf};
use std::time::Instant;

const GROS: usize = 100 * 1024 * 1024;
const PETITS: usize = 1000;
const TAILLE_PETIT: usize = 4 * 1024;

/// Octets pseudo-aléatoires (xorshift) : incompressibles, sans dépendance de
/// plus, et reproductibles d'une exécution à l'autre.
fn octets(n: usize, graine: u64) -> Vec<u8> {
    let mut x = graine | 1;
    let mut v = Vec::with_capacity(n + 8);
    while v.len() < n {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        v.extend_from_slice(&x.to_le_bytes());
    }
    v.truncate(n);
    v
}

fn ligne(cle: &str, mo_par_s: f64) {
    println!(r#"{{"cle":"{cle}","unite":"Mo/s","sens":"haut","valeur":{mo_par_s:.3}}}"#);
}

fn debit(octets: u64, depart: Instant) -> f64 {
    (octets as f64 / 1_048_576.0) / depart.elapsed().as_secs_f64()
}

/// Les fichiers d'un dossier local, chemins relatifs triés, avec leur contenu.
fn contenu_du_dossier(d: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut v: Vec<(PathBuf, Vec<u8>)> = std::fs::read_dir(d)
        .unwrap()
        .map(|e| {
            let p = e.unwrap().path();
            (
                PathBuf::from(p.file_name().unwrap()),
                std::fs::read(&p).unwrap(),
            )
        })
        .collect();
    v.sort();
    v
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [hote, port, user, cle, distant, repetitions] = <[String; 6]>::try_from(args).map_err(|_| {
        anyhow::anyhow!("usage : banc_sftp <hôte> <port> <utilisateur> <clé> <dossier-distant> <répétitions>")
    })?;
    let port: u16 = port.parse()?;
    let repetitions: usize = repetitions.parse()?;

    let auth = avash::ssh::ClientAuth {
        user,
        key_path: Some(PathBuf::from(cle)),
        password: None,
    };
    let session = avash::ssh::AvashSession::connect(&hote, port, &auth).await?;
    let sftp = avash::sftp::SftpHandle::open(session).await?;

    // Le poste : un dossier de travail propre à cette exécution.
    let local = std::env::temp_dir().join(format!("avash-banc-sftp-{}", std::process::id()));
    std::fs::create_dir_all(&local)?;
    let travail = format!(
        "{}/avash-banc-sftp-{}",
        distant.trim_end_matches('/'),
        std::process::id()
    );
    sftp.mkdir(&travail).await?;

    let gros = octets(GROS, 0x5eed);
    let source = local.join("gros.bin");
    std::fs::write(&source, &gros)?;
    let petits = local.join("petits");
    std::fs::create_dir_all(&petits)?;
    for i in 0..PETITS {
        std::fs::write(
            petits.join(format!("f{i:04}.bin")),
            octets(TAILLE_PETIT, i as u64 + 1),
        )?;
    }
    let attendus = contenu_du_dossier(&petits);

    for r in 0..repetitions {
        let distant_gros = format!("{travail}/gros-{r}.bin");
        let depart = Instant::now();
        let n = sftp
            .upload_reprise(&source, &distant_gros, false, None, |_, _| {})
            .await?;
        ligne("S6/montee-100mo", debit(n, depart));

        let recu = local.join(format!("recu-{r}.bin"));
        let depart = Instant::now();
        let n = sftp
            .download_reprise(&distant_gros, &recu, None, |_, _| {})
            .await?;
        ligne("S6/descente-100mo", debit(n, depart));
        anyhow::ensure!(
            std::fs::read(&recu)? == gros,
            "aller-retour de 100 Mo : LES OCTETS DIFFÈRENT"
        );
        std::fs::remove_file(&recu)?;
        sftp.remove(&distant_gros, false).await?;

        let distant_petits = format!("{travail}/petits-{r}");
        let depart = Instant::now();
        let n = sftp
            .upload_dir_with(&petits, &distant_petits, None, |_| {})
            .await?;
        ligne("S6/montee-1000-petits", debit(n, depart));

        let recus = local.join(format!("petits-recus-{r}"));
        let depart = Instant::now();
        let n = sftp
            .download_dir_with(&distant_petits, &recus, None, |_| {})
            .await?;
        ligne("S6/descente-1000-petits", debit(n, depart));
        anyhow::ensure!(
            contenu_du_dossier(&recus) == attendus,
            "aller-retour de 1000 fichiers : CONTENU DIFFÉRENT"
        );
        std::fs::remove_dir_all(&recus)?;
        // `remove` ne vide pas un dossier (pas de rm -rf implicite, voir
        // SftpHandle::remove) : les fichiers d'abord.
        for i in 0..PETITS {
            sftp.remove(&format!("{distant_petits}/f{i:04}.bin"), false)
                .await?;
        }
        sftp.remove(&distant_petits, true).await?;
    }

    sftp.remove(&travail, true).await?;
    sftp.close().await?;
    std::fs::remove_dir_all(&local)?;
    Ok(())
}
