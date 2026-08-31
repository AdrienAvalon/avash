//! Avash — SFTP : liste, transferts avec progression, dossiers, renommage,
//! suppression, via russh-sftp sur une session existante.

use anyhow::{anyhow, Context, Result};
use russh_sftp::client::SftpSession;
use serde::Serialize;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Taille des blocs de transfert. 64 Kio : au-dessus, le gain est nul ;
/// en dessous, la progression est fine mais les allers-retours coutent.
const CHUNK: usize = 64 * 1024;

/// Nom du fichier temporaire d'un téléchargement : `rapport.pdf.part`.
fn chemin_partiel(local: &Path) -> PathBuf {
    let mut nom = local.as_os_str().to_owned();
    nom.push(".part");
    PathBuf::from(nom)
}

use super::ssh::AvashSession;

#[derive(Debug, Clone, Serialize)]
pub struct SftpEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<u64>,
}

/// Handle SFTP vivant — le front liste/téléverse via ces commandes.
/// Possède la session SSH mère (la garde ouverte).
pub struct SftpHandle {
    pub sftp: SftpSession,
    _session: AvashSession,
}

impl SftpHandle {
    /// Ouvre le sous-système SFTP sur une session (consommée, gardée vivante).
    pub async fn open(mut session: AvashSession) -> Result<Self> {
        let channel = session.open_sftp_channel().await?;
        let sftp = SftpSession::new(channel.into_stream())
            .await
            .context("Ouverture sous-système SFTP")?;
        Ok(Self {
            sftp,
            _session: session,
        })
    }

    /// Liste un répertoire distant.
    pub async fn list(&self, path: &str) -> Result<Vec<SftpEntry>> {
        let entries = self
            .sftp
            .read_dir(path)
            .await
            .with_context(|| format!("Lecture répertoire distant {path}"))?;
        Ok(entries
            .into_iter()
            .map(|e| {
                let m = e.metadata();
                SftpEntry {
                    name: e.file_name(),
                    is_dir: e.file_type().is_dir(),
                    size: m.len(),
                    modified: m.mtime.map(u64::from),
                }
            })
            .collect())
    }

    /// Télécharge un fichier distant → local.
    pub async fn download(&self, remote: &str, local: &Path) -> Result<u64> {
        self.download_with(remote, local, |_, _| {}).await
    }

    /// Téléverse un fichier local → distant.
    pub async fn upload(&self, local: &Path, remote: &str) -> Result<u64> {
        self.upload_with(local, remote, |_, _| {}).await
    }

    /// Telechargement avec progression : `progress(octets_faits, total)`.
    /// `total` vaut 0 si le serveur ne donne pas la taille.
    ///
    /// Le transfert passe par un fichier `.part` voisin, renommé une fois
    /// complet. `File::create` **tronque** la cible : un double-clic sur un
    /// fichier déjà présent dans `~/Téléchargements` l'écrasait d'emblée, et
    /// une coupure en cours de route laissait à sa place un fichier tronqué
    /// portant le bon nom — l'interface ne montrant qu'un avertissement fugace,
    /// on croyait avoir son fichier. Tant que le transfert n'a pas abouti, la
    /// cible n'est pas touchée.
    pub async fn download_with(
        &self,
        remote: &str,
        local: &Path,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<u64> {
        let total = self.sftp.metadata(remote).await.map_or(0, |m| m.len());
        let partiel = chemin_partiel(local);
        // Taille connue et fichier assez gros : on lit en bandes parallèles.
        if total > (2 * CHUNK) as u64 {
            let recu = self
                .telecharger_en_bandes(remote, &partiel, total, &mut progress)
                .await;
            return match recu {
                Ok(n) => {
                    tokio::fs::rename(&partiel, local)
                        .await
                        .with_context(|| format!("Renommage vers {}", local.display()))?;
                    Ok(n)
                }
                Err(e) => {
                    let _ = tokio::fs::remove_file(&partiel).await;
                    Err(e)
                }
            };
        }
        // Le descripteur distant n'est ouvert qu'ici : sur le chemin en bandes,
        // il l'était pour rien — un aller-retour d'ouverture et de fermeture de
        // plus, sur le chemin même qu'on cherche à raccourcir.
        let mut remote_file = self
            .sftp
            .open(remote)
            .await
            .with_context(|| format!("Ouverture distant {remote}"))?;
        let mut local_file = tokio::fs::File::create(&partiel)
            .await
            .with_context(|| format!("Création local {}", partiel.display()))?;
        let mut buf = vec![0u8; CHUNK];
        let mut done = 0u64;
        let issue = async {
            loop {
                let n = remote_file
                    .read(&mut buf)
                    .await
                    .context("Lecture distante")?;
                if n == 0 {
                    break;
                }
                local_file
                    .write_all(&buf[..n])
                    .await
                    .context("Écriture locale")?;
                done += n as u64;
                progress(done, total);
            }
            local_file.flush().await.context("Vidage local")?;
            anyhow::Ok(())
        }
        .await;
        drop(local_file);
        // Un transfert interrompu n'a pas à laisser de trace : ni fichier
        // tronqué à la place de la cible, ni `.part` orphelin.
        if let Err(e) = issue {
            let _ = tokio::fs::remove_file(&partiel).await;
            return Err(e);
        }
        tokio::fs::rename(&partiel, local)
            .await
            .with_context(|| format!("Renommage vers {}", local.display()))?;
        Ok(done)
    }

    /// Téléchargement en bandes parallèles.
    ///
    /// `File` de russh-sftp n'émet **qu'une** requête `SSH_FXP_READ` à la fois
    /// et attend sa réponse : le débit descendant plafonnait donc à
    /// `CHUNK / aller-retour`. La montée, elle, est déjà pipelinée par la
    /// bibliothèque (huit écritures en vol).
    ///
    /// Mesuré contre un vrai `internal-sftp` d'OpenSSH, sur 8 Mio
    /// (`examples/sftp_probe.rs`, latence introduite par un relais qui modélise
    /// un délai de propagation) :
    ///
    /// | aller-retour | séquentiel | en bandes | gain  |
    /// |--------------|------------|-----------|-------|
    /// | ~10 ms       | 5,6 Mo/s   | 34,9 Mo/s | 6,3 × |
    /// | ~30 ms       | 2,0 Mo/s   | 13,5 Mo/s | 6,8 × |
    /// | ~60 ms       | 1,0 Mo/s   |  7,1 Mo/s | 7,1 × |
    ///
    /// En réseau local le gain retombe à ~2 ×, la latence n'y étant plus le
    /// facteur limitant. Les octets sont identiques dans tous les cas.
    ///
    /// Faute d'API publique pour empiler des lectures sur un même descripteur,
    /// on ouvre plusieurs descripteurs sur le même chemin — c'est licite, et
    /// bien en deçà de la limite d'un serveur OpenSSH — et chacun lit sa bande,
    /// à son propre décalage. Les écritures locales ne se recouvrent pas :
    /// chaque bande a son propre descripteur local et son propre intervalle,
    /// donc aucun verrou.
    async fn telecharger_en_bandes(
        &self,
        remote: &str,
        partiel: &Path,
        total: u64,
        progress: &mut impl FnMut(u64, u64),
    ) -> Result<u64> {
        /// Au-delà, on encombre le serveur sans gagner : le lien sature avant.
        const BANDES_MAX: u64 = 8;
        let bandes = (total / (CHUNK as u64)).clamp(1, BANDES_MAX);
        let taille_bande = total.div_ceil(bandes);

        // Le fichier est créé (et vidé) une fois, avant que les bandes n'y
        // écrivent chacune à son décalage.
        tokio::fs::File::create(partiel)
            .await
            .with_context(|| format!("Création local {}", partiel.display()))?;

        // La progression appartient à l'appelant et n'est pas partageable : les
        // bandes annoncent leur avancement, cette tâche-ci le rapporte.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<u64>();
        let mut travaux = Vec::new();
        for i in 0..bandes {
            let debut = i * taille_bande;
            let fin = ((i + 1) * taille_bande).min(total);
            if debut >= fin {
                break;
            }
            travaux.push(self.une_bande(remote, partiel, debut, fin, tx.clone()));
        }
        drop(tx); // sans quoi la boucle d'annonce n'aurait pas de fin

        let annonces = async {
            let mut fait = 0u64;
            while let Some(n) = rx.recv().await {
                fait += n;
                progress(fait, total);
            }
            fait
        };
        let (issues, fait) = tokio::join!(futures::future::join_all(travaux), annonces);
        for issue in issues {
            issue?;
        }
        // Une bande qui tombe sur une fin de fichier prématurée sort de sa
        // boucle sans erreur : le fichier distant a rétréci en cours de route —
        // un journal en rotation, par exemple, ce que huit lectures concurrentes
        // rendent bien plus probable qu'une lecture séquentielle. Sans ce
        // contrôle, le `.part` — écrit à des décalages disjoints — contenait des
        // **zéros au milieu** et était promu sur la cible, transfert annoncé
        // réussi. Le chemin séquentiel, lui, ne pouvait que tronquer.
        anyhow::ensure!(
            fait == total,
            "Transfert incomplet : {fait} octets reçus sur {total} annoncés \
             (le fichier distant a changé pendant le transfert)."
        );
        Ok(fait)
    }

    /// Lit `[debut, fin)` du fichier distant et l'écrit au même décalage local.
    async fn une_bande(
        &self,
        remote: &str,
        partiel: &Path,
        debut: u64,
        fin: u64,
        tx: tokio::sync::mpsc::UnboundedSender<u64>,
    ) -> Result<()> {
        use tokio::io::AsyncSeekExt as _;
        let mut distant = self
            .sftp
            .open(remote)
            .await
            .with_context(|| format!("Ouverture distant {remote}"))?;
        distant
            .seek(std::io::SeekFrom::Start(debut))
            .await
            .context("Positionnement distant")?;
        let mut local = tokio::fs::OpenOptions::new()
            .write(true)
            .open(partiel)
            .await
            .with_context(|| format!("Ouverture local {}", partiel.display()))?;
        local
            .seek(std::io::SeekFrom::Start(debut))
            .await
            .context("Positionnement local")?;

        let mut buf = vec![0u8; CHUNK];
        let mut reste = fin - debut;
        while reste > 0 {
            let vise = usize::try_from(reste.min(CHUNK as u64)).unwrap_or(CHUNK);
            let n = distant
                .read(&mut buf[..vise])
                .await
                .context("Lecture distante")?;
            if n == 0 {
                break; // le fichier a rétréci depuis la lecture de sa taille
            }
            local
                .write_all(&buf[..n])
                .await
                .context("Écriture locale")?;
            reste -= n as u64;
            let _ = tx.send(n as u64);
        }
        local.flush().await.context("Vidage local")?;
        Ok(())
    }

    /// Televersement avec progression.
    pub async fn upload_with(
        &self,
        local: &Path,
        remote: &str,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<u64> {
        let mut local_file = tokio::fs::File::open(local)
            .await
            .with_context(|| format!("Ouverture local {}", local.display()))?;
        let total = local_file.metadata().await.map_or(0, |m| m.len());
        let mut remote_file = self
            .sftp
            .create(remote)
            .await
            .with_context(|| format!("Création distant {remote}"))?;
        let mut buf = vec![0u8; CHUNK];
        let mut done = 0u64;
        loop {
            let n = local_file.read(&mut buf).await.context("Lecture locale")?;
            if n == 0 {
                break;
            }
            remote_file
                .write_all(&buf[..n])
                .await
                .context("Écriture distante")?;
            done += n as u64;
            progress(done, total);
        }
        remote_file.shutdown().await.context("Fermeture distante")?;
        Ok(done)
    }

    /// Chemin absolu correspondant a `path` (`.` → home au login).
    /// En cas d'echec, rend `path` inchange : mieux vaut tenter la liste que
    /// bloquer l'ouverture du panneau.
    pub async fn realpath(&self, path: &str) -> String {
        self.sftp
            .canonicalize(path)
            .await
            .unwrap_or_else(|_| path.to_string())
    }

    pub async fn mkdir(&self, path: &str) -> Result<()> {
        self.sftp
            .create_dir(path)
            .await
            .with_context(|| format!("Création du dossier {path}"))
    }

    /// Supprime un fichier, ou un dossier **vide** (`is_dir`). Un dossier
    /// plein est refuse par le serveur : on ne fait pas de `rm -rf` implicite.
    pub async fn remove(&self, path: &str, is_dir: bool) -> Result<()> {
        if is_dir {
            self.sftp
                .remove_dir(path)
                .await
                .with_context(|| format!("Suppression du dossier {path} (doit être vide)"))
        } else {
            self.sftp
                .remove_file(path)
                .await
                .with_context(|| format!("Suppression de {path}"))
        }
    }

    pub async fn rename(&self, from: &str, to: &str) -> Result<()> {
        self.sftp
            .rename(from, to)
            .await
            .with_context(|| format!("Renommage {from} → {to}"))
    }

    pub async fn close(self) -> Result<()> {
        self.sftp.close().await.map_err(|e| anyhow!(e))
    }
}

/// Résolveur de chemin de téléchargement local par défaut.
pub fn default_local_dir() -> PathBuf {
    dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}
