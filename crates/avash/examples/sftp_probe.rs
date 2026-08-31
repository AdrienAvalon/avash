//! Éprouve le téléchargement SFTP contre un VRAI serveur, pas un serveur factice.
//!
//! Le téléchargement en bandes parallèles a été écrit et validé contre le
//! serveur de test du dépôt. Celui-ci honore décalage et longueur, mais il ne
//! reproduit ni les limites annoncées par OpenSSH (`limits@openssh.com`), ni sa
//! taille de paquet, ni son comportement avec plusieurs descripteurs ouverts sur
//! le même fichier. Cette sonde va chercher ces réponses-là.
//!
//! Usage :
//!   `sftp_probe <hôte> <port> <utilisateur> <clé-privée> <chemin-distant>`
//!
//! Elle télécharge deux fois : par bandes (le chemin de production) puis
//! séquentiellement (une lecture après l'autre), compare les octets et donne
//! les deux débits.
use std::time::Instant;
use tokio::io::AsyncReadExt as _;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [hote, port, user, cle, distant] = <[String; 5]>::try_from(args)
        .map_err(|_| anyhow::anyhow!("usage : sftp_probe <hôte> <port> <user> <clé> <chemin>"))?;
    let port: u16 = port.parse()?;

    let auth = avash::ssh::ClientAuth {
        user,
        key_path: Some(std::path::PathBuf::from(cle)),
        password: None,
    };
    let session = avash::ssh::AvashSession::connect(&hote, port, &auth).await?;
    let sftp = avash::sftp::SftpHandle::open(session).await?;

    let taille = sftp.sftp.metadata(&distant).await?.len();
    println!("fichier distant : {distant}  ({taille} octets)");

    // 1) Le chemin de production : bandes parallèles au-delà de deux blocs.
    let local = std::env::temp_dir().join("avash-sonde-bandes.bin");
    let depart = Instant::now();
    let recus = sftp.download_with(&distant, &local, |_, _| {}).await?;
    let d_bandes = depart.elapsed();
    let par_bandes = std::fs::read(&local)?;

    // 2) La même chose, une lecture après l'autre, pour comparer.
    let depart = Instant::now();
    let mut fichier = sftp.sftp.open(&distant).await?;
    let mut sequentiel = Vec::with_capacity(taille as usize);
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let lus = fichier.read(&mut buf).await?;
        if lus == 0 {
            break;
        }
        sequentiel.extend_from_slice(&buf[..lus]);
    }
    let d_seq = depart.elapsed();

    let mo = |o: u64, d: std::time::Duration| (o as f64 / 1_048_576.0) / d.as_secs_f64();
    println!(
        "  bandes parallèles : {:>7.1} Mo/s   ({:?})",
        mo(recus, d_bandes),
        d_bandes
    );
    println!(
        "  séquentiel        : {:>7.1} Mo/s   ({:?})",
        mo(taille, d_seq),
        d_seq
    );
    println!(
        "  rapport           : {:>7.1} ×",
        d_seq.as_secs_f64() / d_bandes.as_secs_f64()
    );

    anyhow::ensure!(
        recus == taille,
        "taille rendue {recus} ≠ taille annoncée {taille}"
    );
    anyhow::ensure!(
        par_bandes == sequentiel,
        "LES OCTETS DIFFÈRENT : le réassemblage des bandes est faux"
    );
    println!(
        "  ✓ identiques à l'octet près ({} octets)",
        par_bandes.len()
    );

    let _ = std::fs::remove_file(&local);
    sftp.close().await?;
    Ok(())
}
