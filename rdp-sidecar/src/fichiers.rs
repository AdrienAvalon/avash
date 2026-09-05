//! Fichiers par le presse-papiers RDP (CLIPRDR, [MS-RDPECLIP] 2.2.5) : la
//! réception des fichiers copiés sur le bureau distant, et l'offre de fichiers
//! du poste au bureau distant.
//!
//! IronRDP porte le protocole (liste de fichiers, verrous, requêtes de contenu
//! par flux) ; ce module tient ce que le protocole ne décide pas : découper un
//! fichier en morceaux demandés à la suite, écrire chaque réponse à sa
//! position, promouvoir le fichier une fois complet, et de l'autre côté
//! parcourir les dossiers offerts et servir les octets demandés. Tout ce qui
//! vient du distant reste une entrée non fiable : les chemins sont déjà
//! assainis par IronRDP (ni absolu, ni `..`), les tailles annoncées ne servent
//! qu'à l'affichage et à borner les requêtes, jamais à allouer.

use anyhow::{Context, Result};
use ironrdp::cliprdr::pdu::{
    ClipboardFileAttributes, FileContentsFlags, FileContentsRequest, FileContentsResponse,
    FileDescriptor,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

/// Taille d'un morceau demandé au distant. Un mégaoctet : assez pour que la
/// latence ne domine pas, assez peu pour que la progression se voie.
pub(crate) const MORCEAU: u32 = 1 << 20;
/// Morceaux en vol par fichier : chaque requête porte son propre `streamId`,
/// les réponses reviennent dans l'ordre que le serveur veut.
pub(crate) const EN_VOL: usize = 4;
/// Plus grande demande qu'on sert au distant d'un coup (16 Mio) : un serveur
/// peut demander `u32::MAX` octets, on ne lit pas cela en mémoire.
pub(crate) const SERVI_MAX: u32 = 16 << 20;
/// Nombre de fichiers qu'une offre peut porter, dossiers parcourus compris.
pub(crate) const FICHIERS_MAX: usize = 10_000;

/// Un fichier annoncé par le distant, tel que l'interface le présente.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct FichierDistant {
    /// Chemin relatif, séparateurs `/`, nom compris.
    pub(crate) chemin: String,
    pub(crate) taille: u64,
    pub(crate) dossier: bool,
}

/// Chemin relatif d'un descripteur, séparateurs `/`.
fn chemin_relatif(d: &FileDescriptor) -> String {
    match d.relative_path.as_deref().filter(|p| !p.is_empty()) {
        Some(p) => format!("{}/{}", p.replace('\\', "/"), d.name),
        None => d.name.clone(),
    }
}

fn est_dossier(d: &FileDescriptor) -> bool {
    d.attributes
        .is_some_and(|a| a.contains(ClipboardFileAttributes::DIRECTORY))
}

/// Ce que l'interface reçoit quand le distant a copié des fichiers.
pub(crate) fn annonce(files: &[FileDescriptor]) -> Vec<FichierDistant> {
    files
        .iter()
        .map(|d| FichierDistant {
            chemin: chemin_relatif(d),
            taille: if est_dossier(d) {
                0
            } else {
                d.file_size.unwrap_or(0)
            },
            dossier: est_dossier(d),
        })
        .collect()
}

/// Le dossier de réception par défaut : celui des téléchargements, sinon le
/// répertoire personnel, sinon le répertoire courant.
///
/// Sous `AVASH_HOME`, tout reste sous ce toit : `dirs::download_dir()` ignore
/// la variable et, sous Windows, rendait le vrai dossier Téléchargements de
/// l'utilisateur, hors du bac à sable des tests (cinquième passage Windows de
/// la suite complète, 05/09/2026).
pub(crate) fn dossier_par_defaut() -> PathBuf {
    if let Some(foyer) = std::env::var_os("AVASH_HOME").filter(|v| !v.is_empty()) {
        let foyer = PathBuf::from(foyer);
        return ["Téléchargements", "Downloads"]
            .iter()
            .map(|n| foyer.join(n))
            .find(|d| d.is_dir())
            .unwrap_or(foyer);
    }
    dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Chemin local d'un fichier reçu : sous `dossier`, avec son chemin relatif.
/// Un nom qui existe déjà prend un suffixe « (2) », « (3) »… plutôt que
/// d'écraser : le poste garde ce qu'il avait.
fn chemin_local(dossier: &Path, d: &FileDescriptor) -> PathBuf {
    let mut p = dossier.to_path_buf();
    if let Some(rel) = d.relative_path.as_deref().filter(|r| !r.is_empty()) {
        for c in rel
            .split('\\')
            .filter(|c| !c.is_empty() && *c != "." && *c != "..")
        {
            p.push(c);
        }
    }
    p.push(&d.name);
    p
}

fn sans_collision(p: &Path) -> PathBuf {
    if !p.exists() {
        return p.to_path_buf();
    }
    let tige = p
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = p
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    for n in 2..10_000u32 {
        let candidat = p.with_file_name(format!("{tige} ({n}){ext}"));
        if !candidat.exists() {
            return candidat;
        }
    }
    p.to_path_buf()
}

/// Un fichier en cours de réception.
struct EnCours {
    index: usize,
    partiel: PathBuf,
    cible: PathBuf,
    fichier: tokio::fs::File,
    taille: u64,
    /// Prochaine position à demander.
    demande: u64,
    /// Octets écrits.
    recu: u64,
    /// Requêtes en vol : `streamId` → (position, longueur demandée).
    en_vol: HashMap<u32, (u64, u32)>,
}

/// L'état d'une réception : les fichiers annoncés, celui qu'on reçoit, ce qui
/// a été écrit. Produit les requêtes à envoyer, consomme les réponses.
pub(crate) struct Reception {
    dossier: PathBuf,
    fichiers: Vec<FileDescriptor>,
    data_id: Option<u32>,
    prochain: usize,
    en_cours: Option<EnCours>,
    prochain_flux: u32,
    fait: u64,
    total: u64,
    termines: usize,
    erreurs: Vec<String>,
}

/// Où en est une réception, pour l'interface.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct Progression {
    pub(crate) fichier: String,
    pub(crate) fait: u64,
    pub(crate) total: u64,
    pub(crate) termines: usize,
    pub(crate) nombre: usize,
}

impl Reception {
    pub(crate) fn nouvelle(
        dossier: PathBuf,
        fichiers: Vec<FileDescriptor>,
        data_id: Option<u32>,
        premier_flux: u32,
    ) -> Self {
        let total = fichiers
            .iter()
            .filter(|d| !est_dossier(d))
            .map(|d| d.file_size.unwrap_or(0))
            .sum();
        Self {
            dossier,
            fichiers,
            data_id,
            prochain: 0,
            en_cours: None,
            prochain_flux: premier_flux,
            fait: 0,
            total,
            termines: 0,
            erreurs: Vec::new(),
        }
    }

    pub(crate) fn dossier(&self) -> &Path {
        &self.dossier
    }

    pub(crate) fn terminee(&self) -> bool {
        self.en_cours.is_none() && self.prochain >= self.fichiers.len()
    }

    pub(crate) fn erreurs(&self) -> &[String] {
        &self.erreurs
    }

    pub(crate) fn progression(&self) -> Progression {
        Progression {
            fichier: self
                .en_cours
                .as_ref()
                .and_then(|e| self.fichiers.get(e.index))
                .map(chemin_relatif)
                .unwrap_or_default(),
            fait: self.fait,
            total: self.total,
            termines: self.termines,
            nombre: self.fichiers.len(),
        }
    }

    /// Ouvre les fichiers suivants jusqu'à en avoir un en réception, et rend
    /// les requêtes à envoyer.
    pub(crate) async fn demarrer(&mut self) -> Vec<FileContentsRequest> {
        while self.en_cours.is_none() && self.prochain < self.fichiers.len() {
            let index = self.prochain;
            self.prochain += 1;
            let d = self.fichiers[index].clone();
            let cible = chemin_local(&self.dossier, &d);
            if est_dossier(&d) {
                if let Err(e) = tokio::fs::create_dir_all(&cible).await {
                    self.erreurs.push(format!("{} : {e}", chemin_relatif(&d)));
                }
                self.termines += 1;
                continue;
            }
            match self.ouvrir(index, &d, cible).await {
                Ok(Some(reqs)) => return reqs,
                Ok(None) => self.termines += 1, // fichier vide, déjà écrit
                Err(e) => {
                    self.erreurs.push(format!("{} : {e:#}", chemin_relatif(&d)));
                    self.termines += 1;
                }
            }
        }
        Vec::new()
    }

    async fn ouvrir(
        &mut self,
        index: usize,
        d: &FileDescriptor,
        cible: PathBuf,
    ) -> Result<Option<Vec<FileContentsRequest>>> {
        if let Some(parent) = cible.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("création de {}", parent.display()))?;
        }
        let cible = sans_collision(&cible);
        let taille = d.file_size.unwrap_or(0);
        if taille == 0 {
            tokio::fs::File::create(&cible)
                .await
                .with_context(|| format!("création de {}", cible.display()))?;
            return Ok(None);
        }
        let mut partiel = cible.as_os_str().to_owned();
        partiel.push(".part");
        let partiel = PathBuf::from(partiel);
        let fichier = tokio::fs::File::create(&partiel)
            .await
            .with_context(|| format!("création de {}", partiel.display()))?;
        self.en_cours = Some(EnCours {
            index,
            partiel,
            cible,
            fichier,
            taille,
            demande: 0,
            recu: 0,
            en_vol: HashMap::new(),
        });
        Ok(Some(self.remplir()))
    }

    /// Remet des requêtes en vol jusqu'à `EN_VOL`, ou jusqu'à la fin du fichier.
    fn remplir(&mut self) -> Vec<FileContentsRequest> {
        let data_id = self.data_id;
        let mut flux_ids = Vec::new();
        let Some(e) = self.en_cours.as_mut() else {
            return Vec::new();
        };
        let mut reqs = Vec::new();
        while e.en_vol.len() < EN_VOL && e.demande < e.taille {
            let longueur =
                u32::try_from((e.taille - e.demande).min(u64::from(MORCEAU))).unwrap_or(MORCEAU);
            flux_ids.push((e.demande, longueur));
            e.demande += u64::from(longueur);
        }
        let index = i32::try_from(e.index).unwrap_or(i32::MAX);
        for (position, longueur) in flux_ids {
            let stream_id = {
                let f = self.prochain_flux;
                self.prochain_flux = self.prochain_flux.wrapping_add(1).max(1);
                f
            };
            if let Some(e) = self.en_cours.as_mut() {
                e.en_vol.insert(stream_id, (position, longueur));
            }
            reqs.push(FileContentsRequest {
                stream_id,
                index,
                flags: FileContentsFlags::RANGE,
                position,
                requested_size: longueur,
                data_id,
            });
        }
        reqs
    }

    /// Une réponse du distant (`None` : erreur). Rend les requêtes suivantes ;
    /// quand le fichier est complet, il est promu et le suivant commence.
    pub(crate) async fn recevoir(
        &mut self,
        stream_id: u32,
        donnees: Option<&[u8]>,
    ) -> Vec<FileContentsRequest> {
        let Some(e) = self.en_cours.as_mut() else {
            return Vec::new();
        };
        let Some((position, longueur)) = e.en_vol.remove(&stream_id) else {
            return Vec::new(); // une réponse à une requête qu'on ne suit plus
        };
        let mut echec: Option<String> = None;
        match donnees {
            None => echec = Some("le distant a refusé de servir le fichier".to_owned()),
            Some(d) => {
                // Un serveur ne rend jamais plus que demandé ; s'il le fait, on
                // n'écrit que la fenêtre demandée.
                let d = &d[..d.len().min(longueur as usize)];
                let ecrit = async {
                    e.fichier
                        .seek(std::io::SeekFrom::Start(position))
                        .await
                        .context("positionnement")?;
                    e.fichier.write_all(d).await.context("écriture")?;
                    anyhow::Ok(())
                }
                .await;
                match ecrit {
                    Ok(()) => {
                        e.recu += d.len() as u64;
                        self.fait += d.len() as u64;
                        // Moins que demandé : le fichier a rétréci ou le serveur
                        // coupe court. On ne demandera pas plus loin que ce qui
                        // vient d'arriver.
                        if (d.len() as u64) < u64::from(longueur) {
                            e.taille = e.taille.min(position + d.len() as u64);
                        }
                    }
                    Err(err) => echec = Some(format!("{err:#}")),
                }
            }
        }
        if let Some(raison) = echec {
            let e = self.en_cours.take().expect("en cours");
            let _ = tokio::fs::remove_file(&e.partiel).await;
            self.erreurs.push(format!(
                "{} : {raison}",
                chemin_relatif(&self.fichiers[e.index])
            ));
            self.termines += 1;
            return self.demarrer().await;
        }
        let complet = self
            .en_cours
            .as_ref()
            .is_some_and(|e| e.recu >= e.taille && e.en_vol.is_empty());
        if complet {
            let mut e = self.en_cours.take().expect("en cours");
            let fin = async {
                e.fichier.flush().await.context("vidage")?;
                drop(e.fichier);
                tokio::fs::rename(&e.partiel, &e.cible)
                    .await
                    .with_context(|| format!("renommage vers {}", e.cible.display()))
            }
            .await;
            if let Err(err) = fin {
                let _ = tokio::fs::remove_file(&e.partiel).await;
                self.erreurs.push(format!(
                    "{} : {err:#}",
                    chemin_relatif(&self.fichiers[e.index])
                ));
            }
            self.termines += 1;
            return self.demarrer().await;
        }
        self.remplir()
    }
}

/// Un fichier offert au distant : son chemin sur le poste et ce qu'on en dit.
#[derive(Debug, Clone)]
pub(crate) struct Offre {
    pub(crate) fichiers: Vec<(PathBuf, FileDescriptor)>,
}

/// Heure de modification en FILETIME (centaines de nanosecondes depuis 1601).
fn filetime(m: &std::fs::Metadata) -> Option<u64> {
    let d = m
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    Some((d.as_secs() + 11_644_473_600) * 10_000_000 + u64::from(d.subsec_nanos() / 100))
}

/// Parcourt les chemins donnés (fichiers ou dossiers, récursivement) et
/// construit l'offre : chemins relatifs à la racine choisie, `\` comme
/// séparateur, taille et date pour les fichiers, attribut dossier pour les
/// dossiers. Les liens symboliques ne sont pas suivis : une offre ne doit
/// pas sortir de ce que l'utilisateur a désigné.
pub(crate) async fn preparer_offre(chemins: &[PathBuf]) -> Result<Offre> {
    let mut fichiers = Vec::new();
    for racine in chemins {
        let nom = racine
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.is_empty())
            .with_context(|| format!("chemin sans nom : {}", racine.display()))?;
        let m = tokio::fs::symlink_metadata(racine)
            .await
            .with_context(|| format!("lecture de {}", racine.display()))?;
        if m.is_dir() {
            fichiers.push((
                racine.clone(),
                FileDescriptor::new(nom.clone())
                    .with_attributes(ClipboardFileAttributes::DIRECTORY),
            ));
            let mut pile = vec![(racine.clone(), nom)];
            while let Some((dossier, rel)) = pile.pop() {
                let mut entrees = tokio::fs::read_dir(&dossier)
                    .await
                    .with_context(|| format!("lecture de {}", dossier.display()))?;
                while let Some(e) = entrees.next_entry().await? {
                    anyhow::ensure!(
                        fichiers.len() < FICHIERS_MAX,
                        "plus de {FICHIERS_MAX} fichiers : trop pour un presse-papiers"
                    );
                    let m = e.metadata().await?;
                    let nom = e.file_name().to_string_lossy().into_owned();
                    if m.is_dir() {
                        let rel_enfant = format!("{rel}\\{nom}");
                        fichiers.push((
                            e.path(),
                            FileDescriptor::new(nom)
                                .with_relative_path(rel.clone())
                                .with_attributes(ClipboardFileAttributes::DIRECTORY),
                        ));
                        pile.push((e.path(), rel_enfant));
                    } else if m.is_file() {
                        let mut d = FileDescriptor::new(nom)
                            .with_relative_path(rel.clone())
                            .with_file_size(m.len())
                            .with_attributes(ClipboardFileAttributes::NORMAL);
                        if let Some(t) = filetime(&m) {
                            d = d.with_last_write_time(t);
                        }
                        fichiers.push((e.path(), d));
                    }
                    // Liens et fichiers spéciaux : ignorés.
                }
            }
        } else if m.is_file() {
            let mut d = FileDescriptor::new(nom)
                .with_file_size(m.len())
                .with_attributes(ClipboardFileAttributes::NORMAL);
            if let Some(t) = filetime(&m) {
                d = d.with_last_write_time(t);
            }
            fichiers.push((racine.clone(), d));
        } else {
            anyhow::bail!("{} n'est ni un fichier ni un dossier", racine.display());
        }
    }
    anyhow::ensure!(!fichiers.is_empty(), "aucun fichier à offrir");
    Ok(Offre { fichiers })
}

impl Offre {
    pub(crate) fn descripteurs(&self) -> Vec<FileDescriptor> {
        self.fichiers.iter().map(|(_, d)| d.clone()).collect()
    }

    pub(crate) fn taille_totale(&self) -> u64 {
        self.fichiers
            .iter()
            .map(|(_, d)| d.file_size.unwrap_or(0))
            .sum()
    }

    /// Sert une requête du distant : la taille, ou une plage d'octets.
    pub(crate) async fn servir(&self, req: &FileContentsRequest) -> FileContentsResponse<'static> {
        let erreur = FileContentsResponse::new_error(req.stream_id);
        let Some((chemin, d)) = usize::try_from(req.index)
            .ok()
            .and_then(|i| self.fichiers.get(i))
        else {
            return erreur;
        };
        if est_dossier(d) {
            return erreur;
        }
        if req.flags.contains(FileContentsFlags::SIZE) {
            return match tokio::fs::metadata(chemin).await {
                Ok(m) => FileContentsResponse::new_size_response(req.stream_id, m.len()),
                Err(_) => erreur,
            };
        }
        let longueur = req.requested_size.min(SERVI_MAX) as usize;
        let lu = async {
            let mut f = tokio::fs::File::open(chemin).await?;
            f.seek(std::io::SeekFrom::Start(req.position)).await?;
            let mut tampon = vec![0u8; longueur];
            let mut total = 0;
            while total < longueur {
                let n = f.read(&mut tampon[total..]).await?;
                if n == 0 {
                    break;
                }
                total += n;
            }
            tampon.truncate(total);
            std::io::Result::Ok(tampon)
        }
        .await;
        match lu {
            Ok(donnees) => FileContentsResponse::new_data_response(req.stream_id, donnees),
            Err(_) => erreur,
        }
    }
}

#[cfg(test)]
mod tests {
    /// Sous `AVASH_HOME`, la réception reste sous ce toit, dans le sous-dossier
    /// des téléchargements s'il existe ; sans la variable, le dossier du
    /// système. Sous le verrou partagé avec les autres tests qui la posent.
    #[test]
    fn sous_avash_home_la_reception_reste_sous_le_foyer() {
        let _verrou = crate::empreintes::VERROU_AVASH_HOME
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let foyer = std::env::temp_dir().join(format!("avash-fichiers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&foyer);
        std::fs::create_dir_all(&foyer).unwrap();
        let precedent = std::env::var_os("AVASH_HOME");
        unsafe { std::env::set_var("AVASH_HOME", &foyer) };
        let sous_foyer = super::dossier_par_defaut();
        std::fs::create_dir_all(foyer.join("Downloads")).unwrap();
        let sous_downloads = super::dossier_par_defaut();
        unsafe {
            match precedent {
                Some(v) => std::env::set_var("AVASH_HOME", v),
                None => std::env::remove_var("AVASH_HOME"),
            }
        }
        let _ = std::fs::remove_dir_all(&foyer);
        assert_eq!(sous_foyer, foyer);
        assert_eq!(sous_downloads, foyer.join("Downloads"));
    }

    use super::{annonce, chemin_local, preparer_offre, Reception, MORCEAU};
    use ironrdp::cliprdr::pdu::{
        ClipboardFileAttributes, FileContentsFlags, FileContentsRequest, FileDescriptor,
    };
    use std::path::PathBuf;

    fn temp(nom: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("avash-fichiers-{}-{nom}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// Un « distant » en mémoire : sert les plages demandées depuis des octets
    /// connus, dans l'ordre inverse pour éprouver l'écriture à la position.
    async fn jouer(r: &mut Reception, contenus: &[Vec<u8>]) {
        let mut reqs = r.demarrer().await;
        while !reqs.is_empty() {
            reqs.reverse();
            let mut suivantes = Vec::new();
            for req in reqs {
                let c = &contenus[usize::try_from(req.index).unwrap()];
                let debut = usize::try_from(req.position).unwrap().min(c.len());
                let fin = (debut + req.requested_size as usize).min(c.len());
                suivantes.extend(r.recevoir(req.stream_id, Some(&c[debut..fin])).await);
            }
            reqs = suivantes;
        }
    }

    fn fichier(nom: &str, taille: u64) -> FileDescriptor {
        FileDescriptor::new(nom)
            .with_file_size(taille)
            .with_attributes(ClipboardFileAttributes::NORMAL)
    }

    #[tokio::test]
    async fn un_fichier_de_plusieurs_morceaux_arrive_entier_meme_dans_le_desordre() {
        let d = temp("morceaux");
        let taille = u64::from(MORCEAU) * 2 + 12_345;
        let contenu: Vec<u8> = (0..taille).map(|i| (i % 251) as u8).collect();
        let mut r = Reception::nouvelle(d.clone(), vec![fichier("gros.bin", taille)], Some(7), 1);
        jouer(&mut r, std::slice::from_ref(&contenu)).await;
        assert!(r.terminee());
        assert!(r.erreurs().is_empty(), "{:?}", r.erreurs());
        assert_eq!(std::fs::read(d.join("gros.bin")).unwrap(), contenu);
        assert!(
            !d.join("gros.bin.part").exists(),
            "le .part doit être promu"
        );
        let p = r.progression();
        assert_eq!(
            (p.fait, p.total, p.termines, p.nombre),
            (taille, taille, 1, 1)
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Les requêtes portent le verrou du presse-papiers et des flux distincts,
    /// et ne dépassent jamais la fin du fichier.
    #[tokio::test]
    async fn les_requetes_sont_bornees_et_portent_le_verrou() {
        let d = temp("requetes");
        let taille = u64::from(MORCEAU) + 1;
        let mut r = Reception::nouvelle(d.clone(), vec![fichier("f", taille)], Some(42), 100);
        let reqs = r.demarrer().await;
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[0].requested_size, MORCEAU);
        assert_eq!(
            (reqs[1].position, reqs[1].requested_size),
            (u64::from(MORCEAU), 1)
        );
        assert!(reqs
            .iter()
            .all(|q| q.data_id == Some(42) && q.flags == FileContentsFlags::RANGE));
        assert_ne!(reqs[0].stream_id, reqs[1].stream_id);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Un dossier annoncé est créé, ses fichiers vont dedans, et un nom déjà
    /// pris reçoit un suffixe plutôt que d'écraser.
    #[tokio::test]
    async fn les_dossiers_sont_recrees_et_rien_n_est_ecrase() {
        let d = temp("dossiers");
        std::fs::write(d.join("a.txt"), b"ancien").unwrap();
        let fichiers = vec![
            FileDescriptor::new("sous").with_attributes(ClipboardFileAttributes::DIRECTORY),
            fichier("b.txt", 3).with_relative_path("sous"),
            fichier("a.txt", 5),
            fichier("vide", 0),
        ];
        let mut r = Reception::nouvelle(d.clone(), fichiers, None, 1);
        jouer(
            &mut r,
            &[vec![], b"bcd".to_vec(), b"neuf!".to_vec(), vec![]],
        )
        .await;
        assert!(r.terminee() && r.erreurs().is_empty(), "{:?}", r.erreurs());
        assert_eq!(std::fs::read(d.join("sous").join("b.txt")).unwrap(), b"bcd");
        assert_eq!(std::fs::read(d.join("a.txt")).unwrap(), b"ancien");
        assert_eq!(std::fs::read(d.join("a (2).txt")).unwrap(), b"neuf!");
        assert_eq!(std::fs::metadata(d.join("vide")).unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Un refus du distant sur un fichier ne perd pas les autres, et ne laisse
    /// pas de `.part`.
    #[tokio::test]
    async fn un_refus_saute_le_fichier_et_continue() {
        let d = temp("refus");
        let mut r = Reception::nouvelle(d.clone(), vec![fichier("x", 4), fichier("y", 2)], None, 1);
        let reqs = r.demarrer().await;
        let mut suite = r.recevoir(reqs[0].stream_id, None).await;
        assert_eq!(suite.len(), 1, "le fichier suivant démarre");
        let q = suite.remove(0);
        let fin = r.recevoir(q.stream_id, Some(b"ok")).await;
        assert!(fin.is_empty() && r.terminee());
        assert_eq!(r.erreurs().len(), 1);
        assert!(!d.join("x").exists() && !d.join("x.part").exists());
        assert_eq!(std::fs::read(d.join("y")).unwrap(), b"ok");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Un serveur qui rend moins que demandé termine le fichier là : pas
    /// d'attente sans fin sur des octets qui ne viendront pas.
    #[tokio::test]
    async fn une_reponse_courte_termine_le_fichier() {
        let d = temp("court");
        let mut r = Reception::nouvelle(d.clone(), vec![fichier("f", 10)], None, 1);
        let reqs = r.demarrer().await;
        let suite = r.recevoir(reqs[0].stream_id, Some(b"abc")).await;
        assert!(suite.is_empty() && r.terminee());
        assert_eq!(std::fs::read(d.join("f")).unwrap(), b"abc");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Les chemins viennent d'IronRDP déjà assainis ; on ne remonte jamais
    /// au-dessus du dossier de réception, même si un composant l'essayait.
    #[test]
    fn le_chemin_local_reste_sous_le_dossier() {
        let d = FileDescriptor::new("f.txt").with_relative_path("a\\..\\b");
        assert_eq!(
            chemin_local(std::path::Path::new("/r"), &d),
            PathBuf::from("/r/a/b/f.txt")
        );
        let plat = FileDescriptor::new("seul");
        assert_eq!(
            chemin_local(std::path::Path::new("/r"), &plat),
            PathBuf::from("/r/seul")
        );
    }

    #[test]
    fn l_annonce_dit_les_chemins_et_les_tailles() {
        let a = annonce(&[
            FileDescriptor::new("d").with_attributes(ClipboardFileAttributes::DIRECTORY),
            fichier("f", 9).with_relative_path("d\\e"),
        ]);
        assert_eq!(a[0].chemin, "d");
        assert!(a[0].dossier && a[0].taille == 0);
        assert_eq!(
            (a[1].chemin.as_str(), a[1].taille, a[1].dossier),
            ("d/e/f", 9, false)
        );
    }

    /// L'offre parcourt les dossiers et sert les plages ; une plage au-delà
    /// du fichier rend ce qui reste, un index inconnu une erreur.
    #[tokio::test]
    async fn une_offre_parcourt_les_dossiers_et_sert_les_plages() {
        let d = temp("offre");
        std::fs::create_dir_all(d.join("doc").join("sous")).unwrap();
        std::fs::write(d.join("doc").join("a.txt"), b"hello").unwrap();
        std::fs::write(d.join("doc").join("sous").join("b.txt"), b"world!").unwrap();
        std::fs::write(d.join("seul.bin"), b"xyz").unwrap();
        let offre = preparer_offre(&[d.join("doc"), d.join("seul.bin")])
            .await
            .unwrap();
        let noms: Vec<String> = offre
            .descripteurs()
            .iter()
            .map(|f| match &f.relative_path {
                Some(r) => format!("{r}\\{}", f.name),
                None => f.name.clone(),
            })
            .collect();
        assert!(noms.contains(&"doc".to_owned()));
        assert!(noms.contains(&"doc\\a.txt".to_owned()));
        assert!(noms.contains(&"doc\\sous\\b.txt".to_owned()));
        assert!(noms.contains(&"seul.bin".to_owned()));
        assert_eq!(offre.taille_totale(), 5 + 6 + 3);
        let i = noms.iter().position(|n| n == "doc\\sous\\b.txt").unwrap();
        let req = |flags, position, requested_size| FileContentsRequest {
            stream_id: 9,
            index: i32::try_from(i).unwrap(),
            flags,
            position,
            requested_size,
            data_id: None,
        };
        let taille = offre.servir(&req(FileContentsFlags::SIZE, 0, 8)).await;
        assert_eq!(taille.data_as_size().unwrap(), 6);
        let plage = offre.servir(&req(FileContentsFlags::RANGE, 2, 100)).await;
        assert_eq!(plage.data(), b"rld!");
        let dossier = offre
            .servir(&FileContentsRequest {
                index: 0,
                ..req(FileContentsFlags::RANGE, 0, 1)
            })
            .await;
        assert!(dossier.is_error(), "un dossier n'a pas de contenu");
        let inconnu = offre
            .servir(&FileContentsRequest {
                index: 99,
                ..req(FileContentsFlags::RANGE, 0, 1)
            })
            .await;
        assert!(inconnu.is_error());
        let _ = std::fs::remove_dir_all(&d);
    }
}
