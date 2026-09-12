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

use crate::verrou::Verrou as _;
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
///
/// Ni point ni espace en fin de nom (audit du 12 septembre 2026, C-sidecar-9) :
/// le test amont des noms de périphérique compare le radical avant le premier
/// point, si bien que `CON ` le passait, et Windows retire points et espaces
/// finaux : c'est vers le périphérique qu'on aurait écrit.
fn composant_sur(c: &str) -> bool {
    if c.contains([':', '/', '\\', '\0'])
        || c.ends_with(['.', ' '])
        || ironrdp::cliprdr::is_windows_device_name(c)
    {
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
///
/// `fichier` précède `partiel` : les champs se détruisent dans l'ordre de
/// déclaration, et Windows refuse de supprimer un fichier encore ouvert.
struct EnCours {
    index: usize,
    fichier: tokio::fs::File,
    partiel: Partiel,
    cible: PathBuf,
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
        // Saturé : voir `octets_annonces`.
        let total = fichiers
            .iter()
            .filter(|d| !est_dossier(d))
            .map(|d| d.file_size.unwrap_or(0))
            .fold(0u64, u64::saturating_add);
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

    /// Cette requête appartient-elle au fichier en cours ?
    fn suit(&self, stream_id: u32) -> bool {
        self.en_cours
            .as_ref()
            .is_some_and(|e| e.flux_taille == Some(stream_id) || e.en_vol.contains_key(&stream_id))
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
            creer_vide(&cible).await?;
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
            fichier,
            partiel: Partiel::nouveau(partiel),
            cible,
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
        // `flux_ids` compte avec `en_vol` : trouvé le 12 septembre 2026, la
        // borne ne regardait que `en_vol`, rempli seulement après cette boucle,
        // si bien que toutes les plages du fichier partaient d'un coup (voir le
        // test `un_gros_fichier_ne_met_jamais_plus_de_en_vol_requetes_en_vol`).
        while e.en_vol.len() + flux_ids.len() < EN_VOL && e.demande < e.taille {
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
                .and_then(|d| <[u8; 8]>::try_from(d).ok())
                .map(u64::from_le_bytes);
            let Some(taille) = taille else {
                // Toujours pas de taille : on ne peut pas recevoir ce fichier,
                // et on le signale au lieu de le compter « réussi » (0 octet).
                // Le `.part` part avec `e` (voir `Partiel`).
                let e = self
                    .en_cours
                    .take()
                    .expect("invariant : en_cours vient d'être emprunté ci-dessus");
                self.erreurs.push(format!(
                    "{} : taille inconnue",
                    chemin_relatif(&self.fichiers[e.index])
                ));
                self.termines += 1;
                return self.demarrer().await;
            };
            // Audit du 12 septembre 2026 (C-sidecar-7) : l'utilisateur a accepté
            // un fichier affiché à 0 octet (taille non annoncée) ; la réponse
            // SIZE fixait ensuite n'importe quelle taille, et le serveur
            // remplissait le disque un mégaoctet à la fois. Ce qui ne tient pas
            // dans l'espace libre du dossier est refusé avant toute plage.
            let libres = crate::disque::octets_libres(&self.dossier);
            if taille > libres {
                let e = self
                    .en_cours
                    .take()
                    .expect("invariant : en_cours vient d'être emprunté ci-dessus");
                self.erreurs.push(format!(
                    "{} : taille annoncée après coup ({taille} octets) plus grande que \
                     l'espace libre du dossier ({libres} octets)",
                    chemin_relatif(&self.fichiers[e.index])
                ));
                self.termines += 1;
                return self.demarrer().await;
            }
            e.taille = taille;
            // La taille annoncée manquait au total (comptée 0) : on la rattrape
            // pour que la progression n'affiche plus 0 pour ce fichier.
            self.total = self.total.saturating_add(taille);
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
            // Le `.part` part avec `e` (voir `Partiel`).
            let e = self
                .en_cours
                .take()
                .expect("invariant : en_cours est emprunté par `e` juste au-dessus");
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
        let Some(mut e) = self.en_cours.take() else {
            return self.demarrer().await;
        };
        let fin = async {
            e.fichier.flush().await.context("vidage")?;
            drop(e.fichier);
            promouvoir(&e.partiel.chemin, &e.cible)
                .await
                .with_context(|| format!("promotion vers {}", e.cible.display()))
        }
        .await;
        match fin {
            Ok(_) => e.partiel.promu(),
            // Le `.part` part avec `e.partiel` (voir `Partiel`).
            Err(err) => self.erreurs.push(format!(
                "{} : {err:#}",
                chemin_relatif(&self.fichiers[e.index])
            )),
        }
        self.termines += 1;
        self.demarrer().await
    }
}

/// Les requêtes d'une réception, encodées par `encoder` pour le canal. Une
/// requête que le canal refuse (`None`) compte son fichier en échec, au lieu
/// d'attendre une réponse qui ne viendra jamais ; les requêtes qui suivent
/// alors (le fichier suivant) passent par le même chemin.
///
/// Trouvé par l'audit du 12 septembre 2026 (C-sidecar-6) : la boucle de session
/// perdait en silence une requête refusée par la bibliothèque (« clipboard
/// channel is not in Ready state », après une FormatList du distant au mauvais
/// moment). La réception gardait un `streamId` que personne ne servirait : elle
/// ne finissait jamais, aucune autre n'était possible dans la session, et le
/// `.part` restait sur le disque. Le cas était traité pour l'offre, jamais pour
/// la réception.
pub(crate) async fn preparer_requetes<M>(
    r: &mut Reception,
    reqs: Vec<FileContentsRequest>,
    mut encoder: impl FnMut(FileContentsRequest) -> Option<M>,
) -> Vec<M> {
    let mut file: std::collections::VecDeque<FileContentsRequest> = reqs.into();
    let mut messages = Vec::new();
    while let Some(req) = file.pop_front() {
        // Les autres requêtes d'un fichier déjà compté en échec ne partent pas.
        if !r.suit(req.stream_id) {
            continue;
        }
        let flux = req.stream_id;
        match encoder(req) {
            Some(m) => messages.push(m),
            None => file.extend(r.recevoir(flux, None).await),
        }
    }
    messages
}

/// Le total des tailles annoncées, saturé.
///
/// Trouvé par l'audit du 12 septembre 2026 (C-sidecar-14, C-panique-7) : les
/// `u64` du distant étaient additionnés sans saturation. Deux descripteurs à
/// 2^63 faisaient paniquer le processus en debug (tests, fuzz, binaires
/// instrumentés de la couverture) dès que le distant copiait des fichiers,
/// sans le moindre geste de l'utilisateur ; en publication, le total bouclait.
pub(crate) fn octets_annonces(liste: &[FileDescriptor]) -> u64 {
    liste
        .iter()
        .filter_map(|d| d.file_size)
        .fold(0u64, u64::saturating_add)
}

/// Crée un fichier vide à `cible`, ou sous un nom dérivé si la place est prise
/// entre-temps : jamais de troncature. Rend le chemin retenu.
///
/// Trouvé par l'audit du 12 septembre 2026 (C-sidecar-8) : `File::create`
/// tronquait une cible apparue entre le choix du nom et la création.
async fn creer_vide(cible: &Path) -> Result<PathBuf> {
    let mut chemin = cible.to_path_buf();
    for _ in 0..16 {
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&chemin)
            .await
        {
            Ok(_) => return Ok(chemin),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                chemin = sans_collision(cible);
            }
            Err(e) => return Err(e).with_context(|| format!("création de {}", chemin.display())),
        }
    }
    anyhow::bail!("création de {} : le nom reste pris", cible.display())
}

/// Promeut le fichier de travail `partiel` en `cible` sans jamais remplacer un
/// fichier existant. Rend le nom retenu, dérivé (« (2) »…) si `cible` est
/// apparue pendant le transfert.
///
/// Trouvé par l'audit du 12 septembre 2026 (C-sidecar-3) : le renommage final
/// (`rename(2)`, `MoveFileExW` avec remplacement sous Windows) écrasait une
/// cible apparue depuis le choix du nom : deux onglets qui reçoivent
/// `rapport.pdf`, ou un téléchargement du navigateur qui se termine sous ce
/// nom pendant un long transfert. Un lien physique, lui, échoue si le nom est
/// pris ; on retire ensuite le nom de travail. Sur un système de fichiers sans
/// liens physiques (FAT, exFAT, certains partages SMB), repli sur le renommage
/// après un dernier test du nom : la fenêtre de course se réduit à ce test.
async fn promouvoir(partiel: &Path, cible: &Path) -> std::io::Result<PathBuf> {
    let mut chemin = cible.to_path_buf();
    for _ in 0..16 {
        match tokio::fs::hard_link(partiel, &chemin).await {
            Ok(()) => {
                tokio::fs::remove_file(partiel).await?;
                return Ok(chemin);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                chemin = sans_collision(cible);
            }
            Err(_) if occupe(&chemin) => chemin = sans_collision(cible),
            Err(_) => {
                tokio::fs::rename(partiel, &chemin).await?;
                return Ok(chemin);
            }
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        format!("{} : le nom reste pris", cible.display()),
    ))
}

/// Le fichier de travail (`.part`) d'une réception : retiré du disque s'il
/// n'a pas été promu, quelle que soit la façon dont la réception s'arrête.
///
/// Trouvé par l'audit du 12 septembre 2026 (C-sidecar-15) : le `.part` était
/// retiré sur refus et sur échec de promotion, mais pas quand la session se
/// terminait au milieu d'un transfert (WebSocket fermé, serveur qui
/// raccroche) : les `<nom>.part` s'accumulaient dans les téléchargements, et
/// un serveur qui raccroche laissait un fichier tronqué au nom prévisible.
struct Partiel {
    chemin: PathBuf,
    a_retirer: bool,
}

impl Partiel {
    fn nouveau(chemin: PathBuf) -> Self {
        Self {
            chemin,
            a_retirer: true,
        }
    }

    /// Le nom de travail a été promu : il ne nous appartient plus.
    fn promu(mut self) {
        self.a_retirer = false;
    }
}

impl Drop for Partiel {
    fn drop(&mut self) {
        if self.a_retirer {
            let _ = std::fs::remove_file(&self.chemin);
        }
    }
}

/// Les chemins que l'utilisateur a désignés, annoncés par le parent Tauri sur
/// l'entrée standard (`AUTORISE <chemin>`, une ligne par chemin) : seuls ceux-là
/// peuvent être offerts au distant.
///
/// Réserve de l'audit du 9 septembre 2026 : l'offre recevait ses chemins du
/// front par le WebSocket, que tout script de la webview atteint avec le
/// jeton. Rien ne distinguait un fichier que l'utilisateur venait de choisir
/// d'un `~/.ssh/id_ed25519` désigné par un script hostile. Le parent, lui, voit
/// passer la boîte de sélection et le dépôt sur la fenêtre : il annonce, ce
/// processus retient, et une offre ne peut porter que de l'annoncé.
#[derive(Debug, Default)]
pub struct Designations {
    inner: std::sync::Mutex<Vec<PathBuf>>,
}

impl Designations {
    /// Constructible en `static` : pas de table de hachage, dont la
    /// construction n'est pas constante ; la liste reste courte.
    pub const fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Retient un chemin désigné par l'utilisateur.
    pub fn designer(&self, chemin: PathBuf) {
        let mut g = self.inner.verrou();
        if !g.contains(&chemin) {
            g.push(chemin);
        }
    }

    /// Ce chemin a-t-il été désigné tel quel ?
    pub fn est_designe(&self, chemin: &Path) -> bool {
        self.inner.verrou().iter().any(|c| c == chemin)
    }
}

/// Les désignations de ce processus, alimentées par la lecture de stdin.
pub static DESIGNATIONS: Designations = Designations::new();

/// Une ligne de stdin est-elle une désignation ? Le mot de passe occupe la
/// première ligne et est consommé avant ; tout le reste suit ce format.
#[must_use]
pub fn designation_depuis_ligne(ligne: &str) -> Option<PathBuf> {
    let chemin = ligne
        .trim_end_matches(['\n', '\r'])
        .strip_prefix("AUTORISE ")?;
    (!chemin.is_empty()).then(|| PathBuf::from(chemin))
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
/// Ce qu'on accorde à une désignation encore en route. Le parent l'écrit sur
/// stdin juste avant que le front n'envoie l'offre par le WebSocket ; les deux
/// arrivent par des fils différents et rien n'ordonne leur traitement. Sur un
/// exécuteur chargé (chaîne GitLab, 10 septembre 2026), l'offre est passée la
/// première et le fichier légitime a été refusé. Deux secondes : la ligne est
/// déjà dans le tube, elle se lit en microsecondes ; un script hostile qui
/// invente un chemin attend ce délai pour rien.
const DELAI_DESIGNATION: std::time::Duration = std::time::Duration::from_secs(2);

pub(crate) async fn preparer_offre(chemins: &[PathBuf], designes: &Designations) -> Result<Offre> {
    // Avant toute lecture : un seul chemin non désigné fait refuser l'offre
    // entière, sans rien parcourir. Un script hostile n'apprend même pas si le
    // chemin existe. Une désignation en route est attendue un court instant.
    let debut = std::time::Instant::now();
    while let Some(racine) = chemins.iter().find(|r| !designes.est_designe(r)) {
        anyhow::ensure!(
            debut.elapsed() < DELAI_DESIGNATION,
            "{} n'a pas été désigné par l'utilisateur (boîte de sélection ou dépôt sur la fenêtre) : offre refusée",
            racine.display()
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
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
        let bac = crate::empreintes::BacAvashHome::poser("fichiers-foyer");
        let foyer = bac.chemin().to_path_buf();
        std::fs::create_dir_all(&foyer).unwrap();
        let sous_foyer = super::dossier_par_defaut();
        std::fs::create_dir_all(foyer.join("Downloads")).unwrap();
        let sous_downloads = super::dossier_par_defaut();
        assert_eq!(sous_foyer, foyer);
        assert_eq!(sous_downloads, foyer.join("Downloads"));
    }

    use super::{
        annonce, chemin_local, designation_depuis_ligne, preparer_offre, Designations, Reception,
        MORCEAU,
    };
    use ironrdp::cliprdr::pdu::{
        ClipboardFileAttributes, FileContentsFlags, FileContentsRequest, FileDescriptor,
    };
    use std::path::PathBuf;

    /// Des désignations qui couvrent les chemins donnés : le cas courant des
    /// tests, où c'est l'offre elle-même qui est à l'épreuve.
    fn designe_tout(chemins: &[PathBuf]) -> Designations {
        let d = Designations::default();
        for c in chemins {
            d.designer(c.clone());
        }
        d
    }

    /// Le protocole d'annonce du parent : préfixe, chemin, fin de ligne.
    #[test]
    fn une_designation_se_lit_sur_une_ligne_prefixee() {
        assert_eq!(
            designation_depuis_ligne("AUTORISE /home/a/rapport.pdf\n"),
            Some(PathBuf::from("/home/a/rapport.pdf"))
        );
        assert_eq!(designation_depuis_ligne("AUTORISE \n"), None);
        assert_eq!(designation_depuis_ligne("/home/a/rapport.pdf\n"), None);
        assert_eq!(designation_depuis_ligne("autorise /x\n"), None);
    }

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

    /// Trouvé par la chaîne GitLab le 10 septembre 2026, à sa première
    /// exécution des désignations : le parent écrit `AUTORISE <chemin>` sur
    /// stdin puis le front envoie l'offre par le WebSocket ; sur un exécuteur
    /// chargé, l'offre a été traitée AVANT que le fil de lecture de stdin ait
    /// enregistré la ligne, et le fichier légitime a été refusé comme non
    /// désigné. La désignation est en route, pas absente : une offre attend
    /// un court instant qu'elle arrive avant de refuser.
    #[tokio::test]
    async fn une_designation_en_route_n_est_pas_prise_pour_une_absence() {
        let d = temp("offre-en-route");
        std::fs::create_dir_all(&d).unwrap();
        let choisi = d.join("doc.txt");
        std::fs::write(&choisi, b"doc").unwrap();
        let designes = std::sync::Arc::new(Designations::default());
        let tardif = designes.clone();
        let chemin = choisi.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            tardif.designer(chemin);
        });
        let debut = std::time::Instant::now();
        preparer_offre(std::slice::from_ref(&choisi), &designes)
            .await
            .expect("une désignation qui arrive 150 ms après l'offre doit être acceptée");
        assert!(
            debut.elapsed() < std::time::Duration::from_secs(2),
            "l'attente s'arrête dès que la désignation arrive"
        );
    }

    /// Réserve de l'audit du 9 septembre 2026 : l'offre au distant recevait ses
    /// chemins du front par le WebSocket, que tout script de la webview sait
    /// atteindre avec le jeton. Rien ne distinguait un fichier que l'utilisateur
    /// venait de choisir d'un `~/.ssh/id_ed25519` désigné par un script hostile.
    /// Le parent Tauri, qui voit passer la boîte de sélection et le dépôt sur la
    /// fenêtre, annonce désormais chaque chemin désigné sur stdin ; une offre ne
    /// peut porter que des chemins ainsi annoncés.
    #[tokio::test]
    async fn une_offre_ne_porte_que_des_chemins_designes_par_l_utilisateur() {
        let d = temp("offre-designee");
        std::fs::create_dir_all(&d).unwrap();
        let choisi = d.join("doc.txt");
        let secret = d.join("id_ed25519");
        std::fs::write(&choisi, b"doc").unwrap();
        std::fs::write(&secret, b"secret").unwrap();
        let designes = Designations::default();
        assert!(
            preparer_offre(std::slice::from_ref(&choisi), &designes)
                .await
                .is_err(),
            "rien n'est offrable tant que rien n'a été désigné"
        );
        designes.designer(choisi.clone());
        preparer_offre(std::slice::from_ref(&choisi), &designes)
            .await
            .expect("un chemin désigné s'offre");
        let err = preparer_offre(&[choisi.clone(), secret.clone()], &designes)
            .await
            .expect_err("un chemin non désigné fait refuser l'offre entière");
        assert!(
            format!("{err:#}").contains("désigné"),
            "le refus dit pourquoi : {err:#}"
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
        let offre = preparer_offre(
            &[d.join("doc"), d.join("seul.bin")],
            &designe_tout(&[d.join("doc"), d.join("seul.bin")]),
        )
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

#[cfg(test)]
mod tests_bornes_reception {
    use super::{
        composant_sur, creer_vide, octets_annonces, preparer_requetes, Reception, MORCEAU,
    };
    use ironrdp::cliprdr::pdu::{ClipboardFileAttributes, FileContentsFlags, FileDescriptor};
    use std::path::PathBuf;

    fn temp(nom: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("avash-bornes-{}-{nom}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn fichier(nom: &str, taille: u64) -> FileDescriptor {
        FileDescriptor::new(nom)
            .with_file_size(taille)
            .with_attributes(ClipboardFileAttributes::NORMAL)
    }

    /// Trouvé par l'audit du 12 septembre 2026 (C-sidecar-6) : quand la
    /// bibliothèque refusait une requête de contenu (« clipboard channel is not
    /// in Ready state », après une FormatList du distant au mauvais moment),
    /// elle était perdue en silence. `Reception` gardait un `streamId` que
    /// personne ne servirait : la réception ne finissait jamais, aucune autre
    /// n'était possible dans la session, et le `.part` restait sur le disque.
    #[tokio::test]
    async fn une_requete_de_contenu_refusee_par_le_canal_termine_le_fichier_au_lieu_de_bloquer() {
        let d = temp("refus-canal");
        let mut r = Reception::nouvelle(d.clone(), vec![fichier("f", 10)], None, 1);
        let reqs = r.demarrer().await;
        assert!(!reqs.is_empty());
        let envoyes = preparer_requetes(&mut r, reqs, |_| None::<()>).await;
        assert!(envoyes.is_empty());
        assert!(
            r.terminee(),
            "la réception attend une réponse qui ne viendra pas"
        );
        assert_eq!(r.erreurs().len(), 1, "{:?}", r.erreurs());
        assert!(!d.join("f.part").exists() && !d.join("f").exists());
        // Contrôle : un canal qui accepte tout reçoit toutes les requêtes.
        let mut r = Reception::nouvelle(d.clone(), vec![fichier("g", 10)], None, 1);
        let reqs = r.demarrer().await;
        let n = reqs.len();
        assert_eq!(preparer_requetes(&mut r, reqs, Some).await.len(), n);
        assert!(!r.terminee());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Trouvé par l'audit du 12 septembre 2026 (C-sidecar-14, C-panique-7) :
    /// les tailles annoncées par le distant étaient additionnées sans
    /// saturation. Deux descripteurs à 2^63 faisaient paniquer le processus en
    /// debug (tests, fuzz, binaires instrumentés de la couverture) dès que le
    /// distant copiait des fichiers, sans geste de l'utilisateur, et donnaient
    /// un total faux en publication.
    #[test]
    fn des_tailles_annoncees_gigantesques_ne_font_pas_deborder_le_total() {
        let enormes = vec![
            fichier("a", u64::MAX / 2 + 1),
            fichier("b", u64::MAX / 2 + 1),
        ];
        assert_eq!(octets_annonces(&enormes), u64::MAX);
        let r = Reception::nouvelle(PathBuf::from("/nulle-part"), enormes, None, 1);
        assert_eq!(r.progression().total, u64::MAX);
    }

    /// Trouvé par l'audit du 12 septembre 2026 (C-sidecar-7) : un fichier
    /// annoncé sans taille s'affiche à 0 octet ; l'utilisateur accepte, puis la
    /// réponse SIZE fixait la taille à n'importe quelle valeur, et le serveur
    /// remplissait le disque un mégaoctet à la fois. Une taille qui ne tient
    /// pas dans l'espace libre du dossier devient une erreur, sans une requête
    /// de plage.
    #[tokio::test]
    async fn une_taille_size_deraisonnable_devient_une_erreur_sans_requete_de_plage() {
        let d = temp("size-demesuree");
        let desc =
            FileDescriptor::new("mystere.bin").with_attributes(ClipboardFileAttributes::NORMAL);
        let mut r = Reception::nouvelle(d.clone(), vec![desc], None, 1);
        let reqs = r.demarrer().await;
        assert_eq!(reqs[0].flags, FileContentsFlags::SIZE);
        let suite = r
            .recevoir(reqs[0].stream_id, Some(&u64::MAX.to_le_bytes()))
            .await;
        assert!(
            suite.is_empty(),
            "{} requêtes de plage pour une taille démesurée",
            suite.len()
        );
        assert!(r.terminee());
        assert_eq!(r.erreurs().len(), 1, "{:?}", r.erreurs());
        assert!(!d.join("mystere.bin").exists() && !d.join("mystere.bin.part").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Trouvé par l'audit du 12 septembre 2026 (C-sidecar-3) : « rien n'écrase
    /// un fichier existant » n'était vrai qu'au démarrage de la réception. Le
    /// nom final se choisit avant le transfert, et le renommage final
    /// remplaçait une cible apparue entre-temps : deux onglets qui reçoivent
    /// `rapport.pdf`, ou un téléchargement du navigateur qui se termine sous ce
    /// nom pendant un long transfert, et le premier fichier était écrasé.
    #[tokio::test]
    async fn un_fichier_apparu_pendant_la_reception_n_est_pas_ecrase() {
        let d = temp("apparu");
        let mut r = Reception::nouvelle(d.clone(), vec![fichier("f", 4)], None, 1);
        let reqs = r.demarrer().await;
        assert_eq!(reqs.len(), 1);
        // Apparu entre le choix du nom et la fin du transfert.
        std::fs::write(d.join("f"), b"local").unwrap();
        for q in reqs {
            assert!(r.recevoir(q.stream_id, Some(b"recu")).await.is_empty());
        }
        assert!(r.terminee() && r.erreurs().is_empty(), "{:?}", r.erreurs());
        assert_eq!(std::fs::read(d.join("f")).unwrap(), b"local", "écrasé");
        assert_eq!(std::fs::read(d.join("f (2)")).unwrap(), b"recu");
        assert!(!d.join("f.part").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Trouvé par l'audit du 12 septembre 2026 (C-sidecar-8) : un fichier
    /// annoncé vide était créé par `File::create`, qui TRONQUE une cible
    /// existante, juste après le test du nom libre. Un fichier apparu entre les
    /// deux était vidé.
    #[tokio::test]
    async fn un_fichier_vide_annonce_ne_tronque_pas_un_fichier_apparu_entre_temps() {
        let d = temp("vide");
        std::fs::write(d.join("f"), b"local").unwrap();
        let cree = creer_vide(&d.join("f")).await.unwrap();
        assert_eq!(std::fs::read(d.join("f")).unwrap(), b"local", "tronqué");
        assert_eq!(cree, d.join("f (2)"));
        assert!(std::fs::read(&cree).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Trouvé par l'audit du 12 septembre 2026 (C-sidecar-9) : le test amont des
    /// noms de périphérique compare le radical avant le premier point ; `CON `
    /// (espace finale) le passait, et Windows retire les points et espaces
    /// finaux : c'est bien vers le périphérique qu'on aurait écrit.
    #[test]
    fn un_nom_a_fin_blanche_est_refuse_a_la_reception() {
        for c in ["CON ", "nom.", "nom ", "aux.", "rapport.pdf "] {
            assert!(!composant_sur(c), "{c:?} accepté");
        }
        assert!(composant_sur("nom.txt"));
        assert!(composant_sur(".cache"));
    }

    /// Trouvé le 12 septembre 2026 en écrivant le test de C-sidecar-7 : sur le
    /// code d'alors, une taille SIZE de `u64::MAX` a fait tuer le binaire de
    /// test par le noyau (22 Gio résidents). `remplir` bornait sa boucle par
    /// `en_vol.len() < EN_VOL`, mais `en_vol` ne se remplissait qu'APRÈS la
    /// boucle : toutes les plages du fichier partaient d'un coup, un million
    /// de requêtes pour un tébioctet annoncé, 2^44 pour `u64::MAX`. Le test
    /// existant ne demandait que deux morceaux, sous la borne. Ici 64 Mio : de
    /// quoi voir le défaut sans mettre la mémoire en danger.
    #[tokio::test]
    async fn un_gros_fichier_ne_met_jamais_plus_de_en_vol_requetes_en_vol() {
        let d = temp("en-vol");
        let taille = u64::from(MORCEAU) * 64;
        let mut r = Reception::nouvelle(d.clone(), vec![fichier("gros", taille)], None, 1);
        let reqs = r.demarrer().await;
        assert_eq!(reqs.len(), super::EN_VOL, "requêtes émises d'un coup");
        // Une réponse libère une place, et une seule requête la reprend.
        let q = &reqs[0];
        let suite = r
            .recevoir(q.stream_id, Some(&vec![0u8; q.requested_size as usize]))
            .await;
        assert_eq!(suite.len(), 1);
        drop(r);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Trouvé par l'audit du 12 septembre 2026 (C-sidecar-15) : le `.part`
    /// était retiré sur refus et sur échec de promotion, pas quand la session
    /// se termine au milieu d'un transfert (WebSocket fermé, serveur qui
    /// raccroche). Les `<nom>.part` s'accumulaient dans les téléchargements.
    #[tokio::test]
    async fn une_reception_abandonnee_ne_laisse_pas_de_part() {
        let d = temp("abandon");
        let taille = u64::from(MORCEAU) * 2;
        let mut r = Reception::nouvelle(d.clone(), vec![fichier("f", taille)], None, 1);
        let reqs = r.demarrer().await;
        assert!(reqs.len() >= 2);
        let q = &reqs[0];
        let _ = r
            .recevoir(q.stream_id, Some(&vec![1u8; q.requested_size as usize]))
            .await;
        assert!(d.join("f.part").exists(), "la réception est en cours");
        drop(r);
        assert!(!d.join("f.part").exists(), "le .part survit à l'abandon");
        assert!(!d.join("f").exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
