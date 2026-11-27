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

/// Écritures SFTP en vol par fichier (voir `SftpHandle::sous_systeme`).
const ECRITURES_EN_VOL: usize = 16;

/// Ce qu'une requête de lecture peut demander : russh-sftp 4 demande la taille
/// que le serveur annonce (`limits@openssh.com`, 255 Kio chez OpenSSH), bornée
/// par sa taille de paquet (256 Kio), quel que soit le tampon de l'appelant.
const LECTURE_MAX: usize = 256 * 1024;

/// Octets de lecture en vol sur une session SFTP, toutes bandes et tous
/// fichiers confondus.
///
/// Audit du 27 novembre 2026 (PR-4, FI-4) : russh-sftp exige la réponse de
/// chaque requête dans un délai compté dès son écriture, et le serveur répond
/// dans l'ordre, au débit du lien. Huit bandes demandaient 2 Mio d'un coup, un
/// dossier de gros fichiers quatre fois plus : sur un lien lent (4G, Wi-Fi
/// d'hôtel, VPN chargé), les dernières réponses arrivaient après le délai, et
/// le dossier entier échouait en « Timeout » alors que le lien marchait. Le
/// parallélisme se règle désormais en octets : chaque lecture prend sa part de
/// ce budget avant de partir et la rend à sa réponse, et ce qui attend chez le
/// serveur ne dépasse jamais 2 Mio, soit une fenêtre de canal SSH. Un seul gros
/// fichier garde ses huit bandes de front ; plusieurs se les partagent, au lieu
/// de s'additionner. Les écritures, elles, n'en ont pas besoin : elles ne
/// partent qu'à mesure que la fenêtre SSH du serveur s'ouvre, et le délai de
/// russh-sftp ne court qu'une fois la requête écrite.
const BUDGET_LECTURES: usize = 2 * 1024 * 1024;

/// Délai de réponse d'une requête SFTP, en secondes.
///
/// Audit du 27 novembre 2026 (PR-4) : les 10 s par défaut de russh-sftp
/// faisaient de ce délai un débit plancher (2 Mio en vol en 10 s, 1,7 Mbit/s).
/// À 30 s, avec le budget ci-dessus, le plancher tombe à 0,6 Mbit/s. Un lien
/// mort reste vu par le keepalive SSH (30 s, trois essais) et, en plein envoi,
/// par le délai d'acquittement TCP de la session.
const DELAI_REQUETE_S: u64 = 30;

/// Nom du fichier temporaire d'un téléchargement : `rapport.pdf.part`.
fn chemin_partiel(local: &Path) -> PathBuf {
    let mut nom = local.as_os_str().to_owned();
    nom.push(".part");
    PathBuf::from(nom)
}

/// Promeut le `.part` complet sur sa cible définitive ; si le renommage échoue,
/// garde le `.part` et le dit dans l'erreur.
///
/// Trouvé par l'audit du 7 septembre 2026 : un téléchargement se termine par ce
/// renommage pour ne toucher la cible existante qu'à la fin. Sous Unix, `rename`
/// remplace toujours ; sous Windows, le remplacement échoue par violation de
/// partage si la cible est ouverte ailleurs sans `FILE_SHARE_DELETE` (Acrobat,
/// Excel, un aperçu de l'Explorateur) ou porte l'attribut lecture seule. Le
/// transfert est pourtant COMPLET, tout entier dans le `.part` — que le code
/// laissait alors muet, en rendant un sec « Renommage vers X », transfert à
/// 100 % annoncé en échec. On pointe désormais le `.part`, qu'on ne supprime
/// pas : aucune perte de données, et l'utilisateur sait où est son fichier. Le
/// message enrichi ne borne pas son texte à `cfg(windows)` : il reste juste
/// partout (tout échec de renommage laisse bien le `.part`) et ne change aucun
/// comportement (ni suppression, ni nouvelle tentative), ce qui le rend
/// vérifiable sous Unix en forçant l'échec (cible qui est un répertoire).
async fn promouvoir_partiel(partiel: &Path, local: &Path) -> Result<()> {
    tokio::fs::rename(partiel, local).await.with_context(|| {
        format!(
            "Renommage vers {} (le fichier complet est dans {})",
            local.display(),
            partiel.display()
        )
    })
}

use super::ssh::AvashSession;

#[derive(Debug, Clone, Serialize)]
pub struct SftpEntry {
    pub name: String,
    pub is_dir: bool,
    /// Fichier régulier ? Distinct de `!is_dir` : un lien symbolique et un
    /// fichier spécial ne sont ni l'un ni l'autre. `parcourir` s'en sert pour
    /// écarter ces entrées d'un transfert récursif (voir l'audit du 7 septembre
    /// 2026, plus bas).
    pub is_file: bool,
    pub size: u64,
    pub modified: Option<u64>,
}

/// Handle SFTP vivant — le front liste/téléverse via ces commandes.
///
/// Possède la session SSH mère quand il l'a reçue (`open`) ; avec `open_on`,
/// elle reste à l'appelant, qui doit la garder vivante.
pub struct SftpHandle {
    pub sftp: SftpSession,
    _session: Option<AvashSession>,
    /// Budget d'octets de lecture en vol de cette session (`BUDGET_LECTURES`).
    lectures: tokio::sync::Semaphore,
}

impl SftpHandle {
    /// Ouvre le sous-système SFTP sur une session (consommée, gardée vivante).
    pub async fn open(mut session: AvashSession) -> Result<Self> {
        let sftp = Self::sous_systeme(&mut session).await?;
        Ok(Self {
            sftp,
            _session: Some(session),
            lectures: tokio::sync::Semaphore::new(BUDGET_LECTURES),
        })
    }

    /// Ouvre le sous-système SFTP sur un canal supplémentaire d'une session
    /// qui reste à l'appelant — celle du terminal, typiquement. Pas de seconde
    /// connexion ni de seconde authentification : le protocole multiplexe les
    /// canaux, et le serveur ne voit qu'une session. Le canal vit tant que la
    /// session vit.
    pub async fn open_on(session: &mut AvashSession) -> Result<Self> {
        let sftp = Self::sous_systeme(session).await?;
        Ok(Self {
            sftp,
            _session: None,
            lectures: tokio::sync::Semaphore::new(BUDGET_LECTURES),
        })
    }

    async fn sous_systeme(session: &mut AvashSession) -> Result<SftpSession> {
        let channel = session.open_sftp_channel().await?;
        // Une lecture à la fois par fichier, seize écritures en vol.
        //
        // Lectures : russh-sftp 4 lit en avance (seize requêtes en vol par
        // fichier, par défaut) ; or le parallélisme est déjà fait ici, par
        // bandes, et la reprise repose sur ce qu'on sait reçu : la carte des
        // bandes. Avec la lecture anticipée, une bande débordait sur la
        // suivante (octets relus dans une bande déjà faite) et une annulation
        // laissait des requêtes en vol qui faisaient expirer la suivante
        // (trois tests d'intégration rouges à la montée du 21 novembre 2026,
        // quatre rejoués le 27 novembre avec les seules lectures anticipées).
        //
        // Écritures : le 21 novembre, elles avaient été bridées à une aussi,
        // par prudence. Audit du 27 novembre 2026 (PR-2) : c'était la cause de
        // la montée à 1,5 Mo/s derrière un VPN à 40 ms (64 Kio par
        // aller-retour), contre 10,9 Mo/s mesurés le 4 septembre avec les huit
        // écritures de russh-sftp 2. Aucun des défauts du 21 novembre ne tient
        // aux écritures : rejouée avec seize écritures en vol, la suite entière
        // reste verte, et la reprise d'un envoi n'inscrit sa carte qu'après un
        // `flush`, qui attend tous les accusés en attente. Une erreur en plein
        // envoi perd les écritures en vol, que la carte ne promettait pas.
        // Seize (le défaut de russh-sftp 4) plutôt que huit : 4 Mio montés à
        // 40 ms d'aller-retour en 0,44 s contre 0,64 s (0,9 Mo/s → 9,5 Mo/s
        // depuis une écriture à la fois, 3,1 s), test
        // `un_envoi_sous_latence_garde_plusieurs_ecritures_en_vol`.
        //
        // Taille des écritures : russh-sftp 4 préfère 32 Kio par paquet là où
        // la 2 prenait la limite annoncée par le serveur (ou la taille maximale
        // du paquet). On garde l'ancienne règle en alignant la préférence sur
        // la taille maximale du paquet, le serveur restant maître par
        // `limits@openssh.com` ; les blocs d'Avash font 64 Kio (`CHUNK`).
        let defaut = russh_sftp::client::Config::default();
        let reglages = russh_sftp::client::Config {
            max_concurrent_reads: 1,
            max_concurrent_writes: ECRITURES_EN_VOL,
            max_write_packet_len: defaut.max_packet_len,
            request_timeout_secs: DELAI_REQUETE_S,
            ..defaut
        };
        SftpSession::new_with_config(channel.into_stream(), reglages)
            .await
            .context("Ouverture sous-système SFTP")
    }

    /// Prend sur le budget de la session de quoi lire `octets` en une requête
    /// (voir `BUDGET_LECTURES`) ; la part revient au budget quand la garde
    /// tombe, une fois la lecture rendue. Un appel à `read` sur un `File` de
    /// russh-sftp émet au plus une requête : la garde la couvre donc entière.
    /// Le sémaphore n'est jamais fermé ; s'il l'était, on lirait sans budget
    /// plutôt que d'échouer.
    async fn avant_lecture(&self, octets: u64) -> Option<tokio::sync::SemaphorePermit<'_>> {
        let part = usize::try_from(octets).map_or(LECTURE_MAX, |o| o.clamp(1, LECTURE_MAX));
        self.lectures
            .acquire_many(u32::try_from(part).unwrap_or(u32::MAX))
            .await
            .ok()
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
                    is_file: e.file_type().is_file(),
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
        progress: impl FnMut(u64, u64),
    ) -> Result<u64> {
        let total = self.sftp.metadata(remote).await.map_or(0, |m| m.len());
        self.telecharger_connu(remote, local, total, None, progress)
            .await
    }

    /// [`download_with`](Self::download_with) sur un fichier dont la taille
    /// est déjà lue (0 : inconnue), annulable entre deux blocs.
    ///
    /// Audit du 12 septembre 2026 (C-perf-3 a) : `download_reprise` lisait les
    /// attributs puis renvoyait les petits fichiers vers `download_with`, qui
    /// les relisait : un aller-retour de plus par fichier, le plus coûteux des
    /// cinq sur un dossier de petits fichiers à distance.
    async fn telecharger_connu(
        &self,
        remote: &str,
        local: &Path,
        total: u64,
        annulation: Option<&Annulation>,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<u64> {
        let partiel = chemin_partiel(local);
        // Taille connue et fichier assez gros : on lit en bandes parallèles.
        if total > (2 * CHUNK) as u64 {
            let recu = self
                .telecharger_en_bandes(remote, &partiel, total, &mut progress)
                .await;
            return match recu {
                Ok(n) => {
                    promouvoir_partiel(&partiel, local).await?;
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
        // Une taille connue borne ce que le serveur rendra ; inconnue, une
        // requête entière.
        let part = if total > 0 { total } else { LECTURE_MAX as u64 };
        let issue = async {
            loop {
                verifier(annulation)?;
                let garde = self.avant_lecture(part).await;
                let n = remote_file
                    .read(&mut buf)
                    .await
                    .context("Lecture distante")?;
                drop(garde);
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
        // Trouvé par l'audit du 7 septembre 2026 : le chemin en bandes contrôle
        // `fait == total` (fichier distant rétréci en cours de route), mais le
        // chemin séquentiel renommait le `.part` sans rien vérifier. Un journal
        // distant rotationné/tronqué pendant la lecture atteint un EOF propre
        // plus tôt : `done < total`, aucune erreur, et le préfixe était promu
        // sur la cible, transfert annoncé réussi. On ne contrôle qu'une taille
        // connue (`total > 0` : `download_reprise` renvoie aussi ici quand le
        // serveur ne donne pas de taille) et on n'exige que `done >= total` :
        // un fichier qui GROSSIT pendant la lecture (journal en cours
        // d'écriture) reste un transfert complet et correct — d'où `<` et non
        // `!=`. Le `drop(local_file)` a déjà eu lieu : le `.part` peut être
        // supprimé même sous Windows.
        if total > 0 && done < total {
            let _ = tokio::fs::remove_file(&partiel).await;
            anyhow::bail!(
                "Transfert incomplet : {done} octets reçus sur {total} annoncés \
                 (le fichier distant a changé pendant le transfert)."
            );
        }
        promouvoir_partiel(&partiel, local).await?;
        Ok(done)
    }

    /// Téléchargement en bandes parallèles.
    ///
    /// `File` de russh-sftp n'émet **qu'une** requête `SSH_FXP_READ` à la fois
    /// et attend sa réponse : le débit descendant plafonnait donc à
    /// `CHUNK / aller-retour`. La montée, elle, est pipelinée par la
    /// bibliothèque (`ECRITURES_EN_VOL` écritures en vol par fichier).
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
            let garde = self.avant_lecture(LECTURE_MAX as u64).await;
            let n = distant
                .read(&mut buf[..vise])
                .await
                .context("Lecture distante")?;
            drop(garde);
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
        // Trouvé par l'audit du 7 septembre 2026 : jumeau du défaut de
        // `download_with`. `upload_reprise` renvoie ici tout fichier <= 128 Kio,
        // qui ne vérifiait pas `done == total`. Un fichier local tronqué pendant
        // l'envoi (rotation, écrasement par un autre processus) atteint un EOF
        // plus tôt : `done < total`, aucune erreur, et le distant recevait un
        // fichier court annoncé réussi — le chemin long, lui, contrôle
        // (`ensure! fait == total`). Même règle : taille connue (`total > 0`) et
        // `done >= total` toléré (un fichier local qui grossit reste complet).
        if total > 0 && done < total {
            anyhow::bail!(
                "Envoi incomplet : {done} octets envoyés sur {total} (le fichier local a changé pendant l'envoi)."
            );
        }
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

// ---------- Transferts : dossiers, reprise, bandes montantes, annulation, relais ----------

/// Annulation coopérative d'un transfert : l'interface la lève, les boucles la
/// lisent entre deux blocs. Ce qui est déjà écrit reste en place, avec sa carte
/// de reprise.
pub type Annulation = std::sync::Arc<std::sync::atomic::AtomicBool>;

/// Où en est un transfert qui peut porter plusieurs fichiers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Avancement {
    /// Le fichier en cours (chemin relatif à la racine du transfert).
    pub fichier: String,
    /// Octets faits et total, tous fichiers confondus.
    pub fait: u64,
    pub total: u64,
    /// Fichiers terminés et nombre de fichiers (les dossiers ne comptent pas).
    pub termines: usize,
    pub nombre: usize,
}

/// Une entrée d'un parcours distant, relative à la racine parcourue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntreeDistante {
    pub chemin: String,
    pub dossier: bool,
    pub taille: u64,
    /// Date de modification lue dans le `READDIR` : avec la taille, de quoi
    /// valider une carte de reprise sans relire les attributs (PR-3).
    pub mtime: Option<u64>,
}

/// Nombre d'entrées qu'un parcours accepte : un serveur peut annoncer une
/// arborescence sans fin (lien qui boucle), on s'arrête et on le dit.
const ENTREES_MAX: usize = 100_000;

/// Message de l'erreur d'annulation, que l'interface reconnaît.
pub const ANNULE: &str = "Transfert annulé.";

/// Fichiers d'un dossier transférés à la fois.
///
/// Audit du 12 septembre 2026 (C-perf-3 b) : un dossier passait un fichier
/// après l'autre, et un petit fichier coûte cinq allers-retours strictement
/// séquentiels (une dizaine en relais) : mille petits fichiers à 30 ms
/// d'aller-retour, 2 min 30 en téléchargement quel que soit le débit du lien.
/// Quatre en vol : sous les huit bandes d'un gros fichier, bien sous la limite
/// de descripteurs de tout serveur.
///
/// Audit du 27 novembre 2026 (PR-3) : quatre petits fichiers à la fois, à
/// quatre allers-retours chacun, plafonnaient à 25 fichiers par seconde à
/// 40 ms (0,1 Mo/s en fichiers de 4 Kio). Seize petits fichiers sont en vol ;
/// un gros fichier (en bandes) compte pour `PLACES_GROS_FICHIER`, si bien que
/// quatre gros au plus restent de front, comme avant : leurs lectures se
/// partagent de toute façon le budget de la session (`BUDGET_LECTURES`), et
/// seize fois huit bandes ouvriraient cent vingt-huit descripteurs chez le
/// serveur pour rien.
const FICHIERS_EN_VOL: usize = 16;

/// Places qu'un gros fichier (plus de deux blocs, donc en bandes ou en
/// reprise) prend parmi les `FICHIERS_EN_VOL` d'un dossier.
const PLACES_GROS_FICHIER: u32 = 4;

/// Places qu'un fichier de `taille` octets prend dans un dossier.
fn places_pour(taille: u64) -> u32 {
    if taille > (2 * CHUNK) as u64 {
        PLACES_GROS_FICHIER
    } else {
        1
    }
}

/// Ce qu'un fichier en vol rapporte à la boucle qui tient la progression.
enum Nouvelle {
    /// Octets de plus pour ce fichier, depuis sa dernière annonce.
    Octets(String, u64),
    /// Fichier terminé ; porte le reliquat entre les annonces et le total rendu.
    Fini(String, u64),
}

/// Traduit l'avancement cumulé d'un fichier (ce que rapportent les transferts
/// d'un fichier) en écarts pour la boucle commune du dossier.
struct Rapporteur {
    nom: String,
    vu: std::sync::atomic::AtomicU64,
    tx: tokio::sync::mpsc::UnboundedSender<Nouvelle>,
}

impl Rapporteur {
    fn nouveau(nom: String, tx: tokio::sync::mpsc::UnboundedSender<Nouvelle>) -> Self {
        Self {
            nom,
            vu: std::sync::atomic::AtomicU64::new(0),
            tx,
        }
    }

    fn avance(&self, cumul: u64) {
        let avant = self
            .vu
            .fetch_max(cumul, std::sync::atomic::Ordering::Relaxed);
        if cumul > avant {
            let _ = self
                .tx
                .send(Nouvelle::Octets(self.nom.clone(), cumul - avant));
        }
    }

    fn fini(&self, n: u64) {
        let vu = self.vu.load(std::sync::atomic::Ordering::Relaxed);
        let _ = self
            .tx
            .send(Nouvelle::Fini(self.nom.clone(), n.saturating_sub(vu)));
    }
}

/// L'erreur est-elle (au fond) une annulation ?
fn est_annule(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.to_string() == ANNULE)
}

/// Ce qu'un fichier en vol rend en erreur : l'annulation telle quelle (que
/// l'interface reconnaît), toute autre erreur avec le nom du fichier.
fn avec_le_fichier(e: anyhow::Error, nom: &str) -> anyhow::Error {
    if est_annule(&e) {
        anyhow!(ANNULE)
    } else {
        e.context(nom.to_owned())
    }
}

/// Transfère les `fichiers` d'un dossier, `FICHIERS_EN_VOL` à la fois, et tient
/// la progression commune.
///
/// Chaque fichier reçoit un drapeau d'arrêt levé à la première erreur, ou dès
/// que l'utilisateur annule : ceux qui sont en vol s'arrêtent à leur prochain
/// bloc et nettoient derrière eux (partiel retiré, ou gardé avec sa carte de
/// reprise pour un gros fichier), ceux qui n'étaient pas partis ne partent pas.
/// On attend qu'ils aient tous rendu la main avant de répondre, et l'erreur
/// rendue est la vraie cause, pas l'« annulé » des fichiers qu'elle a arrêtés.
async fn en_vol<T, F, Fut>(
    fichiers: Vec<(T, u32)>,
    total: u64,
    annulation: Option<&Annulation>,
    mut progress: impl FnMut(Avancement),
    un_fichier: F,
) -> Result<u64>
where
    F: Fn(T, Annulation, tokio::sync::mpsc::UnboundedSender<Nouvelle>) -> Fut,
    Fut: std::future::Future<Output = Result<u64>>,
{
    use futures::StreamExt as _;
    use std::sync::atomic::Ordering::Relaxed;
    verifier(annulation)?;
    let nombre = fichiers.len();
    let arret: Annulation = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let fini = std::sync::atomic::AtomicBool::new(false);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Nouvelle>();
    let arret_des_fichiers = arret.clone();
    // Chaque fichier porte ses places (voir `places_pour`) : seize petits ou
    // quatre gros à la fois. Le sémaphore est équitable : un gros fichier qui
    // attend ses places ne se fait pas doubler sans fin par les petits.
    let places = tokio::sync::Semaphore::new(FICHIERS_EN_VOL);
    let places = &places;
    let un_fichier = &un_fichier;
    let travaux = futures::stream::iter(fichiers)
        .map(move |(f, poids)| {
            let arret = arret_des_fichiers.clone();
            let tx = tx.clone();
            async move {
                let _place = places.acquire_many(poids).await.ok();
                un_fichier(f, arret, tx).await
            }
        })
        .buffer_unordered(FICHIERS_EN_VOL);
    let consommer = async {
        // Le flux (et le dernier émetteur d'annonces qu'il tient) meurt à la
        // fin de ce bloc : la boucle d'annonces peut alors s'arrêter.
        let mut travaux = std::pin::pin!(travaux);
        let mut cause: Option<anyhow::Error> = None;
        while let Some(issue) = travaux.next().await {
            if let Err(e) = issue {
                arret.store(true, Relaxed);
                let remplacer = match &cause {
                    None => true,
                    Some(c) => est_annule(c) && !est_annule(&e),
                };
                if remplacer {
                    cause = Some(e);
                }
            }
        }
        fini.store(true, Relaxed);
        cause
    };
    let annonces = async {
        let (mut fait, mut termines) = (0u64, 0usize);
        while let Some(nouvelle) = rx.recv().await {
            let fichier = match nouvelle {
                Nouvelle::Octets(nom, n) => {
                    fait += n;
                    nom
                }
                Nouvelle::Fini(nom, reliquat) => {
                    fait += reliquat;
                    termines += 1;
                    nom
                }
            };
            progress(Avancement {
                fichier,
                fait,
                total,
                termines,
                nombre,
            });
        }
        fait
    };
    // L'annulation de l'utilisateur, recopiée sur le drapeau des fichiers.
    let miroir = async {
        let Some(a) = annulation else { return };
        while !fini.load(Relaxed) {
            if a.load(Relaxed) {
                arret.store(true, Relaxed);
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    };
    let (cause, fait, ()) = tokio::join!(consommer, annonces, miroir);
    match cause {
        Some(e) => Err(e),
        None => Ok(fait),
    }
}

fn verifier(annulation: Option<&Annulation>) -> Result<()> {
    if annulation.is_some_and(|a| a.load(std::sync::atomic::Ordering::Relaxed)) {
        anyhow::bail!(ANNULE);
    }
    Ok(())
}

fn chemin_reprise(partiel: &Path) -> PathBuf {
    let mut nom = partiel.as_os_str().to_owned();
    nom.push(".reprise");
    PathBuf::from(nom)
}

/// Carte de reprise d'un transfert en bandes : ce que le fichier fait, quand,
/// et les bandes déjà complètes. Écrite à côté du `.part` ; effacée avec lui.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
struct Reprise {
    taille: u64,
    mtime: Option<u64>,
    /// Bandes complètes, `[debut, fin)`.
    faites: Vec<(u64, u64)>,
    /// Chemin distant que cette carte vise (envoi seulement ; `None` en
    /// téléchargement, où la carte est repérée par le `.part` local).
    /// `default` de serde : une vieille carte sans ce champ se relit encore, et
    /// vaut alors pour aucune cible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cible: Option<String>,
}

impl Reprise {
    fn lire(chemin: &Path) -> Option<Self> {
        serde_json::from_slice(&std::fs::read(chemin).ok()?).ok()
    }
    /// Écrit la carte. Un échec n'arrête pas le transfert (il n'y perd rien
    /// grâce à `vaut_pour` et à `part_assez_long`), mais n'est plus avalé :
    /// audit du 12 septembre 2026 (C-SIL-13), la relance repartait de zéro sans
    /// que rien ne dise pourquoi. Le journal le dit.
    fn ecrire(&self, chemin: &Path) {
        let ecrit = serde_json::to_vec(self)
            .map_err(anyhow::Error::from)
            .and_then(|json| crate::ecrire_atomiquement(chemin, &json));
        if let Err(e) = ecrit {
            tracing::warn!(
                "Carte de reprise {} non écrite : {e:#}. Une relance repartira de zéro \
                 pour ce fichier.",
                chemin.display()
            );
        }
    }
    /// Cette carte vaut-elle pour un fichier de cette taille et de cette date ?
    fn vaut_pour(&self, taille: u64, mtime: Option<u64>) -> bool {
        self.taille == taille && self.mtime == mtime
    }
    /// Cette carte d'envoi vise-t-elle bien ce chemin distant ?
    ///
    /// Trouvé par l'audit du 7 septembre 2026 : une carte `.envoi.reprise`
    /// laissée par un envoi interrompu vers un chemin (ou un hôte) était
    /// réutilisée pour un autre envoi du même fichier local vers un chemin
    /// différent. `upload_reprise` reprenait alors à l'octet `deja` et greffait
    /// la queue du fichier au milieu d'un fichier distant sans rapport, pourvu
    /// qu'il soit assez gros pour passer le test `fin <= distant_len` : ni
    /// troncature ni erreur, un fichier distant corrompu en silence. La carte
    /// porte donc désormais sa cible et ne vaut que pour elle.
    fn vise(&self, remote: &str) -> bool {
        self.cible.as_deref() == Some(remote)
    }
}

/// Octets envoyés au moins entre deux points de contrôle d'un envoi.
const POINT_DE_CONTROLE: u64 = 4 * 1024 * 1024;
/// Temps au moins entre deux points de contrôle d'un envoi.
const CADENCE_POINT_DE_CONTROLE: std::time::Duration = std::time::Duration::from_secs(2);

/// Faut-il poser un point de contrôle d'envoi, `octets` envoyés et `ecoule`
/// passé depuis le précédent (ou le départ, `premier`) ?
///
/// Audit du 27 novembre 2026 (PR-7) : il en tombait un toutes les 4 Mio. Or
/// un point de contrôle vide la file d'écritures (au plus un aller-retour)
/// puis attend le `fsync@openssh.com` du serveur (un aller-retour de plus et
/// son disque), pendant quoi rien ne part : invisible à une écriture en vol,
/// un quart du débit une fois les seize écritures rétablies (4 Mio partent en
/// 0,4 s à 40 ms, la bulle en coûte au moins 80 ms). Cadencé au temps, il garde
/// la promesse de la carte, « une coupure ne coûte que quelques secondes »,
/// sans freiner un lien rapide. Les 4 Mio restent un plancher : sur un lien
/// lent, pas de `fsync` distant pour quelques kilo-octets. Le PREMIER point
/// tombe toujours à 4 Mio : sans carte, un envoi unitaire interrompu ne se
/// relance pas (le distant partiel existe, l'écrasement est refusé), et cette
/// fenêtre ne doit pas s'allonger de 4 Mio à deux secondes de lien rapide.
fn point_de_controle_du(octets: u64, ecoule: std::time::Duration, premier: bool) -> bool {
    octets >= POINT_DE_CONTROLE && (premier || ecoule >= CADENCE_POINT_DE_CONTROLE)
}

/// Les bandes d'un fichier : `[debut, fin)`, au plus `BANDES_MAX`.
fn bandes(total: u64) -> Vec<(u64, u64)> {
    const BANDES_MAX: u64 = 8;
    let n = (total / (CHUNK as u64)).clamp(1, BANDES_MAX);
    let taille = total.div_ceil(n);
    (0..n)
        .map(|i| (i * taille, ((i + 1) * taille).min(total)))
        .filter(|(d, f)| d < f)
        .collect()
}

/// Les événements qu'une bande rapporte à la tâche qui tient la progression.
enum Bande {
    Octets(u64),
    Finie(u64, u64),
}

impl SftpHandle {
    /// Parcourt un dossier distant, dossiers d'abord, puis fichiers, en
    /// profondeur ; les chemins sont relatifs à `racine`.
    pub async fn parcourir(&self, racine: &str) -> Result<Vec<EntreeDistante>> {
        let mut entrees = Vec::new();
        let mut a_visiter = vec![String::new()];
        while let Some(rel) = a_visiter.pop() {
            let chemin = if rel.is_empty() {
                racine.to_owned()
            } else {
                joindre(racine, &rel)
            };
            for e in self.list(&chemin).await? {
                if e.name == "." || e.name == ".." {
                    continue;
                }
                // Le nom vient du serveur : un « ../x » ou un « a/b » y
                // deviendrait un chemin qui sort du dossier de réception (ou
                // du dossier cible, pour un relais). On s'arrête, plutôt que
                // d'ignorer une entrée d'un serveur qui ment.
                //
                // Audit du 27 novembre 2026 (WI-5) : la règle NTFS (« : »,
                // noms réservés…) s'appliquait ici aussi, sous Windows, donc
                // au relais d'un Linux à un autre, qui n'écrit rien sur le
                // poste : elle ne s'applique plus qu'au téléchargement, et
                // sans arrêter le dossier (`ecarter_les_noms_impossibles`).
                // La règle d'évasion, elle, reste stricte et commune.
                anyhow::ensure!(
                    nom_sans_evasion(&e.name),
                    "Le serveur annonce une entrée au nom interdit sous {chemin} : {:?}",
                    e.name
                );
                // Liens symboliques et fichiers spéciaux : ignorés, comme
                // `scp -r` sans `-L`, symétriquement à `upload_dir_with`.
                // Trouvé par l'audit du 7 septembre 2026 : `list` déduit le type
                // d'un `lstat` (SSH_FXP_READDIR), donc un lien y est un
                // `FileType::Symlink` dont `is_dir` est faux ; `parcourir` le
                // rangeait alors en fichier et `download_reprise`/`relayer_vers`
                // tentaient d'`open` dessus. Un lien vers un dossier ouvrait un
                // répertoire (échec à la lecture), un lien cassé échouait à
                // l'ouverture, et l'erreur remontait par `?` en interrompant TOUT
                // le dossier ; un relais créait même un fichier vide à sa place.
                if !e.is_dir && !e.is_file {
                    continue;
                }
                anyhow::ensure!(
                    entrees.len() < ENTREES_MAX,
                    "Plus de {ENTREES_MAX} entrées sous {racine} : le parcours s'arrête."
                );
                let rel_e = if rel.is_empty() {
                    e.name.clone()
                } else {
                    format!("{rel}/{}", e.name)
                };
                if e.is_dir {
                    a_visiter.push(rel_e.clone());
                }
                entrees.push(EntreeDistante {
                    chemin: rel_e,
                    dossier: e.is_dir,
                    taille: if e.is_dir { 0 } else { e.size },
                    mtime: e.modified,
                });
            }
        }
        // Les dossiers d'abord, du moins profond au plus profond : chacun
        // existe avant ce qu'il contient.
        entrees.sort_by(|a, b| {
            b.dossier
                .cmp(&a.dossier)
                .then(
                    a.chemin
                        .matches('/')
                        .count()
                        .cmp(&b.chemin.matches('/').count()),
                )
                .then(a.chemin.cmp(&b.chemin))
        });
        Ok(entrees)
    }

    /// Télécharge un dossier distant entier dans `local` (créé), fichier par
    /// fichier, chacun repris s'il l'avait été à moitié. Rend les octets
    /// reçus ; une entrée que le disque du poste ne peut pas nommer est
    /// écartée (voir [`download_dir_bilan`](Self::download_dir_bilan)) et
    /// signalée au journal.
    pub async fn download_dir_with(
        &self,
        remote_dir: &str,
        local: &Path,
        annulation: Option<&Annulation>,
        progress: impl FnMut(Avancement),
    ) -> Result<u64> {
        let bilan = self
            .download_dir_bilan(remote_dir, local, annulation, progress)
            .await?;
        if !bilan.ecartes.is_empty() {
            tracing::warn!(
                "{} : {} entrée(s) non téléchargée(s), nom impossible sur ce système : {:?}",
                remote_dir,
                bilan.ecartes.len(),
                bilan.ecartes
            );
        }
        Ok(bilan.octets)
    }

    /// Comme [`download_dir_with`](Self::download_dir_with), avec la liste
    /// des entrées écartées, pour que l'interface la montre.
    pub async fn download_dir_bilan(
        &self,
        remote_dir: &str,
        local: &Path,
        annulation: Option<&Annulation>,
        progress: impl FnMut(Avancement),
    ) -> Result<BilanDossier> {
        let (entrees, ecartes) =
            ecarter_les_noms_impossibles(self.parcourir(remote_dir).await?, cfg!(windows));
        let total: u64 = entrees.iter().map(|e| e.taille).sum();
        tokio::fs::create_dir_all(local)
            .await
            .with_context(|| format!("Création de {}", local.display()))?;
        // Les dossiers d'abord, dans l'ordre du parcours (chacun avant ce qu'il
        // contient) ; les fichiers ensuite, plusieurs à la fois (C-perf-3 b).
        for e in entrees.iter().filter(|e| e.dossier) {
            verifier(annulation)?;
            let cible = local.join(e.chemin.replace('/', std::path::MAIN_SEPARATOR_STR));
            tokio::fs::create_dir_all(&cible)
                .await
                .with_context(|| format!("Création de {}", cible.display()))?;
        }
        let fichiers: Vec<(&EntreeDistante, u32)> = entrees
            .iter()
            .filter(|e| !e.dossier)
            .map(|e| (e, places_pour(e.taille)))
            .collect();
        en_vol(
            fichiers,
            total,
            annulation,
            progress,
            move |e, arret, tx| async move {
                verifier(Some(&arret))?;
                let cible = local.join(e.chemin.replace('/', std::path::MAIN_SEPARATOR_STR));
                let suivi = Rapporteur::nouveau(e.chemin.clone(), tx);
                // La taille et la date viennent du parcours (`READDIR`) : pas
                // de `STAT` de plus par fichier (PR-3).
                let n = self
                    .download_reprise_connu(
                        &joindre(remote_dir, &e.chemin),
                        &cible,
                        e.taille,
                        e.mtime,
                        Some(&arret),
                        |f, _| suivi.avance(f),
                    )
                    .await
                    .map_err(|err| avec_le_fichier(err, &e.chemin))?;
                suivi.fini(n);
                Ok(n)
            },
        )
        .await
        .map(|octets| BilanDossier { octets, ecartes })
    }

    /// Téléverse un dossier local entier sous `remote_dir` (créé), fichier par
    /// fichier, chacun repris s'il l'avait été à moitié.
    pub async fn upload_dir_with(
        &self,
        local: &Path,
        remote_dir: &str,
        annulation: Option<&Annulation>,
        progress: impl FnMut(Avancement),
    ) -> Result<u64> {
        // Parcours local : dossiers d'abord, fichiers ensuite, chemins relatifs.
        let mut dossiers: Vec<PathBuf> = vec![PathBuf::new()];
        let mut fichiers: Vec<(PathBuf, u64)> = Vec::new();
        let mut pile = vec![PathBuf::new()];
        while let Some(rel) = pile.pop() {
            let mut lecture = tokio::fs::read_dir(local.join(&rel))
                .await
                .with_context(|| format!("Lecture de {}", local.join(&rel).display()))?;
            while let Some(e) = lecture.next_entry().await? {
                anyhow::ensure!(
                    dossiers.len() + fichiers.len() < ENTREES_MAX,
                    "Plus de {ENTREES_MAX} entrées sous {} : l'envoi s'arrête.",
                    local.display()
                );
                let m = e.metadata().await?;
                let rel_e = rel.join(e.file_name());
                if m.is_dir() {
                    dossiers.push(rel_e.clone());
                    pile.push(rel_e);
                } else if m.is_file() {
                    fichiers.push((rel_e, m.len()));
                }
                // Liens et fichiers spéciaux : ignorés, comme scp -r sans -L.
            }
        }
        dossiers.sort();
        fichiers.sort();
        let total: u64 = fichiers.iter().map(|(_, t)| *t).sum();
        for d in &dossiers {
            verifier(annulation)?;
            let chemin = joindre(remote_dir, &rel_texte(d));
            // Un dossier déjà présent n'est pas une erreur : on continue dedans.
            if self.sftp.metadata(&chemin).await.is_err() {
                self.mkdir(&chemin).await?;
            }
        }
        // Les dossiers sont créés ci-dessus, dans l'ordre ; les fichiers
        // partent plusieurs à la fois (C-perf-3 b).
        en_vol(
            fichiers.iter().map(|f| (f, places_pour(f.1))).collect(),
            total,
            annulation,
            progress,
            move |(rel, _): &(PathBuf, u64), arret, tx| async move {
                verifier(Some(&arret))?;
                let nom = rel_texte(rel);
                let suivi = Rapporteur::nouveau(nom.clone(), tx);
                let n = self
                    .upload_reprise(
                        &local.join(rel),
                        &joindre(remote_dir, &nom),
                        // Fusion volontaire : un fichier déjà présent est
                        // remplacé, comme une resynchronisation de dossier
                        // l'attend.
                        false,
                        Some(&arret),
                        |f, _| suivi.avance(f),
                    )
                    .await
                    .map_err(|err| avec_le_fichier(err, &nom))?;
                suivi.fini(n);
                Ok(n)
            },
        )
        .await
    }

    /// Téléchargement d'un fichier, annulable et repris là où il s'était
    /// arrêté : les bandes déjà complètes (carte `.part.reprise`) ne sont pas
    /// redemandées, pour peu que le fichier distant ait encore la même taille
    /// et la même date. Un fichier trop petit pour les bandes repart de zéro.
    pub async fn download_reprise(
        &self,
        remote: &str,
        local: &Path,
        annulation: Option<&Annulation>,
        progress: impl FnMut(u64, u64),
    ) -> Result<u64> {
        let meta = self.sftp.metadata(remote).await.ok();
        let total = meta
            .as_ref()
            .map_or(0, russh_sftp::protocol::FileAttributes::len);
        let mtime = meta.as_ref().and_then(|m| m.mtime).map(u64::from);
        self.download_reprise_connu(remote, local, total, mtime, annulation, progress)
            .await
    }

    /// [`download_reprise`](Self::download_reprise) sur un fichier dont la
    /// taille et la date sont déjà lues (taille 0 : inconnue).
    ///
    /// Audit du 27 novembre 2026 (PR-3) : un dossier lisait la taille de chaque
    /// fichier dans le `READDIR` du parcours, la jetait, puis la redemandait
    /// par un `STAT` : un aller-retour de plus sur les quatre d'un petit
    /// fichier. Le motif de C-perf-3 a, un cran plus haut.
    async fn download_reprise_connu(
        &self,
        remote: &str,
        local: &Path,
        total: u64,
        mtime: Option<u64>,
        annulation: Option<&Annulation>,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<u64> {
        if total <= (2 * CHUNK) as u64 {
            verifier(annulation)?;
            // La taille lue ci-dessus sert telle quelle (C-perf-3 a).
            return self
                .telecharger_connu(remote, local, total, annulation, progress)
                .await;
        }
        let partiel = chemin_partiel(local);
        let carte = chemin_reprise(&partiel);
        let mut deja = Reprise::lire(&carte)
            .filter(|r| r.vaut_pour(total, mtime) && partiel.exists())
            .unwrap_or_default();
        // Trouvé par l'audit du 7 septembre 2026 : la carte est écrite avec un
        // `fsync` (`ecrire_atomiquement`), mais les octets d'une bande n'avaient
        // qu'un `flush` (pas de `fsync`) à l'annonce « faite ». Après une coupure,
        // la carte durable pouvait promettre une bande que le `.part` n'avait pas
        // encore sur le disque, plus court d'autant. On tient désormais chaque
        // bande durable avant de l'annoncer (voir `une_bande_annulable`), et à la
        // reprise on refuse une carte dont le `.part` est plus court que la plus
        // grande bande dite faite (ou dont la métadonnée est illisible) : on
        // repart de zéro, comme pour une carte périmée. Garde nécessaire mais
        // partielle : un `.part` de la bonne longueur mais troué (zéros au milieu
        // d'une bande dite faite) n'est pas rattrapé ici, seul le `fsync` par
        // bande le ferme (il faudrait un condensé par bande pour l'attraper).
        let plus_grande_fin = deja.faites.iter().map(|(_, f)| *f).max().unwrap_or(0);
        let part_assez_long = tokio::fs::metadata(&partiel)
            .await
            .is_ok_and(|m| m.len() >= plus_grande_fin);
        if !part_assez_long {
            deja.faites.clear();
        }
        if deja.faites.is_empty() {
            tokio::fs::File::create(&partiel)
                .await
                .with_context(|| format!("Création local {}", partiel.display()))?;
        }
        let mut reprise = Reprise {
            taille: total,
            mtime,
            faites: deja.faites,
            cible: None,
        };
        let deja_fait: u64 = reprise.faites.iter().map(|(d, f)| f - d).sum();
        progress(deja_fait, total);

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Bande>();
        let mut travaux = Vec::new();
        for (debut, fin) in bandes(total) {
            if reprise.faites.contains(&(debut, fin)) {
                continue;
            }
            travaux.push(self.une_bande_annulable(
                remote,
                &partiel,
                debut,
                fin,
                annulation,
                tx.clone(),
            ));
        }
        drop(tx);
        let annonces = async {
            let mut fait = deja_fait;
            while let Some(ev) = rx.recv().await {
                match ev {
                    Bande::Octets(n) => {
                        fait += n;
                        progress(fait, total);
                    }
                    Bande::Finie(d, f) => {
                        reprise.faites.push((d, f));
                        reprise.ecrire(&carte);
                    }
                }
            }
            fait
        };
        let (issues, fait) = tokio::join!(futures::future::join_all(travaux), annonces);
        for issue in issues {
            issue?;
        }
        anyhow::ensure!(
            fait == total,
            "Transfert incomplet : {fait} octets reçus sur {total} annoncés \
             (le fichier distant a changé pendant le transfert)."
        );
        let _ = tokio::fs::remove_file(&carte).await;
        promouvoir_partiel(&partiel, local).await?;
        Ok(fait)
    }

    /// Une bande, qui s'arrête entre deux blocs si le transfert est annulé et
    /// annonce sa fin pour la carte de reprise.
    async fn une_bande_annulable(
        &self,
        remote: &str,
        partiel: &Path,
        debut: u64,
        fin: u64,
        annulation: Option<&Annulation>,
        tx: tokio::sync::mpsc::UnboundedSender<Bande>,
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
            verifier(annulation)?;
            let vise = usize::try_from(reste.min(CHUNK as u64)).unwrap_or(CHUNK);
            let garde = self.avant_lecture(LECTURE_MAX as u64).await;
            let n = distant
                .read(&mut buf[..vise])
                .await
                .context("Lecture distante")?;
            drop(garde);
            if n == 0 {
                break;
            }
            local
                .write_all(&buf[..n])
                .await
                .context("Écriture locale")?;
            reste -= n as u64;
            let _ = tx.send(Bande::Octets(n as u64));
        }
        local.flush().await.context("Vidage local")?;
        if reste == 0 {
            // Trouvé par l'audit du 7 septembre 2026 : la carte de reprise est
            // écrite avec un `fsync`, mais `flush` (tokio) n'attend que la fin de
            // l'écriture en cours, sans `fsync`. Sans ce `sync_data`, une coupure
            // pouvait laisser la carte durable promettre une bande que le `.part`
            // n'avait pas encore sur le disque. On rend donc la bande durable
            // AVANT d'en annoncer la fin (la carte est écrite par une autre tâche
            // dès réception) ; une erreur d'E/S fait échouer la bande plutôt que
            // de la déclarer faite à tort.
            local.sync_data().await.context("Synchronisation local")?;
            let _ = tx.send(Bande::Finie(debut, fin));
        }
        Ok(())
    }

    /// Où reprendre un envoi (octet `deja`, 0 = départ), et refus d'un
    /// écrasement silencieux.
    ///
    /// Trouvé par l'audit du 7 septembre 2026 : un envoi de fichier unitaire
    /// (`sftp_upload`, glisser-déposer) faisait `create(remote)`, qui TRONQUE une
    /// cible du même nom — l'ancien fichier distant perdu sans un mot, quand la
    /// réception RDP, elle, n'écrase jamais (SECURITY.md). On refuse désormais
    /// d'écraser une cible qui existe déjà, SAUF quand c'est la reprise d'un envoi
    /// interrompu vers ce même chemin (carte `.envoi.reprise` valable, donc
    /// `deja > 0`). L'envoi de DOSSIER fusionne volontairement dans une
    /// arborescence déjà là (`refuser_ecrasement = false`) : la resynchro d'un
    /// dossier n'est pas cassée.
    async fn plan_envoi(
        &self,
        remote: &str,
        carte: &Path,
        total: u64,
        mtime: Option<u64>,
        refuser_ecrasement: bool,
    ) -> Result<u64> {
        // Audit du 27 novembre 2026 (PR-3) : le `STAT` distant ne sert qu'à
        // valider une carte (le distant porte-t-il encore ces octets ?) et à
        // refuser un écrasement. Sans carte candidate, un envoi de dossier
        // (`refuser_ecrasement` faux) le payait pour rien, un aller-retour sur
        // les quatre d'un petit fichier. La carte locale se lit d'abord ; le
        // refus d'écrasement d'un envoi unitaire garde son `STAT`.
        let candidate = Reprise::lire(carte)
            .filter(|r| r.vaut_pour(total, mtime) && r.vise(remote))
            .and_then(|r| r.faites.first().map(|(_, fin)| *fin))
            .filter(|fin| *fin > 0 && *fin < total);
        if candidate.is_none() && !refuser_ecrasement {
            return Ok(0);
        }
        let distant = self.sftp.metadata(remote).await.ok();
        let distant_len = distant
            .as_ref()
            .map_or(0, russh_sftp::protocol::FileAttributes::len);
        let deja = candidate.filter(|fin| *fin <= distant_len).unwrap_or(0);
        if refuser_ecrasement && deja == 0 && distant.is_some() {
            anyhow::bail!(
                "Le fichier distant « {remote} » existe déjà : envoi refusé pour ne pas l'écraser."
            );
        }
        Ok(deja)
    }

    /// Téléversement d'un fichier, annulable, repris là où il s'était arrêté.
    ///
    /// Un seul descripteur, **pas** de bandes : `File` de russh-sftp tient
    /// déjà `ECRITURES_EN_VOL` écritures en vol (une seule du 21 au 27 novembre
    /// 2026, voir `sous_systeme`), et la mesure (voir la feuille de route,
    /// axe 3) a tranché contre huit descripteurs en parallèle, quatre fois plus
    /// lents en réseau local pour 1,2 × à 40 ms d'aller-retour.
    ///
    /// La reprise : une carte `<local>.envoi.reprise` note, à chaque point de
    /// contrôle (voir `point_de_controle_du`, après un vidage), ce qui est
    /// sûrement écrit chez le distant, avec la taille et la date du
    /// fichier local. Relancer le même envoi repart de là, si le fichier n'a
    /// pas changé et si le distant porte encore au moins ces octets.
    pub async fn upload_reprise(
        &self,
        local: &Path,
        remote: &str,
        refuser_ecrasement: bool,
        annulation: Option<&Annulation>,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<u64> {
        use russh_sftp::protocol::OpenFlags;
        use tokio::io::AsyncSeekExt as _;

        let meta = tokio::fs::metadata(local)
            .await
            .with_context(|| format!("Lecture de {}", local.display()))?;
        let total = meta.len();
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        let mut carte = local.as_os_str().to_owned();
        carte.push(".envoi.reprise");
        let carte = PathBuf::from(carte);
        // Reprise éventuelle et refus d'un écrasement silencieux (voir
        // `plan_envoi`) : `deja` est l'octet où l'envoi doit reprendre.
        let deja = self
            .plan_envoi(remote, &carte, total, mtime, refuser_ecrasement)
            .await?;
        if total <= (2 * CHUNK) as u64 {
            verifier(annulation)?;
            return self.upload_with(local, remote, progress).await;
        }

        let mut source = tokio::fs::File::open(local)
            .await
            .with_context(|| format!("Ouverture local {}", local.display()))?;
        let mut distant = if deja > 0 {
            // On reprend : le fichier distant existe et porte déjà `deja` octets.
            let mut f = self
                .sftp
                .open_with_flags(remote, OpenFlags::WRITE)
                .await
                .with_context(|| format!("Ouverture distant {remote}"))?;
            source
                .seek(std::io::SeekFrom::Start(deja))
                .await
                .context("Positionnement local")?;
            f.seek(std::io::SeekFrom::Start(deja))
                .await
                .context("Positionnement distant")?;
            f
        } else {
            self.sftp
                .create(remote)
                .await
                .with_context(|| format!("Création distant {remote}"))?
        };
        let mut reprise = Reprise {
            taille: total,
            mtime,
            faites: vec![(0, deja)],
            cible: Some(remote.to_string()),
        };
        let mut fait = deja;
        let mut dernier_point = deja;
        let mut dernier_instant = std::time::Instant::now();
        progress(fait, total);
        let mut buf = vec![0u8; CHUNK];
        let envoi = async {
            loop {
                verifier(annulation)?;
                let n = source.read(&mut buf).await.context("Lecture locale")?;
                if n == 0 {
                    break;
                }
                distant
                    .write_all(&buf[..n])
                    .await
                    .context("Écriture distante")?;
                fait += n as u64;
                progress(fait, total);
                if point_de_controle_du(
                    fait - dernier_point,
                    dernier_instant.elapsed(),
                    dernier_point == deja,
                ) {
                    // Vidé, donc sûrement chez le distant : la carte peut le dire.
                    distant.flush().await.context("Vidage distant")?;
                    dernier_point = fait;
                    dernier_instant = std::time::Instant::now();
                    reprise.faites = vec![(0, fait)];
                    reprise.ecrire(&carte);
                }
            }
            distant.shutdown().await.context("Fermeture distante")?;
            anyhow::Ok(())
        }
        .await;
        if let Err(e) = envoi {
            // Ce qui a été vidé est acquis ; on l'inscrit pour la reprise. Une
            // annulation vide d'abord : tout ce qui est parti compte.
            if e.to_string().contains(ANNULE) && distant.flush().await.is_ok() {
                reprise.faites = vec![(0, fait)];
                reprise.ecrire(&carte);
            }
            return Err(e);
        }
        anyhow::ensure!(
            fait == total,
            "Envoi incomplet : {fait} octets envoyés sur {total} (le fichier local a changé pendant l'envoi)."
        );
        let _ = tokio::fs::remove_file(&carte).await;
        Ok(fait)
    }

    /// Ce qu'il faut vérifier avant qu'un seul octet ne parte en relais, et le
    /// partiel qui recevra la copie chez la cible (avec la taille de la source).
    async fn preparer_relais(
        &self,
        remote: &str,
        cible: &SftpHandle,
        remote_cible: &str,
        refuser_ecrasement: bool,
    ) -> Result<(u64, String)> {
        // Trouvé par l'audit du 7 septembre 2026 : `metadata` échouant (lien
        // symbolique cassé, fichier disparu) donnait `total = 0` ; la cible était
        // alors créée vide et la copie annoncée réussie. Et une source lisible par
        // `stat` mais pas par `open` (droits root) tronquait une cible existante
        // avant d'échouer. On lit d'abord les attributs — erreur propagée — et
        // l'on s'assure que la source S'OUVRE, sans rien créer chez la cible.
        let total = self
            .sftp
            .metadata(remote)
            .await
            .with_context(|| format!("Lecture des attributs de {remote}"))?
            .len();
        {
            let sonde = self
                .sftp
                .open(remote)
                .await
                .with_context(|| format!("Ouverture distant {remote}"))?;
            drop(sonde); // les bandes rouvrent leur propre descripteur
        }
        let partiel = format!("{remote_cible}.part");
        // Trouvé par l'audit du 7 septembre 2026 : `create(remote_cible)` TRONQUE
        // un fichier du même nom chez la cible — une copie de fichier vers un
        // autre hôte écrasait sans un mot. On refuse d'écraser une cible qui
        // existe déjà (copie unitaire) ; la copie de DOSSIER fusionne
        // (`refuser_ecrasement = false`), comme l'envoi.
        if refuser_ecrasement && cible.sftp.metadata(remote_cible).await.is_ok() {
            anyhow::bail!(
                "Le fichier « {remote_cible} » existe déjà chez la cible : copie refusée pour ne pas l'écraser."
            );
        }
        // Relecture du 9 septembre 2026 : depuis que le relais passe par un
        // partiel, le même garde-fou doit le couvrir. Le `create` ci-dessous
        // TRONQUE le partiel comme n'importe quel fichier, et un `.part` n'est
        // pas un déchet : quand la promotion échoue, on le conserve exprès parce
        // qu'il porte les seuls octets complets. Une copie qui promet de ne rien
        // écraser ne peut pas effacer cela au passage.
        if refuser_ecrasement && cible.sftp.metadata(&partiel).await.is_ok() {
            anyhow::bail!(
                "Une copie interrompue de « {remote_cible} » attend encore chez la cible, dans « {partiel} » : copie refusée pour ne pas l'écraser."
            );
        }
        // Trouvé par l'audit du 9 septembre 2026 : la cible était créée, donc
        // TRONQUÉE, avant que les bandes ne partent, et rien n'était nettoyé si
        // l'une d'elles échouait (annulation d'un clic, coupure réseau). Un
        // relais interrompu laissait chez le second serveur un fragment troué
        // (les bandes écrivent à des décalages disjoints) à la place du fichier
        // valide, sans carte de reprise pour le rattraper ni rien pour signaler
        // la perte, d'autant plus fâcheux que la copie de DOSSIER fusionne
        // (`refuser_ecrasement = false`) et passe donc là-dessus à chaque
        // resynchro. On écrit désormais dans un partiel qu'on ne promeut qu'une
        // fois tous les octets arrivés.
        cible
            .sftp
            .create(&partiel)
            .await
            .with_context(|| format!("Création distant {partiel}"))?
            .shutdown()
            .await
            .context("Fermeture distante")?;
        Ok((total, partiel))
    }

    /// Promeut un partiel distant sur son nom définitif, chez le serveur qui le
    /// porte. `SSH_FXP_RENAME` refuse une cible existante sur la plupart des
    /// serveurs (OpenSSH), là où un `rename` POSIX remplace sans broncher : on
    /// tente donc d'abord le renommage (atomique quand le serveur le veut
    /// bien, sans le moindre instant où le fichier manque) et l'on n'écarte
    /// celui en place qu'après son refus. Si le renommage échoue quand même,
    /// on dit où sont les octets plutôt que de les jeter : ils sont complets
    /// dans le partiel.
    async fn promouvoir_distant(&self, partiel: &str, cible: &str) -> Result<()> {
        if self.sftp.metadata(cible).await.is_ok() {
            if self.sftp.rename(partiel, cible).await.is_ok() {
                return Ok(());
            }
            self.sftp.remove_file(cible).await.with_context(|| {
                format!("Remplacement de {cible} (la copie complète est dans {partiel})")
            })?;
        }
        self.sftp.rename(partiel, cible).await.with_context(|| {
            format!("Renommage vers {cible} (la copie complète est dans {partiel})")
        })
    }

    /// Copie un fichier de ce serveur vers un autre, sans rien écrire sur le
    /// poste : les octets ne font que le traverser, par bandes, chaque bande
    /// avec son descripteur de lecture ici et son descripteur d'écriture
    /// là-bas.
    ///
    /// Comme le téléchargement, la copie passe par un `.part` chez la cible :
    /// interrompue, elle ne laisse rien, et le fichier qu'elle remplace reste
    /// entier jusqu'au dernier octet reçu.
    pub async fn relayer_vers(
        &self,
        remote: &str,
        cible: &SftpHandle,
        remote_cible: &str,
        refuser_ecrasement: bool,
        annulation: Option<&Annulation>,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<u64> {
        use russh_sftp::protocol::OpenFlags;
        use tokio::io::AsyncSeekExt as _;
        let (total, partiel) = self
            .preparer_relais(remote, cible, remote_cible, refuser_ecrasement)
            .await?;
        if total == 0 {
            cible.promouvoir_distant(&partiel, remote_cible).await?;
            return Ok(0);
        }
        let partiel_ref: &str = &partiel;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<u64>();
        let travaux: Vec<_> = bandes(total)
            .into_iter()
            .map(|(debut, fin)| {
                let tx = tx.clone();
                async move {
                    let mut source = self
                        .sftp
                        .open(remote)
                        .await
                        .with_context(|| format!("Ouverture distant {remote}"))?;
                    source
                        .seek(std::io::SeekFrom::Start(debut))
                        .await
                        .context("Positionnement source")?;
                    let mut dest = cible
                        .sftp
                        .open_with_flags(partiel_ref, OpenFlags::WRITE)
                        .await
                        .with_context(|| format!("Ouverture distant {partiel_ref}"))?;
                    dest.seek(std::io::SeekFrom::Start(debut))
                        .await
                        .context("Positionnement cible")?;
                    let mut buf = vec![0u8; CHUNK];
                    let mut reste = fin - debut;
                    while reste > 0 {
                        verifier(annulation)?;
                        let vise = usize::try_from(reste.min(CHUNK as u64)).unwrap_or(CHUNK);
                        let garde = self.avant_lecture(LECTURE_MAX as u64).await;
                        let n = source
                            .read(&mut buf[..vise])
                            .await
                            .context("Lecture source")?;
                        drop(garde);
                        if n == 0 {
                            break;
                        }
                        dest.write_all(&buf[..n]).await.context("Écriture cible")?;
                        reste -= n as u64;
                        let _ = tx.send(n as u64);
                    }
                    dest.shutdown().await.context("Fermeture cible")?;
                    anyhow::Ok(())
                }
            })
            .collect();
        drop(tx);
        let annonces = async {
            let mut fait = 0u64;
            while let Some(n) = rx.recv().await {
                fait += n;
                progress(fait, total);
            }
            fait
        };
        let (issues, fait) = tokio::join!(futures::future::join_all(travaux), annonces);
        let souci = issues.into_iter().find_map(Result::err).or_else(|| {
            (fait != total).then(|| {
                anyhow!(
                    "Copie incomplète : {fait} octets sur {total} (le fichier source a changé pendant la copie)."
                )
            })
        });
        if let Some(e) = souci {
            // Rien ne doit rester qui puisse se faire passer pour une copie
            // complète, ni encombrer la cible : le partiel s'en va avec l'échec.
            let _ = cible.sftp.remove_file(&partiel).await;
            return Err(e);
        }
        cible.promouvoir_distant(&partiel, remote_cible).await?;
        Ok(fait)
    }

    /// Copie un dossier entier de ce serveur vers un autre, sans passer par le
    /// disque du poste.
    pub async fn relayer_dir_vers(
        &self,
        remote_dir: &str,
        cible: &SftpHandle,
        remote_cible: &str,
        annulation: Option<&Annulation>,
        progress: impl FnMut(Avancement),
    ) -> Result<u64> {
        let entrees = self.parcourir(remote_dir).await?;
        let total: u64 = entrees.iter().map(|e| e.taille).sum();
        if cible.sftp.metadata(remote_cible).await.is_err() {
            cible.mkdir(remote_cible).await?;
        }
        // Les dossiers d'abord, dans l'ordre ; les fichiers ensuite, plusieurs
        // à la fois (C-perf-3 b).
        for e in entrees.iter().filter(|e| e.dossier) {
            verifier(annulation)?;
            let chez_cible = joindre(remote_cible, &e.chemin);
            if cible.sftp.metadata(&chez_cible).await.is_err() {
                cible.mkdir(&chez_cible).await?;
            }
        }
        let fichiers: Vec<(&EntreeDistante, u32)> = entrees
            .iter()
            .filter(|e| !e.dossier)
            .map(|e| (e, places_pour(e.taille)))
            .collect();
        en_vol(
            fichiers,
            total,
            annulation,
            progress,
            move |e, arret, tx| async move {
                verifier(Some(&arret))?;
                let suivi = Rapporteur::nouveau(e.chemin.clone(), tx);
                let n = self
                    .relayer_vers(
                        &joindre(remote_dir, &e.chemin),
                        cible,
                        &joindre(remote_cible, &e.chemin),
                        // Fusion volontaire dans l'arborescence cible déjà là ;
                        // le fichier homologue déjà présent n'est remplacé
                        // qu'une fois sa relève complète (partiel promu),
                        // jamais avant.
                        false,
                        Some(&arret),
                        |f, _| suivi.avance(f),
                    )
                    .await
                    .map_err(|err| avec_le_fichier(err, &e.chemin))?;
                suivi.fini(n);
                Ok(n)
            },
        )
        .await
    }
}

/// Ce qu'un téléchargement de dossier a fait.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BilanDossier {
    /// Octets reçus.
    pub octets: u64,
    /// Entrées écartées (chemins relatifs à la racine), parce que le disque
    /// du poste ne peut pas les nommer ; ce qu'elles contiennent l'est aussi.
    pub ecartes: Vec<String>,
}

/// Sépare d'un parcours ce que le disque local peut nommer de ce qu'il ne
/// peut pas (`windows` : la règle NTFS s'applique). Une entrée écartée est
/// rendue une fois, sans le contenu d'un dossier écarté, qui part avec lui.
///
/// Audit du 27 novembre 2026 (WI-5) : sous Windows, un seul nom à deux-points
/// faisait échouer tout le dossier (« Le serveur annonce une entrée au nom
/// interdit »). Or ces noms sont courants sous Linux : chaque message d'un
/// Maildir (`1700000000.M1P2.hote:2,S`), les horodatages ISO
/// (`sauvegarde-10:00.tar`). Deux issues étaient possibles. Transcrire le nom
/// (`%3A`, comme `WinSCP`) change ce que l'utilisateur retrouve, peut tomber sur
/// un fichier voisin déjà nommé ainsi (et l'écraser, ou le dédoubler), et ne
/// sait pas quoi faire d'un `CON` ; écarter l'entrée et le dire ne crée rien
/// que le serveur n'ait nommé, n'écrase rien et laisse le reste arriver.
/// C'est le choix retenu, le plus sûr ; la transcription reste au mainteneur.
/// La règle d'évasion de dossier (`..`, `/`, `\`, NUL) n'est pas ici : elle
/// arrête toujours le parcours, sur toutes les plateformes.
pub(crate) fn ecarter_les_noms_impossibles(
    entrees: Vec<EntreeDistante>,
    windows: bool,
) -> (Vec<EntreeDistante>, Vec<String>) {
    let mut gardees = Vec::with_capacity(entrees.len());
    let mut ecartes: Vec<String> = Vec::new();
    // Un parcours peut compter cent mille entrées (un Maildir) : la recherche
    // des déjà écartées ne doit pas être quadratique.
    let mut vus = std::collections::HashSet::new();
    for e in entrees {
        let mut prefixe = String::new();
        let mut sous_un_ecarte = false;
        for (i, nom) in e.chemin.split('/').enumerate() {
            if i > 0 {
                prefixe.push('/');
            }
            prefixe.push_str(nom);
            if !nom_representable_localement(nom, windows) {
                if vus.insert(prefixe.clone()) {
                    ecartes.push(prefixe.clone());
                }
                sous_un_ecarte = true;
                break;
            }
        }
        if !sous_un_ecarte {
            gardees.push(e);
        }
    }
    (gardees, ecartes)
}

/// Un nom qu'on peut créer tel quel sur le disque du poste. Sous Windows
/// (`windows`), NTFS refuse `< > : " | ? *` et les caractères de contrôle,
/// retire un point ou une espace final (et retombe sur un autre nom), et lit
/// CON, NUL, COM1… comme des périphériques. Pure, la plateforme en paramètre :
/// la règle se teste partout.
pub(crate) fn nom_representable_localement(nom: &str, windows: bool) -> bool {
    !windows
        || !(piege_ntfs(nom)
            || nom.contains(['<', '>', '"', '|', '?', '*'])
            || nom.chars().any(|c| u32::from(c) < 0x20))
}

/// Les pièges NTFS d'un nom que le reste du chemin ne voit pas : flux de
/// données alternatif (« : »), point ou espace final retiré, nom réservé.
fn piege_ntfs(nom: &str) -> bool {
    nom.contains(':') || nom.ends_with('.') || nom.ends_with(' ') || nom_reserve_windows(nom)
}

/// Un nom qui ne fait pas sortir du dossier où on le recopie : un seul
/// composant, sans séparateur ni octet nul, ni `.` ni `..`. Commun à toutes les
/// plateformes et à tous les transferts (voir `parcourir`).
pub(crate) fn nom_sans_evasion(nom: &str) -> bool {
    !nom.is_empty() && nom != "." && nom != ".." && !nom.contains(['/', '\\', '\0'])
}

/// Un nom d'entrée qu'on accepte de recopier tel quel sous un dossier, et
/// plus largement un nom de fichier qu'on accepte de créer : un seul
/// composant, sans séparateur ni octet nul, ni `.` ni `..`. Un serveur SFTP
/// peut annoncer n'importe quoi et un nom de clé vient de l'utilisateur ;
/// c'est ici que ça s'arrête.
///
/// Sous Windows on écarte en plus les pièges NTFS que le reste du chemin ne
/// voit pas. Trouvé par l'audit du 7 septembre 2026 : `keys::generate`
/// acceptait « travail:pro », que NTFS lit comme le flux de données alternatif
/// « pro » du fichier « travail » — la clé privée finissait dans un flux caché,
/// invisible du panneau. Les noms réservés (CON, NUL, COM1…) désignent un
/// périphérique et non un fichier, et un point ou une espace final est retiré
/// par le système, ce qui fait retomber sur un autre nom.
pub(crate) fn nom_d_entree_sur(nom: &str) -> bool {
    nom_sans_evasion(nom) && !(cfg!(windows) && piege_ntfs(nom))
}

/// Vrai si `nom` est un nom de périphérique réservé par Windows (CON, PRN,
/// AUX, NUL, COM1-9, LPT1-9), quelle que soit la casse et même suivi d'une
/// extension : `com1.txt` désigne toujours le port série, pas un fichier.
/// Le calcul ne dépend pas du système hôte, ce qui rend la règle testable
/// partout ; l'appel n'est fait que sous Windows.
pub(crate) fn nom_reserve_windows(nom: &str) -> bool {
    // Le radical est la partie avant le premier point (con.txt -> con), puis
    // débarrassée des espaces finaux, que Windows retire aussi.
    let radical = nom.split('.').next().unwrap_or(nom).trim_end_matches(' ');
    let haut = radical.to_ascii_uppercase();
    matches!(haut.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (haut.len() == 4
            && (haut.starts_with("COM") || haut.starts_with("LPT"))
            && matches!(haut.as_bytes()[3], b'1'..=b'9'))
}

/// Concatène un dossier distant et un chemin relatif (séparateur `/`).
fn joindre(dir: &str, rel: &str) -> String {
    if dir.is_empty() || dir == "." {
        rel.to_owned()
    } else if dir.ends_with('/') {
        format!("{dir}{rel}")
    } else {
        format!("{dir}/{rel}")
    }
}

/// Un chemin relatif local en texte à séparateurs `/`, tel que le distant le veut.
fn rel_texte(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Résolveur de chemin de téléchargement local par défaut.
///
/// Le repli passe par `repertoire_personnel()`, le point d'entrée unique du
/// crate, et non par `dirs::home_dir()` : sous `AVASH_HOME` — les tests, ou une
/// installation qui isole sa configuration —, c'est là que les fichiers
/// doivent atterrir.
///
/// Sous `AVASH_HOME`, tout reste sous ce toit : `dirs::download_dir()` ignore
/// la variable et, sous Windows, rendait le vrai dossier Téléchargements de
/// l'utilisateur, hors du bac à sable des tests (cinquième passage Windows de
/// la suite complète, 05/09/2026). Le sous-dossier des téléchargements y est
/// pris s'il existe, le foyer sinon.
#[must_use]
pub fn default_local_dir() -> PathBuf {
    if std::env::var_os("AVASH_HOME").is_some_and(|v| !v.is_empty()) {
        if let Some(foyer) = crate::repertoire_personnel() {
            return ["Téléchargements", "Downloads"]
                .iter()
                .map(|n| foyer.join(n))
                .find(|d| d.is_dir())
                .unwrap_or(foyer);
        }
    }
    dirs::download_dir()
        .or_else(crate::repertoire_personnel)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests_bandes {
    use super::{bandes, joindre, point_de_controle_du, rel_texte, Reprise, CHUNK};

    /// Audit du 27 novembre 2026 (PR-7) : le point de contrôle d'envoi tombait
    /// tous les 4 Mio, quel que soit le débit. Passé le premier (toujours à
    /// 4 Mio), il faut deux secondes ET 4 Mio depuis le précédent : ni un lien
    /// rapide freiné par un `fsync` distant à chaque fraction de seconde, ni un
    /// lien lent qui en paierait un pour quelques octets.
    #[test]
    fn le_point_de_controle_suit_le_temps_et_non_les_octets() {
        use std::time::Duration;
        const MIO: u64 = 1024 * 1024;
        let deux_s = Duration::from_secs(2);
        assert!(point_de_controle_du(4 * MIO, deux_s, false));
        assert!(point_de_controle_du(
            400 * MIO,
            Duration::from_secs(5),
            false
        ));
        assert!(
            !point_de_controle_du(400 * MIO, Duration::from_millis(1999), false),
            "un lien rapide ne vide pas sa file à chaque fraction de seconde"
        );
        assert!(
            !point_de_controle_du(4 * MIO - 1, Duration::from_secs(60), false),
            "un lien lent ne paie pas un fsync distant pour quelques octets"
        );
        assert!(
            point_de_controle_du(4 * MIO, Duration::ZERO, true),
            "le premier point tombe à 4 Mio, sans attendre"
        );
        assert!(!point_de_controle_du(4 * MIO - 1, Duration::ZERO, true));
    }

    /// Audit du 12 septembre 2026 (C-SIL-13) : une carte de reprise qui ne
    /// s'écrit pas (répertoire devenu fichier, disque plein) était avalée ;
    /// le journal la nomme désormais.
    #[test]
    fn une_carte_de_reprise_non_ecrite_est_signalee_au_journal() {
        let dir = std::env::temp_dir().join(format!("avash-carte-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("fichier"), b"x").unwrap();
        // Le parent de la carte est un fichier : impossible d'y écrire.
        let carte = dir.join("fichier").join("x.part.reprise");
        let ((), journal) =
            crate::testutil::avertissements_pendant(|| Reprise::default().ecrire(&carte));
        assert!(
            journal.iter().any(|l| l.contains("x.part.reprise")),
            "le journal nomme la carte : {journal:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Les bandes couvrent exactement le fichier, sans trou ni recouvrement,
    /// et il n'y en a jamais plus de huit.
    #[test]
    fn les_bandes_couvrent_le_fichier_sans_trou() {
        for total in [
            1u64,
            CHUNK as u64,
            3 * CHUNK as u64 + 17,
            100 * CHUNK as u64,
        ] {
            let b = bandes(total);
            assert!(!b.is_empty() && b.len() <= 8, "{total} : {b:?}");
            assert_eq!(b[0].0, 0);
            assert_eq!(b.last().unwrap().1, total);
            for w in b.windows(2) {
                assert_eq!(w[0].1, w[1].0, "trou ou recouvrement : {w:?}");
            }
        }
        assert_eq!(bandes(0), [] as [(u64, u64); 0]);
    }

    /// Une carte ne vaut que pour le même fichier : même taille, même date.
    #[test]
    fn une_carte_ne_vaut_que_pour_le_meme_fichier() {
        let r = Reprise {
            taille: 10,
            mtime: Some(5),
            faites: vec![(0, 5)],
            cible: None,
        };
        assert!(r.vaut_pour(10, Some(5)));
        assert!(!r.vaut_pour(11, Some(5)));
        assert!(!r.vaut_pour(10, Some(6)));
        assert!(!r.vaut_pour(10, None));
    }

    /// Trouvé par l'audit du 7 septembre 2026 : une carte d'envoi ne vaut que
    /// pour la cible distante qu'elle porte. Sans ce contrôle, une carte laissée
    /// par un envoi interrompu vers `/a/backup.sql` était réutilisée pour un
    /// envoi du même fichier local vers `/b/backup.sql` et greffait la queue du
    /// fichier au milieu de ce fichier distant sans rapport.
    #[test]
    fn une_carte_d_envoi_ne_vaut_que_pour_la_meme_cible_distante() {
        let r = Reprise {
            taille: 10,
            mtime: Some(5),
            faites: vec![(0, 4)],
            cible: Some("/a/backup.sql".into()),
        };
        assert!(r.vise("/a/backup.sql"));
        assert!(!r.vise("/b/backup.sql"), "chemin distant différent");
        // Une vieille carte sans cible enregistrée ne vaut pour aucun envoi :
        // mieux vaut repartir de zéro que se greffer sur un fichier inconnu.
        let ancienne = Reprise {
            cible: None,
            ..r.clone()
        };
        assert!(!ancienne.vise("/a/backup.sql"));
    }

    /// Trouvé par la revue de sécurité du commit : un serveur hostile qui
    /// nomme une entrée « ../évasion » ferait écrire hors du dossier local.
    #[test]
    fn un_nom_d_entree_qui_sort_du_dossier_est_refuse() {
        use super::nom_d_entree_sur;
        assert!(nom_d_entree_sur("rapport.md"));
        assert!(nom_d_entree_sur("..rapport"));
        assert!(!nom_d_entree_sur(".."));
        assert!(!nom_d_entree_sur("."));
        assert!(!nom_d_entree_sur(""));
        assert!(!nom_d_entree_sur("../evasion"));
        assert!(!nom_d_entree_sur("a/b"));
        assert!(!nom_d_entree_sur("a\\b"));
        assert!(!nom_d_entree_sur("nul\0"));
    }

    /// Audit du 27 novembre 2026 (WI-5) : la règle d'évasion vaut partout et
    /// pour tout transfert ; la règle NTFS ne vaut que pour le disque d'un
    /// poste Windows, sans arrêter le dossier.
    #[test]
    fn un_nom_a_deux_points_n_est_ecarte_que_du_disque_windows() {
        use super::{nom_representable_localement, nom_sans_evasion};
        for nom in [
            "1700000000.M1P2.hote:2,S",
            "sauvegarde-10:00.tar",
            "CON",
            "a?b",
            "fin.",
        ] {
            assert!(nom_sans_evasion(nom), "{nom} ne sort pas du dossier");
            assert!(nom_representable_localement(nom, false), "{nom} sous Linux");
            assert!(
                !nom_representable_localement(nom, true),
                "{nom} sous Windows"
            );
        }
        assert!(nom_representable_localement("rapport.md", true));
        assert!(!nom_sans_evasion(".."));
        assert!(!nom_sans_evasion("a\\b"));
    }

    /// Le dossier se télécharge sans ce qu'il ne peut pas nommer : l'entrée est
    /// rendue une fois, son contenu part avec elle, le reste arrive.
    #[test]
    fn un_dossier_garde_ce_que_le_disque_peut_nommer_et_rend_le_reste() {
        use super::{ecarter_les_noms_impossibles, EntreeDistante};
        let e = |chemin: &str, dossier: bool| EntreeDistante {
            chemin: chemin.to_owned(),
            dossier,
            taille: 1,
            mtime: None,
        };
        let parcours = vec![
            e("Maildir", true),
            e("Maildir/cur:x", true),
            e("a.txt", false),
            e("Maildir/1.M1:2,S", false),
            e("Maildir/cur:x/y", false),
            e("Maildir/propre", false),
        ];
        let (gardees, ecartes) = ecarter_les_noms_impossibles(parcours.clone(), true);
        let noms: Vec<&str> = gardees.iter().map(|g| g.chemin.as_str()).collect();
        assert_eq!(noms, ["Maildir", "a.txt", "Maildir/propre"]);
        assert_eq!(ecartes, ["Maildir/cur:x", "Maildir/1.M1:2,S"]);
        let (tout, rien) = ecarter_les_noms_impossibles(parcours.clone(), false);
        assert_eq!((tout, rien), (parcours, Vec::<String>::new()));
    }

    #[test]
    fn joindre_et_rel_texte_parlent_le_langage_du_distant() {
        assert_eq!(joindre("/srv", "a/b"), "/srv/a/b");
        assert_eq!(joindre("/srv/", "a"), "/srv/a");
        assert_eq!(joindre(".", "a"), "a");
        assert_eq!(rel_texte(std::path::Path::new("a/b/c.txt")), "a/b/c.txt");
        assert_eq!(rel_texte(std::path::Path::new("")), "");
    }
}
