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
}

impl SftpHandle {
    /// Ouvre le sous-système SFTP sur une session (consommée, gardée vivante).
    pub async fn open(mut session: AvashSession) -> Result<Self> {
        let sftp = Self::sous_systeme(&mut session).await?;
        Ok(Self {
            sftp,
            _session: Some(session),
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
        })
    }

    async fn sous_systeme(session: &mut AvashSession) -> Result<SftpSession> {
        let channel = session.open_sftp_channel().await?;
        SftpSession::new(channel.into_stream())
            .await
            .context("Ouverture sous-système SFTP")
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
}

/// Nombre d'entrées qu'un parcours accepte : un serveur peut annoncer une
/// arborescence sans fin (lien qui boucle), on s'arrête et on le dit.
const ENTREES_MAX: usize = 100_000;

/// Message de l'erreur d'annulation, que l'interface reconnaît.
pub const ANNULE: &str = "Transfert annulé.";

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
    fn ecrire(&self, chemin: &Path) {
        if let Ok(json) = serde_json::to_vec(self) {
            let _ = crate::ecrire_atomiquement(chemin, &json);
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
                anyhow::ensure!(
                    nom_d_entree_sur(&e.name),
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
    /// fichier, chacun repris s'il l'avait été à moitié.
    pub async fn download_dir_with(
        &self,
        remote_dir: &str,
        local: &Path,
        annulation: Option<&Annulation>,
        mut progress: impl FnMut(Avancement),
    ) -> Result<u64> {
        let entrees = self.parcourir(remote_dir).await?;
        let total: u64 = entrees.iter().map(|e| e.taille).sum();
        let nombre = entrees.iter().filter(|e| !e.dossier).count();
        tokio::fs::create_dir_all(local)
            .await
            .with_context(|| format!("Création de {}", local.display()))?;
        let mut fait = 0u64;
        let mut termines = 0usize;
        for e in &entrees {
            verifier(annulation)?;
            let cible = local.join(e.chemin.replace('/', std::path::MAIN_SEPARATOR_STR));
            if e.dossier {
                tokio::fs::create_dir_all(&cible)
                    .await
                    .with_context(|| format!("Création de {}", cible.display()))?;
                continue;
            }
            let base = fait;
            let nom = e.chemin.clone();
            let n = self
                .download_reprise(
                    &joindre(remote_dir, &e.chemin),
                    &cible,
                    annulation,
                    |f, _| {
                        progress(Avancement {
                            fichier: nom.clone(),
                            fait: base + f,
                            total,
                            termines,
                            nombre,
                        });
                    },
                )
                .await
                .with_context(|| e.chemin.clone())?;
            fait += n;
            termines += 1;
            progress(Avancement {
                fichier: e.chemin.clone(),
                fait,
                total,
                termines,
                nombre,
            });
        }
        Ok(fait)
    }

    /// Téléverse un dossier local entier sous `remote_dir` (créé), fichier par
    /// fichier, chacun repris s'il l'avait été à moitié.
    pub async fn upload_dir_with(
        &self,
        local: &Path,
        remote_dir: &str,
        annulation: Option<&Annulation>,
        mut progress: impl FnMut(Avancement),
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
        let nombre = fichiers.len();
        for d in &dossiers {
            verifier(annulation)?;
            let chemin = joindre(remote_dir, &rel_texte(d));
            // Un dossier déjà présent n'est pas une erreur : on continue dedans.
            if self.sftp.metadata(&chemin).await.is_err() {
                self.mkdir(&chemin).await?;
            }
        }
        let mut fait = 0u64;
        for (termines, (rel, _)) in fichiers.iter().enumerate() {
            verifier(annulation)?;
            let nom = rel_texte(rel);
            let base = fait;
            let nom_prog = nom.clone();
            let n = self
                .upload_reprise(
                    &local.join(rel),
                    &joindre(remote_dir, &nom),
                    // Fusion volontaire : un fichier déjà présent est remplacé,
                    // comme une resynchronisation de dossier l'attend.
                    false,
                    annulation,
                    |f, _| {
                        progress(Avancement {
                            fichier: nom_prog.clone(),
                            fait: base + f,
                            total,
                            termines,
                            nombre,
                        });
                    },
                )
                .await
                .with_context(|| nom.clone())?;
            fait += n;
            progress(Avancement {
                fichier: nom,
                fait,
                total,
                termines: termines + 1,
                nombre,
            });
        }
        Ok(fait)
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
        mut progress: impl FnMut(u64, u64),
    ) -> Result<u64> {
        let meta = self.sftp.metadata(remote).await.ok();
        let total = meta
            .as_ref()
            .map_or(0, russh_sftp::protocol::FileAttributes::len);
        let mtime = meta.as_ref().and_then(|m| m.mtime).map(u64::from);
        if total <= (2 * CHUNK) as u64 {
            verifier(annulation)?;
            return self.download_with(remote, local, progress).await;
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
        tokio::fs::rename(&partiel, local)
            .await
            .with_context(|| format!("Renommage vers {}", local.display()))?;
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
            let n = distant
                .read(&mut buf[..vise])
                .await
                .context("Lecture distante")?;
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
        let distant = self.sftp.metadata(remote).await.ok();
        let distant_len = distant
            .as_ref()
            .map_or(0, russh_sftp::protocol::FileAttributes::len);
        let deja = Reprise::lire(carte)
            .filter(|r| r.vaut_pour(total, mtime) && r.vise(remote))
            .and_then(|r| r.faites.first().map(|(_, fin)| *fin))
            .filter(|fin| *fin <= distant_len && *fin < total)
            .unwrap_or(0);
        if refuser_ecrasement && deja == 0 && distant.is_some() {
            anyhow::bail!(
                "Le fichier distant « {remote} » existe déjà : envoi refusé pour ne pas l'écraser."
            );
        }
        Ok(deja)
    }

    /// Téléversement d'un fichier, annulable, repris là où il s'était arrêté.
    ///
    /// Une seule écriture après l'autre, **pas** de bandes : `File` de
    /// russh-sftp pipeline déjà huit écritures, et la mesure (voir la feuille
    /// de route, axe 3) a tranché contre huit descripteurs en parallèle,
    /// quatre fois plus lents en réseau local pour 1,2 × à 40 ms d'aller-retour.
    ///
    /// La reprise : une carte `<local>.envoi.reprise` note, à chaque point de
    /// contrôle (toutes les `POINT_DE_CONTROLE` octets, après un vidage), ce
    /// qui est sûrement écrit chez le distant, avec la taille et la date du
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
        /// Entre deux points de contrôle : assez rare pour ne pas ralentir,
        /// assez fréquent pour qu'une coupure ne coûte que quelques secondes.
        const POINT_DE_CONTROLE: u64 = 4 * 1024 * 1024;

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
                if fait - dernier_point >= POINT_DE_CONTROLE {
                    // Vidé, donc sûrement chez le distant : la carte peut le dire.
                    distant.flush().await.context("Vidage distant")?;
                    dernier_point = fait;
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

    /// Copie un fichier de ce serveur vers un autre, sans rien écrire sur le
    /// poste : les octets ne font que le traverser, par bandes, chaque bande
    /// avec son descripteur de lecture ici et son descripteur d'écriture
    /// là-bas.
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
        // Trouvé par l'audit du 7 septembre 2026 : `metadata` échouant (lien
        // symbolique cassé, fichier disparu) donnait `total = 0` ; la cible était
        // alors créée vide et la copie annoncée réussie. Et une source lisible par
        // `stat` mais pas par `open` (droits root) tronquait une cible existante
        // avant d'échouer. On lit d'abord les attributs — erreur propagée — et
        // l'on s'assure que la source S'OUVRE, AVANT de créer ou tronquer la cible.
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
        cible
            .sftp
            .create(remote_cible)
            .await
            .with_context(|| format!("Création distant {remote_cible}"))?
            .shutdown()
            .await
            .context("Fermeture distante")?;
        if total == 0 {
            return Ok(0);
        }
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
                        .open_with_flags(remote_cible, OpenFlags::WRITE)
                        .await
                        .with_context(|| format!("Ouverture distant {remote_cible}"))?;
                    dest.seek(std::io::SeekFrom::Start(debut))
                        .await
                        .context("Positionnement cible")?;
                    let mut buf = vec![0u8; CHUNK];
                    let mut reste = fin - debut;
                    while reste > 0 {
                        verifier(annulation)?;
                        let vise = usize::try_from(reste.min(CHUNK as u64)).unwrap_or(CHUNK);
                        let n = source
                            .read(&mut buf[..vise])
                            .await
                            .context("Lecture source")?;
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
        for issue in issues {
            issue?;
        }
        anyhow::ensure!(
            fait == total,
            "Copie incomplète : {fait} octets sur {total} (le fichier source a changé pendant la copie)."
        );
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
        mut progress: impl FnMut(Avancement),
    ) -> Result<u64> {
        let entrees = self.parcourir(remote_dir).await?;
        let total: u64 = entrees.iter().map(|e| e.taille).sum();
        let nombre = entrees.iter().filter(|e| !e.dossier).count();
        if cible.sftp.metadata(remote_cible).await.is_err() {
            cible.mkdir(remote_cible).await?;
        }
        let mut fait = 0u64;
        let mut termines = 0usize;
        for e in &entrees {
            verifier(annulation)?;
            let chez_cible = joindre(remote_cible, &e.chemin);
            if e.dossier {
                if cible.sftp.metadata(&chez_cible).await.is_err() {
                    cible.mkdir(&chez_cible).await?;
                }
                continue;
            }
            let base = fait;
            let nom = e.chemin.clone();
            let n = self
                .relayer_vers(
                    &joindre(remote_dir, &e.chemin),
                    cible,
                    &chez_cible,
                    // Fusion volontaire dans l'arborescence cible déjà là.
                    false,
                    annulation,
                    |f, _| {
                        progress(Avancement {
                            fichier: nom.clone(),
                            fait: base + f,
                            total,
                            termines,
                            nombre,
                        });
                    },
                )
                .await
                .with_context(|| e.chemin.clone())?;
            fait += n;
            termines += 1;
            progress(Avancement {
                fichier: e.chemin.clone(),
                fait,
                total,
                termines,
                nombre,
            });
        }
        Ok(fait)
    }
}

/// Un nom d'entrée qu'on accepte de recopier tel quel sous un dossier :
/// un seul composant, sans séparateur ni octet nul, ni `.` ni `..`. Un
/// serveur SFTP peut annoncer n'importe quoi ; c'est ici que ça s'arrête.
fn nom_d_entree_sur(nom: &str) -> bool {
    !nom.is_empty()
        && nom != "."
        && nom != ".."
        && !nom.contains(['/', '\\', '\0'])
        && !(cfg!(windows) && nom.contains(':'))
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
    use super::{bandes, joindre, rel_texte, Reprise, CHUNK};

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
        assert!(bandes(0).is_empty());
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

    #[test]
    fn joindre_et_rel_texte_parlent_le_langage_du_distant() {
        assert_eq!(joindre("/srv", "a/b"), "/srv/a/b");
        assert_eq!(joindre("/srv/", "a"), "/srv/a");
        assert_eq!(joindre(".", "a"), "a");
        assert_eq!(rel_texte(std::path::Path::new("a/b/c.txt")), "a/b/c.txt");
        assert_eq!(rel_texte(std::path::Path::new("")), "");
    }
}
