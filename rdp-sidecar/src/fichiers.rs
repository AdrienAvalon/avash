//! Fichiers par le presse-papiers RDP (CLIPRDR, [MS-RDPECLIP] 2.2.5) : la
//! réception des fichiers copiés sur le bureau distant, et l'offre de fichiers
//! du poste au bureau distant.
//!
//! IronRDP porte le protocole (liste de fichiers, verrous, requêtes de contenu
//! par flux) ; ce module tient ce que le protocole ne décide pas : découper un
//! fichier en morceaux demandés à la suite, écrire chaque réponse à sa
//! position, promouvoir le fichier une fois complet, et de l'autre côté
//! parcourir les dossiers offerts et servir les octets demandés. Tout ce qui
//! vient du distant reste une entrée non fiable : IronRDP retire `..` et les
//! préfixes absolus en tête, mais laisse passer un nom sans séparateur portant
//! une lettre de lecteur (« C:evil.exe »), un nom de périphérique réservé ou un
//! nom truqué par un contrôle de direction Unicode qui inverse l'extension à
//! l'affichage, si bien qu'on revalide chaque composant nous-mêmes (voir
//! [`composant_sur`]) et qu'on rend lisible ce qu'on affiche (voir
//! [`lisible`]) ;
//! les tailles annoncées ne servent qu'à l'affichage et à borner les requêtes,
//! jamais à allouer.

use anyhow::{Context, Result};
use ironrdp::cliprdr::pdu::{
    ClipboardFileAttributes, FileContentsFlags, FileContentsRequest, FileContentsResponse,
    FileDescriptor,
};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
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

/// Un caractère qui ment sur ce qu'on lit : contrôles de direction
/// bidirectionnelle (ils réordonnent le texte qui suit), caractères invisibles
/// sans chasse (un nom peut alors se lire comme un autre), et caractères qui
/// coupent une ligne d'affichage en deux.
///
/// Trouvé par l'audit du 9 septembre 2026 : voir [`composant_sur`].
///
/// Les séparateurs de ligne et de paragraphe (U+2028, U+2029) comptent autant
/// que les contrôles C0/C1 : le front est une webview, et dans un nœud texte
/// HTML un « \n » est replié en espace par le traitement des blancs alors que
/// ces deux-là restent des sauts de ligne forcés. Une première version du
/// filtre ne prenait que C0/C1 et laissait donc passer le seul cas qui coupe
/// vraiment le badge des fichiers en cours et la notification d'erreurs
/// (relecture du 9 septembre 2026).
///
/// L'antiliant et le liant sans chasse (U+200C, U+200D) restent permis : ils
/// ne réordonnent rien, l'écriture persane repose sur l'antiliant et les
/// séquences emoji sur le liant. Les sélecteurs de variante restent permis
/// pour la même raison (U+FE0F fait l'emoji).
fn caractere_trompeur(c: char) -> bool {
    matches!(c,
        '\u{0}'..='\u{1f}'
            | '\u{7f}'..='\u{9f}'
            | '\u{ad}'
            | '\u{61c}'
            | '\u{115f}'
            | '\u{1160}'
            | '\u{180e}'
            | '\u{200b}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{3164}'
            | '\u{feff}'
            | '\u{ffa0}'
            | '\u{fff9}'..='\u{fffb}')
}

/// Le même nom, mais lisible : chaque caractère trompeur devient U+FFFD.
///
/// L'interface voit le nom annoncé *avant* que l'utilisateur accepte la
/// réception, et le voit encore dans les messages d'erreur du bilan : sans
/// cela, un « malware\u{202E}txt.exe » s'affichait « malwareexe.txt » et le
/// texte français qui l'entoure partait à l'envers avec lui.
fn lisible(nom: &str) -> String {
    if nom.chars().any(caractere_trompeur) {
        nom.chars()
            .map(|c| if caractere_trompeur(c) { '\u{fffd}' } else { c })
            .collect()
    } else {
        nom.to_owned()
    }
}

/// Chemin relatif d'un descripteur, séparateurs `/`, rendu lisible pour
/// l'affichage : ce chemin ne sert jamais à ouvrir un fichier (c'est
/// [`chemin_local`] qui construit la cible), seulement à dire à l'utilisateur
/// ce que le distant annonce.
fn chemin_relatif(d: &FileDescriptor) -> String {
    let brut = match d.relative_path.as_deref().filter(|p| !p.is_empty()) {
        Some(p) => format!("{}/{}", p.replace('\\', "/"), d.name),
        None => d.name.clone(),
    };
    lisible(&brut)
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

/// Un composant de nom sûr : un unique [`Component::Normal`], sans deux-points
/// (préfixe de disque « C: » ou flux ADS sous Windows), sans séparateur ni
/// octet nul, sans caractère trompeur ([`caractere_trompeur`]), et qui n'est
/// pas un nom de périphérique réservé Windows (CON, NUL, LPT1…, que
/// `sanitize_file_path` ne filtre pas).
///
/// Trouvé par l'audit du 9 septembre 2026 : rien ne filtrait les contrôles de
/// direction Unicode, ni ici, ni dans `sanitize_file_path` d'IronRDP, ni au
/// front. Un serveur annonçant « malware\u{202E}txt.exe » faisait lire
/// « malwareexe.txt » à l'utilisateur au moment d'accepter, puis dans son
/// gestionnaire de fichiers (mêmes règles de rendu bidirectionnel partout) :
/// il croyait ouvrir un fichier texte et lançait un exécutable. On refuse le
/// fichier plutôt que de réécrire son nom : le nom accepté est alors celui que
/// l'utilisateur a vu.
///
/// Trouvé par l'audit du 7 septembre 2026 : `sanitize_file_path` d'IronRDP rend
/// « C:evil.exe » tel quel (aucun séparateur, chemin de sortie inchangé) et ne
/// retire une lettre de lecteur qu'en tête de chemin ; sous Windows
/// `PathBuf::push("C:evil.exe")` remplace le chemin construit par un chemin
/// relatif au disque courant (donc au cwd du sidecar), hors du dossier de
/// réception. On valide donc chaque composant nous-mêmes.
fn composant_sur(c: &str) -> bool {
    if c.contains([':', '/', '\\', '\0']) || ironrdp::cliprdr::is_windows_device_name(c) {
        return false;
    }
    if c.chars().any(caractere_trompeur) {
        return false;
    }
    let mut composants = Path::new(c).components();
    matches!(
        (composants.next(), composants.next()),
        (Some(Component::Normal(_)), None)
    )
}

/// Chemin local d'un fichier reçu : sous `dossier`, avec son chemin relatif.
/// Un nom qui existe déjà prend un suffixe « (2) », « (3) »… plutôt que
/// d'écraser : le poste garde ce qu'il avait.
///
/// Chaque composant (parties du chemin relatif et nom) doit être sûr
/// ([`composant_sur`]) ; sinon `None`, et l'appelant écarte le fichier avec une
/// entrée dans `erreurs` plutôt que d'écrire hors du dossier.
fn chemin_local(dossier: &Path, d: &FileDescriptor) -> Option<PathBuf> {
    let mut p = dossier.to_path_buf();
    if let Some(rel) = d.relative_path.as_deref().filter(|r| !r.is_empty()) {
        // Découpe sur `/` et `\` : IronRDP joint avec `\`, mais un composant
        // piégé pourrait porter l'autre séparateur (défense en profondeur).
        for c in rel
            .split(['\\', '/'])
            .filter(|c| !c.is_empty() && *c != "." && *c != "..")
        {
            if !composant_sur(c) {
                return None;
            }
            p.push(c);
        }
    }
    if !composant_sur(&d.name) {
        return None;
    }
    p.push(&d.name);
    // Défense en profondeur : après le join, la cible reste sous le dossier de
    // réception (les composants validés le garantissent déjà). Cette vérité
    // n'est que lexicale : c'est [`creer_sous`] qui empêche un lien symbolique
    // préexistant de faire sortir la destination du dossier.
    p.starts_with(dossier).then_some(p)
}

/// Un nom déjà pris sur le disque, lien symbolique compris (même pendouillant).
///
/// Trouvé par l'audit du 9 septembre 2026 : `exists()` suit les liens, donc un
/// lien dont la cible manque passait pour un nom libre et la création d'un
/// fichier vide écrivait au bout du lien, hors du dossier de réception.
fn occupe(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}

/// Crée sous `dossier` chaque composant manquant de `chemin` sans jamais suivre
/// un lien : un composant qui existe déjà mais n'est pas un vrai dossier (lien
/// symbolique en tête) fait échouer la préparation.
///
/// Trouvé par l'audit du 9 septembre 2026 : `create_dir_all` traverse sans
/// broncher un sous-dossier qui est en réalité un lien, si bien qu'un
/// `Téléchargements/partage -> ~/.ssh` posé par stow, une synchro nuagique ou
/// l'utilisateur lui-même laissait un serveur annonçant `relative_path =
/// "partage"` écrire où pointe le lien. Le module RDPDR voisin
/// (`disque.rs::resoudre`) se défendait déjà ainsi ; la réception CLIPRDR non.
async fn creer_sous(dossier: &Path, chemin: &Path) -> Result<()> {
    // Le dossier de réception vient du poste (défaut ou choix de l'utilisateur),
    // pas du distant : on le crée d'un bloc s'il manque, comme le faisait le
    // `create_dir_all` d'avant. Seuls les composants annoncés par le serveur
    // descendent ensuite un à un.
    tokio::fs::create_dir_all(dossier)
        .await
        .with_context(|| format!("création de {}", dossier.display()))?;
    let reste = chemin.strip_prefix(dossier).map_err(|_| {
        anyhow::anyhow!(
            "{} n'est pas sous le dossier de réception",
            chemin.display()
        )
    })?;
    let mut courant = dossier.to_path_buf();
    for c in reste.components() {
        courant.push(c);
        match tokio::fs::symlink_metadata(&courant).await {
            Ok(m) if m.is_dir() => continue,
            Ok(m) if m.is_symlink() => {
                anyhow::bail!("{} est un lien symbolique", courant.display())
            }
            Ok(_) => anyhow::bail!("{} n'est pas un dossier", courant.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("lecture de {}", courant.display())),
        }
        match tokio::fs::create_dir(&courant).await {
            Ok(()) => {}
            // Course avec un autre écrivain : on repasse par la même règle,
            // c'est le lien qu'on refuse, pas la simultanéité.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let vrai_dossier = tokio::fs::symlink_metadata(&courant)
                    .await
                    .is_ok_and(|m| m.is_dir());
                if !vrai_dossier {
                    anyhow::bail!("{} n'est pas un dossier", courant.display());
                }
            }
            Err(e) => return Err(e).with_context(|| format!("création de {}", courant.display())),
        }
    }
    Ok(())
}

fn sans_collision(p: &Path) -> PathBuf {
    if !occupe(p) {
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
        if !occupe(&candidat) {
            return candidat;
        }
    }
    p.to_path_buf()
}

/// Ouvre un fichier de travail neuf sans jamais tronquer un fichier existant :
/// `create_new` sur `base`, puis sur des noms dérivés « (2) », « (3) »… tant
/// qu'un fichier occupe le nom. Rend le chemin retenu et le fichier ouvert.
///
/// Trouvé par l'audit du 7 septembre 2026 : voir l'appelant. `create_new` ferme
/// aussi la course entre le test d'existence et l'ouverture.
async fn ouvrir_travail(base: &Path) -> std::io::Result<(PathBuf, tokio::fs::File)> {
    let tige = base
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut chemin = base.to_path_buf();
    let mut n = 2u32;
    loop {
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&chemin)
            .await
        {
            Ok(f) => return Ok((chemin, f)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && n < 10_000 => {
                chemin = base.with_file_name(format!("{tige} ({n}).part"));
                n += 1;
            }
            Err(e) => return Err(e),
        }
    }
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
    /// `streamId` d'une requête FILECONTENTS_SIZE en attente, quand le
    /// descripteur n'a pas donné la taille (FD_FILESIZE absent) ; sinon `None`.
    flux_taille: Option<u32>,
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
            let Some(cible) = chemin_local(&self.dossier, &d) else {
                // Nom refusé (préfixe de disque, séparateur, périphérique
                // réservé, caractère trompeur) : on n'écrit rien et on signale
                // le fichier.
                self.erreurs
                    .push(format!("{} : nom de fichier refusé", chemin_relatif(&d)));
                self.termines += 1;
                continue;
            };
            if est_dossier(&d) {
                if let Err(e) = creer_sous(&self.dossier, &cible).await {
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
            creer_sous(&self.dossier, parent).await?;
        }
        let cible = sans_collision(&cible);
        // On distingue `Some(0)` (le distant affirme un fichier vide, on le
        // crée tel quel) de `None` (FD_FILESIZE absent : la taille est inconnue,
        // MS-RDPECLIP 2.2.5.2.3.1). Trouvé par l'audit du 7 septembre 2026 :
        // `file_size.unwrap_or(0)` confondait les deux, si bien qu'un serveur
        // qui n'annonce pas la taille faisait recevoir des fichiers de 0 octet
        // comptés « réussis » (bilan « 0 erreur »). Pour `None`, on demande
        // d'abord la taille par FILECONTENTS_SIZE (plus bas).
        if d.file_size == Some(0) {
            tokio::fs::File::create(&cible)
                .await
                .with_context(|| format!("création de {}", cible.display()))?;
            return Ok(None);
        }
        let mut base = cible.as_os_str().to_owned();
        base.push(".part");
        let base = PathBuf::from(base);
        // Fichier de travail : on n'utilise pas `File::create`, qui tronque un
        // fichier existant. Trouvé par l'audit du 7 septembre 2026 : le nom de
        // la cible vient du serveur et le dossier par défaut est celui des
        // téléchargements, où Firefox écrit son téléchargement en cours sous
        // exactement « <nom>.part » et ne le renomme qu'à la fin ; une
        // coïncidence de nom aurait tronqué ce téléchargement en silence (perte
        // de données, promesse « sans écraser » de SECURITY.md). On ouvre en
        // `create_new` (jamais de troncature) et, si le « .part » est déjà pris,
        // on se replie sur un nom de travail dérivé.
        let (partiel, fichier) = ouvrir_travail(&base)
            .await
            .with_context(|| format!("création de {}", base.display()))?;
        self.en_cours = Some(EnCours {
            index,
            partiel,
            cible,
            fichier,
            taille: d.file_size.unwrap_or(0),
            demande: 0,
            recu: 0,
            en_vol: HashMap::new(),
            flux_taille: None,
        });
        // Taille connue (> 0) : on demande les plages. Taille inconnue (`None`) :
        // on demande d'abord la taille, la réponse SIZE fixera `taille`.
        if d.file_size.is_some() {
            Ok(Some(self.remplir()))
        } else {
            Ok(Some(self.demander_taille()))
        }
    }

    /// Émet une requête FILECONTENTS_SIZE pour le fichier en cours dont le
    /// descripteur n'a pas donné la taille (FD_FILESIZE absent). La réponse
    /// (8 octets, taille en petit-boutien, MS-RDPECLIP 2.2.5.4) fixera `taille`
    /// dans [`Self::recevoir`], puis les plages suivront par [`Self::remplir`].
    fn demander_taille(&mut self) -> Vec<FileContentsRequest> {
        let data_id = self.data_id;
        let stream_id = {
            let f = self.prochain_flux;
            self.prochain_flux = self.prochain_flux.wrapping_add(1).max(1);
            f
        };
        let Some(e) = self.en_cours.as_mut() else {
            return Vec::new();
        };
        e.flux_taille = Some(stream_id);
        let index = i32::try_from(e.index).unwrap_or(i32::MAX);
        vec![FileContentsRequest {
            stream_id,
            index,
            flags: FileContentsFlags::SIZE,
            position: 0,
            requested_size: 8,
            data_id,
        }]
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
        if e.flux_taille == Some(stream_id) {
            // Réponse à la requête FILECONTENTS_SIZE émise pour un descripteur
            // sans FD_FILESIZE : 8 octets, taille en petit-boutien (2.2.5.4).
            e.flux_taille = None;
            let taille = donnees
                .filter(|d| d.len() == 8)
                .map(|d| u64::from_le_bytes(d[..8].try_into().expect("8 octets")));
            let Some(taille) = taille else {
                // Toujours pas de taille : on ne peut pas recevoir ce fichier,
                // et on le signale au lieu de le compter « réussi » (0 octet).
                let e = self.en_cours.take().expect("en cours");
                let _ = tokio::fs::remove_file(&e.partiel).await;
                self.erreurs.push(format!(
                    "{} : taille inconnue",
                    chemin_relatif(&self.fichiers[e.index])
                ));
                self.termines += 1;
                return self.demarrer().await;
            };
            e.taille = taille;
            // La taille annoncée manquait au total (comptée 0) : on la rattrape
            // pour que la progression n'affiche plus 0 pour ce fichier.
            self.total += taille;
            if taille == 0 {
                // Le distant confirme un fichier vide : le `.part` (vide) est
                // promu tel quel.
                return self.promouvoir_et_suivre().await;
            }
            return self.remplir();
        }
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
            return self.promouvoir_et_suivre().await;
        }
        self.remplir()
    }

    /// Promeut le fichier en cours (`.part` → cible), le compte terminé, et
    /// démarre le suivant. Un échec de vidage ou de renommage retire le `.part`
    /// et devient une erreur du bilan.
    async fn promouvoir_et_suivre(&mut self) -> Vec<FileContentsRequest> {
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
        self.demarrer().await
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

    /// Un « .part » préexistant du poste n'est ni tronqué ni promu quand le
    /// serveur copie un fichier de même nom. Trouvé par l'audit du 7 septembre
    /// 2026 : dans le dossier des téléchargements, Firefox écrit son
    /// téléchargement en cours sous exactement « <nom>.part » et ne le renomme
    /// qu'à la fin ; l'ancien `File::create` du fichier de travail le tronquait
    /// (perte de données silencieuse). La réception se replie sur un nom de
    /// travail dérivé et laisse le `.part` de Firefox intact.
    #[tokio::test]
    async fn un_part_preexistant_n_est_ni_tronque_ni_promu() {
        let d = temp("part-preexistant");
        // Téléchargement Firefox en cours : « installateur.iso.part » plein,
        // « installateur.iso » pas encore là.
        std::fs::write(
            d.join("installateur.iso.part"),
            b"telechargement firefox en cours",
        )
        .unwrap();
        let mut r = Reception::nouvelle(d.clone(), vec![fichier("installateur.iso", 4)], None, 1);
        jouer(&mut r, &[b"avsh".to_vec()]).await;
        assert!(r.terminee() && r.erreurs().is_empty(), "{:?}", r.erreurs());
        // Le .part de Firefox est intact (ni tronqué, ni renommé/promu).
        assert_eq!(
            std::fs::read(d.join("installateur.iso.part")).unwrap(),
            b"telechargement firefox en cours"
        );
        // Le fichier reçu a bien atterri sous le nom cible (aucune collision
        // sur la cible finale, seul le « .part » était pris).
        assert_eq!(std::fs::read(d.join("installateur.iso")).unwrap(), b"avsh");
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

    /// Un chemin ordinaire reste sous le dossier de réception, `..` est ôté.
    /// Mais un composant piégé qui échapperait au dossier sous Windows
    /// (`PathBuf::push` d'un préfixe de disque remplace le chemin construit)
    /// est refusé : `d.name` « C:evil.exe » que `sanitize_file_path` laisse
    /// passer tel quel, un `relative_path` « a\\D:\\b » dont IronRDP ne retire
    /// pas la lettre de lecteur en milieu de chemin, un nom de périphérique
    /// réservé, un séparateur ou un octet nul glissés dans le nom. Trouvé par
    /// l'audit du 7 septembre 2026.
    #[test]
    fn le_chemin_local_reste_sous_le_dossier() {
        let d = FileDescriptor::new("f.txt").with_relative_path("a\\..\\b");
        assert_eq!(
            chemin_local(std::path::Path::new("/r"), &d),
            Some(PathBuf::from("/r/a/b/f.txt"))
        );
        let plat = FileDescriptor::new("seul");
        assert_eq!(
            chemin_local(std::path::Path::new("/r"), &plat),
            Some(PathBuf::from("/r/seul"))
        );
        // Cas d'évasion : chacun doit être refusé (aucun chemin rendu).
        for d in [
            FileDescriptor::new("C:evil.exe"),
            FileDescriptor::new("f.txt").with_relative_path("a\\D:\\b"),
            FileDescriptor::new("f.txt").with_relative_path("C:"),
            FileDescriptor::new("a/b"),
            FileDescriptor::new("NUL"),
            FileDescriptor::new("lpt1.txt"),
        ] {
            assert_eq!(
                chemin_local(std::path::Path::new("/r"), &d),
                None,
                "nom piégé accepté : {:?} / {:?}",
                d.relative_path,
                d.name
            );
        }
    }

    /// La réception écarte un fichier au nom piégé (préfixe de disque) sans
    /// écrire hors du dossier, et le signale dans `erreurs`. Trouvé par l'audit
    /// du 7 septembre 2026 : sous Windows le `.part` de « C:evil.exe » serait
    /// créé relativement au cwd du sidecar, hors du dossier annoncé.
    #[tokio::test]
    async fn un_nom_a_prefixe_de_disque_est_refuse_et_signale() {
        let d = temp("prefixe");
        let mut r = Reception::nouvelle(
            d.clone(),
            vec![fichier("C:evil.exe", 4), fichier("bon.txt", 2)],
            None,
            1,
        );
        let reqs = r.demarrer().await;
        // Le premier fichier est écarté ; c'est « bon.txt » qui démarre.
        assert_eq!(reqs.len(), 1);
        let fin = r.recevoir(reqs[0].stream_id, Some(b"ok")).await;
        assert!(fin.is_empty() && r.terminee());
        assert_eq!(r.erreurs().len(), 1);
        assert!(r.erreurs()[0].contains("refusé"), "{:?}", r.erreurs());
        assert!(!d.join("C:evil.exe.part").exists());
        assert_eq!(std::fs::read(d.join("bon.txt")).unwrap(), b"ok");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Un nom qui porte un contrôle bidirectionnel Unicode ou un caractère
    /// invisible est refusé à l'écriture, et l'affichage rend ces caractères
    /// visibles au lieu de les laisser réordonner le texte. Les caractères
    /// légitimes (accents, liant d'emoji) passent.
    ///
    /// Trouvé par l'audit du 9 septembre 2026 : un serveur annonçant
    /// « malware\u{202E}txt.exe » faisait afficher « malwareexe.txt » à
    /// l'utilisateur (le RIGHT-TO-LEFT OVERRIDE inverse ce qui suit), qui
    /// acceptait un exécutable en croyant prendre un fichier texte ; le nom
    /// écrit sur le disque gardait le caractère et continuait de mentir dans
    /// tous les gestionnaires de fichiers.
    #[test]
    fn un_nom_a_controle_bidirectionnel_est_refuse_et_neutralise_a_l_affichage() {
        for d in [
            FileDescriptor::new("malware\u{202E}txt.exe"),
            FileDescriptor::new("facture\u{2066}gpj.exe"),
            FileDescriptor::new("note\u{200F}txt.exe"),
            FileDescriptor::new("a\u{200B}b.txt"),
            FileDescriptor::new("a\u{FEFF}b.txt"),
            FileDescriptor::new("saut\nde ligne.txt"),
            // Le front est une webview : dans un nœud texte, « \n » est replié
            // en espace par le traitement des blancs, tandis que U+2028 et
            // U+2029 restent des sauts de ligne forcés. C'est donc eux, et non
            // « \n », qui coupent en deux le badge des fichiers en cours et la
            // notification d'erreurs.
            FileDescriptor::new("a\u{2028}b.txt"),
            FileDescriptor::new("a\u{2029}b.txt"),
            // Invisibles sans chasse que la première correction avait laissés
            // passer alors qu'elle annonçait les filtrer.
            FileDescriptor::new("a\u{00AD}b.txt"),
            FileDescriptor::new("a\u{180E}b.txt"),
            FileDescriptor::new("a\u{115F}b.txt"),
            FileDescriptor::new("a\u{1160}b.txt"),
            FileDescriptor::new("a\u{3164}b.txt"),
            FileDescriptor::new("a\u{FFA0}b.txt"),
            FileDescriptor::new("a\u{2061}b.txt"),
            FileDescriptor::new("a\u{FFF9}b.txt"),
            FileDescriptor::new("f.txt").with_relative_path("dossier\u{202E}"),
        ] {
            assert_eq!(
                chemin_local(std::path::Path::new("/r"), &d),
                None,
                "nom trompeur accepté : {:?} / {:?}",
                d.relative_path,
                d.name
            );
        }
        // Ce qui est légitime reste accepté : accents, liant d'emoji (U+200D,
        // ce qui soude une séquence emoji) et antiliant (U+200C, ce sur quoi
        // repose l'écriture persane, « می‌رود » sans lui devient un autre mot).
        for d in [
            FileDescriptor::new("é\u{300}tat des lieux.txt"),
            FileDescriptor::new("famille \u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}.png"),
            FileDescriptor::new("می\u{200C}رود.txt"),
        ] {
            assert!(
                chemin_local(std::path::Path::new("/r"), &d).is_some(),
                "nom légitime refusé : {:?}",
                d.name
            );
        }
        // L'affichage voit le nom avant toute acceptation : il ne doit plus
        // porter le caractère qui inverse la lecture.
        let a = annonce(&[fichier("malware\u{202E}txt.exe", 4)]);
        assert!(
            !a[0].chemin.contains('\u{202E}'),
            "le chemin annoncé garde le contrôle bidi : {:?}",
            a[0].chemin
        );
        assert!(a[0].chemin.contains('\u{FFFD}'), "{:?}", a[0].chemin);
    }

    /// La réception écarte le fichier au nom trompeur sans rien écrire, et le
    /// message d'erreur lui-même ne réordonne pas le bilan affiché.
    ///
    /// Trouvé par l'audit du 9 septembre 2026 : voir le test précédent.
    #[tokio::test]
    async fn un_nom_a_controle_bidirectionnel_n_est_pas_ecrit_sur_le_disque() {
        let d = temp("bidi");
        let piege = "malware\u{202E}txt.exe";
        // Le second piège coupe en deux le bilan affiché : dans la webview,
        // U+2028 est un saut de ligne forcé que le traitement des blancs ne
        // replie pas, contrairement à « \n ».
        let coupure = "bilan\u{2028}tronqué.txt";
        let mut r = Reception::nouvelle(
            d.clone(),
            vec![
                fichier(piege, 4),
                fichier(coupure, 4),
                fichier("bon.txt", 2),
            ],
            None,
            1,
        );
        let reqs = r.demarrer().await;
        // Les deux premiers fichiers sont écartés ; c'est « bon.txt » qui démarre.
        assert_eq!(reqs.len(), 1);
        let fin = r.recevoir(reqs[0].stream_id, Some(b"ok")).await;
        assert!(fin.is_empty() && r.terminee());
        assert_eq!(r.erreurs().len(), 2, "{:?}", r.erreurs());
        assert!(
            r.erreurs().iter().all(|e| e.contains("refusé")),
            "{:?}",
            r.erreurs()
        );
        assert!(
            !r.erreurs()
                .iter()
                .any(|e| e.contains('\u{202E}') || e.contains('\u{2028}')),
            "l'erreur affichée garde le caractère trompeur : {:?}",
            r.erreurs()
        );
        assert!(!d.join(piege).exists() && !d.join(format!("{piege}.part")).exists());
        assert!(!d.join(coupure).exists() && !d.join(format!("{coupure}.part")).exists());
        assert_eq!(std::fs::read(d.join("bon.txt")).unwrap(), b"ok");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Un sous-dossier du dossier de réception qui est en réalité un lien
    /// symbolique ne laisse pas le serveur distant écrire à l'autre bout du
    /// lien, et un nom de fichier déjà occupé par un lien pendouillant n'est
    /// pas suivi non plus.
    ///
    /// Trouvé par l'audit du 9 septembre 2026 : `chemin_local` ne vérifiait la
    /// sortie du dossier que lexicalement (`starts_with` sur des composants
    /// dont aucun n'est `..`, donc toujours vrai), sans jamais résoudre les
    /// liens. Un `Téléchargements/partage -> ~/.ssh` préexistant (stow, synchro
    /// nuagique, raccourci de l'utilisateur) suffisait à ce qu'un serveur
    /// annonçant `relative_path = "partage"` et `name = "authorized_keys"`
    /// fasse écrire un contenu de son choix hors du dossier de réception. Le
    /// module RDPDR voisin (`disque.rs::resoudre`) s'en défendait déjà.
    #[cfg(unix)]
    #[tokio::test]
    async fn un_lien_du_dossier_de_reception_n_est_pas_suivi() {
        let d = temp("lien-reception");
        let dehors = temp("lien-reception-dehors");
        std::fs::write(dehors.join("authorized_keys"), b"cle de l utilisateur").unwrap();
        // Le piège du scénario : un sous-dossier qui est un lien vers ailleurs.
        std::os::unix::fs::symlink(&dehors, d.join("partage")).unwrap();
        // Second piège : un lien pendouillant portant le nom d'un fichier
        // annoncé. Sa cible n'existe pas encore, `exists()` le disait libre et
        // la création d'un fichier vide écrivait donc au bout du lien.
        std::os::unix::fs::symlink(dehors.join("neuf.txt"), d.join("neuf.txt")).unwrap();

        let fichiers = vec![
            fichier("authorized_keys", 4).with_relative_path("partage"),
            fichier("neuf.txt", 0),
            fichier("bon.txt", 2),
        ];
        let mut r = Reception::nouvelle(d.clone(), fichiers, None, 1);
        jouer(&mut r, &[b"pwn!".to_vec(), vec![], b"ok".to_vec()]).await;
        assert!(r.terminee());

        // Rien n'a traversé les liens : au bout, on retrouve exactement ce que
        // le poste y avait, et pas un fichier de plus (ni la cible reçue, ni un
        // nom dérivé « (2) », ni un fichier de travail « .part »).
        let mut restant: Vec<String> = std::fs::read_dir(&dehors)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        restant.sort();
        assert_eq!(
            restant,
            ["authorized_keys"],
            "le serveur a écrit hors du dossier de réception"
        );
        assert_eq!(
            std::fs::read(dehors.join("authorized_keys")).unwrap(),
            b"cle de l utilisateur"
        );
        assert_eq!(r.erreurs().len(), 1, "{:?}", r.erreurs());
        assert!(r.erreurs()[0].contains("lien"), "{:?}", r.erreurs());

        // Le lien pendouillant n'a pas été suivi : le lien est toujours là et
        // le fichier reçu a pris un nom dérivé, à l'intérieur du dossier.
        assert!(std::fs::symlink_metadata(d.join("neuf.txt"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(d.join("neuf (2).txt").exists());

        // Un fichier sain de la même offre passe quand même.
        assert_eq!(std::fs::read(d.join("bon.txt")).unwrap(), b"ok");
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&dehors);
    }

    /// Le dossier de réception manquant est créé, comme avant que la descente
    /// composant par composant ne remplace `create_dir_all`. Écrit avec la
    /// correction de l'audit du 9 septembre 2026 pour garder ce comportement.
    #[tokio::test]
    async fn un_dossier_de_reception_absent_est_cree() {
        let d = temp("absent").join("sous").join("encore");
        assert!(!d.exists());
        let mut r = Reception::nouvelle(d.clone(), vec![fichier("f.txt", 2)], None, 1);
        jouer(&mut r, &[b"ok".to_vec()]).await;
        assert!(r.terminee() && r.erreurs().is_empty(), "{:?}", r.erreurs());
        assert_eq!(std::fs::read(d.join("f.txt")).unwrap(), b"ok");
        let _ = std::fs::remove_dir_all(d.parent().unwrap().parent().unwrap());
    }

    /// Un descripteur sans FD_FILESIZE (`file_size` à `None`) ne doit pas être
    /// confondu avec un fichier vide : le sidecar demande d'abord la taille par
    /// FILECONTENTS_SIZE (MS-RDPECLIP 2.2.5.2.3.1), puis reçoit le contenu
    /// entier. Trouvé par l'audit du 7 septembre 2026 : `file_size.unwrap_or(0)`
    /// créait un fichier de 0 octet compté « réussi » (bilan « 0 erreur »),
    /// impossible à distinguer d'un vrai fichier vide.
    #[tokio::test]
    async fn un_descripteur_sans_taille_demande_filecontents_size() {
        let d = temp("sans-taille");
        let taille = u64::from(MORCEAU) + 42; // plus d'un morceau
        let contenu: Vec<u8> = (0..taille).map(|i| (i % 251) as u8).collect();
        // Descripteur SANS with_file_size : file_size vaut None (FD_FILESIZE
        // absent), comme un serveur qui ne renseigne pas la taille.
        let desc =
            FileDescriptor::new("mystere.bin").with_attributes(ClipboardFileAttributes::NORMAL);
        assert_eq!(
            desc.file_size, None,
            "le descripteur ne porte pas de taille"
        );
        let mut r = Reception::nouvelle(d.clone(), vec![desc], Some(3), 1);

        // Premier tour : une seule requête, et c'est une requête de TAILLE.
        let reqs = r.demarrer().await;
        assert_eq!(reqs.len(), 1, "une requête FILECONTENTS_SIZE d'abord");
        assert_eq!(reqs[0].flags, FileContentsFlags::SIZE);
        assert_eq!((reqs[0].requested_size, reqs[0].position), (8, 0));
        assert_eq!(reqs[0].data_id, Some(3));

        // On répond la taille (8 octets petit-boutien) : les requêtes de plage
        // suivent, et aucune n'est encore une requête de taille.
        let mut suite = r
            .recevoir(reqs[0].stream_id, Some(&taille.to_le_bytes()))
            .await;
        assert!(!suite.is_empty(), "les plages suivent la taille");
        assert!(suite.iter().all(|q| q.flags == FileContentsFlags::RANGE));
        while !suite.is_empty() {
            let mut prochaines = Vec::new();
            for req in suite {
                let debut = usize::try_from(req.position).unwrap().min(contenu.len());
                let fin = (debut + req.requested_size as usize).min(contenu.len());
                prochaines.extend(r.recevoir(req.stream_id, Some(&contenu[debut..fin])).await);
            }
            suite = prochaines;
        }
        assert!(r.terminee() && r.erreurs().is_empty(), "{:?}", r.erreurs());
        assert_eq!(std::fs::read(d.join("mystere.bin")).unwrap(), contenu);
        assert!(
            !d.join("mystere.bin.part").exists(),
            "le .part doit être promu"
        );
        // La progression connaît la vraie taille : elle n'affiche plus 0.
        let p = r.progression();
        assert_eq!(
            (p.fait, p.total, p.termines, p.nombre),
            (taille, taille, 1, 1)
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Un serveur qui, taille demandée, refuse la réponse (aucune donnée) ne
    /// laisse pas croire à une réussite : le fichier devient une erreur du
    /// bilan, pas un fichier de 0 octet silencieux.
    #[tokio::test]
    async fn une_taille_refusee_devient_une_erreur() {
        let d = temp("taille-refusee");
        let desc =
            FileDescriptor::new("mystere.bin").with_attributes(ClipboardFileAttributes::NORMAL);
        let mut r = Reception::nouvelle(d.clone(), vec![desc], None, 1);
        let reqs = r.demarrer().await;
        assert_eq!(reqs[0].flags, FileContentsFlags::SIZE);
        // Le distant refuse : `None` (réponse d'erreur côté protocole).
        let suite = r.recevoir(reqs[0].stream_id, None).await;
        assert!(suite.is_empty() && r.terminee());
        assert_eq!(r.erreurs().len(), 1, "{:?}", r.erreurs());
        assert!(
            r.erreurs()[0].contains("taille inconnue"),
            "{:?}",
            r.erreurs()
        );
        assert!(!d.join("mystere.bin").exists() && !d.join("mystere.bin.part").exists());
        let _ = std::fs::remove_dir_all(&d);
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
