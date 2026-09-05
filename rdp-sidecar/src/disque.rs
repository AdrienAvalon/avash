//! Redirection de lecteur (canal statique `rdpdr`, MS-RDPEFS) : un dossier du
//! poste, vu du bureau distant comme un lecteur nommé « Avash ».
//!
//! Le serveur pilote tout : il envoie des requêtes d'entrée-sortie (IRP :
//! ouvrir, lire, écrire, énumérer, renommer, supprimer…), le canal les décode
//! (`ironrdp-rdpdr`) et les remet à [`DisqueBackend`], qui ne fait que les
//! passer à un fil dédié. L'accès disque bloque, et la boucle de session ne le
//! doit pas : une lecture de quatre mégaoctets dans la branche qui lit les
//! trames figerait l'affichage. Le fil répond quand il a fini ; le protocole
//! apparie requête et réponse par `completion_id`, pas par ordre, et une
//! réponse différée est permise (FreeRDP travaille ainsi). Comme le canal
//! n'annonce pas `ENABLE_ASYNCIO`, le serveur ne lance jamais deux
//! opérations sur le même fichier en même temps : un seul fil suffit.
//!
//! **Sécurité : c'est le serveur qui choisit les chemins.** Chacun est ramené
//! sous la racine partagée avant d'ouvrir quoi que ce soit : les composants
//! `..` sont refusés, le répertoire parent est résolu (liens symboliques
//! compris) et doit rester sous la racine, et un lien symbolique en dernier
//! composant n'est pas suivi. Un serveur hostile ne lit donc que le dossier
//! qu'on lui a donné.

use ironrdp::pdu::PduResult;
use ironrdp::rdpdr::backend::RdpdrBackend;
use ironrdp::rdpdr::pdu::efs::{
    Boolean, Characteristics, ClientDriveQueryDirectoryResponse,
    ClientDriveQueryInformationResponse, ClientDriveQueryVolumeInformationResponse,
    ClientDriveSetInformationResponse, CreateDisposition, CreateOptions, DesiredAccess,
    DeviceCloseRequest, DeviceCloseResponse, DeviceControlRequest, DeviceControlResponse,
    DeviceCreateRequest, DeviceCreateResponse, DeviceIoResponse, DeviceReadRequest,
    DeviceReadResponse, DeviceWriteRequest, DeviceWriteResponse, FileAttributeTagInformation,
    FileAttributes, FileBasicInformation, FileBothDirectoryInformation, FileDirectoryInformation,
    FileFsAttributeInformation, FileFsDeviceInformation, FileFsFullSizeInformation,
    FileFsSizeInformation, FileFsVolumeInformation, FileFullDirectoryInformation,
    FileInformationClass, FileInformationClassLevel, FileNamesInformation, FileStandardInformation,
    FileSystemAttributes, FileSystemInformationClass, FileSystemInformationClassLevel, Information,
    NtStatus, ServerDeviceAnnounceResponse, ServerDriveIoRequest, ServerDriveLockControlRequest,
    ServerDriveNotifyChangeDirectoryRequest, ServerDriveQueryDirectoryRequest,
    ServerDriveQueryInformationRequest, ServerDriveQueryVolumeInformationRequest,
    ServerDriveSetInformationRequest,
};
use ironrdp::rdpdr::pdu::esc::{ScardCall, ScardIoCtlCode};
use ironrdp::rdpdr::pdu::RdpdrPdu;
use ironrdp::svc::SvcMessage;
use std::collections::HashMap;
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};

/// Identifiant du lecteur annoncé au serveur, et son nom sur le bureau distant.
pub(crate) const LECTEUR_ID: u32 = 1;
pub(crate) const LECTEUR_NOM: &str = "Avash";

/// Étiquette du volume, telle que l'explorateur la montre.
const ETIQUETTE_VOLUME: &str = "AVASH";

/// Une lecture plus grosse que ça est découpée : le serveur redemande la
/// suite. Borné, comme tout ce qui vient du serveur.
const LECTURE_MAX: u32 = 4 << 20;

/// Statuts NT que le paquet ne nomme pas (MS-ERREF).
const STATUS_OBJECT_NAME_INVALID: u32 = 0xC000_0033;
const STATUS_OBJECT_PATH_NOT_FOUND: u32 = 0xC000_003A;
const STATUS_INVALID_HANDLE: u32 = 0xC000_0008;
const STATUS_FILE_IS_A_DIRECTORY: u32 = 0xC000_00BA;

/// `FILE_CREATED` : le paquet ne connaît que `FILE_SUPERSEDED`, `FILE_OPENED`
/// et `FILE_OVERWRITTEN`, mais un serveur Windows attend cette valeur quand la
/// création a eu lieu (c'est ce que FreeRDP répond, drive_main.c).
const FILE_CREATED: u8 = 0x02;

/// `FILE_DEVICE_DISK` (MS-FSCC 2.5.10).
const FILE_DEVICE_DISK: u32 = 0x0000_0007;

/// Le gestionnaire que le canal appelle : il ne fait que transmettre au fil
/// du lecteur, et ne répond jamais dans l'appel.
#[derive(Debug)]
pub(crate) struct DisqueBackend {
    tx: std::sync::mpsc::Sender<ServerDriveIoRequest>,
}

ironrdp::core::impl_as_any!(DisqueBackend);

impl RdpdrBackend for DisqueBackend {
    fn handle_server_device_announce_response(
        &mut self,
        pdu: ServerDeviceAnnounceResponse,
    ) -> PduResult<()> {
        if pdu.result_code == NtStatus::SUCCESS {
            eprintln!("lecteur : « {LECTEUR_NOM} » accepté par le serveur");
        } else {
            eprintln!("lecteur : refusé par le serveur ({:?})", pdu.result_code);
        }
        Ok(())
    }

    fn handle_scard_call(
        &mut self,
        _req: DeviceControlRequest<ScardIoCtlCode>,
        _call: ScardCall,
    ) -> PduResult<()> {
        // Aucune carte à puce annoncée : le serveur n'a pas à en parler.
        Ok(())
    }

    fn handle_drive_io_request(&mut self, req: ServerDriveIoRequest) -> PduResult<Vec<SvcMessage>> {
        // Le fil a pu s'arrêter (racine disparue) : la requête reste alors sans
        // réponse, ce que le serveur traite comme un lecteur qui ne répond
        // plus, sans faire tomber la session.
        let _ = self.tx.send(req);
        Ok(Vec::new())
    }
}

/// Démarre le fil du lecteur sur `racine` et rend le gestionnaire à donner au
/// canal. Les réponses partent sur `reponses`, que la boucle de session écrit
/// sur le canal.
pub(crate) fn demarrer(
    racine: &Path,
    reponses: tokio::sync::mpsc::UnboundedSender<RdpdrPdu>,
) -> anyhow::Result<DisqueBackend> {
    let lecteur = Lecteur::nouveau(racine)?;
    let (tx, rx) = std::sync::mpsc::channel::<ServerDriveIoRequest>();
    std::thread::Builder::new()
        .name("lecteur-rdpdr".to_owned())
        .spawn(move || {
            let mut lecteur = lecteur;
            while let Ok(req) = rx.recv() {
                for pdu in lecteur.traiter(req) {
                    if reponses.send(pdu).is_err() {
                        return;
                    }
                }
            }
        })
        .map_err(|e| anyhow::anyhow!("fil du lecteur : {e}"))?;
    Ok(DisqueBackend { tx })
}

/// Un fichier ou un dossier ouvert par le serveur, désigné par son `file_id`.
struct Ouvert {
    /// Chemin réel sur le poste, sous la racine.
    chemin: PathBuf,
    /// Le fichier lui-même ; `None` pour un dossier.
    fichier: Option<std::fs::File>,
    repertoire: bool,
    /// `FileDispositionInformation` : à supprimer quand le serveur fermera.
    supprimer: bool,
    /// Énumération en cours (`IRP_MN_QUERY_DIRECTORY`), entrée par entrée.
    entrees: Vec<Entree>,
    position: usize,
}

/// Une entrée d'énumération, métadonnées déjà lues.
struct Entree {
    nom: String,
    meta: std::fs::Metadata,
}

/// L'état du lecteur : la racine et les fichiers ouverts.
pub(crate) struct Lecteur {
    racine: PathBuf,
    ouverts: HashMap<u32, Ouvert>,
    prochain: u32,
}

impl Lecteur {
    /// `racine` doit être un dossier ; elle est résolue une fois pour toutes
    /// (liens compris), c'est contre elle que chaque chemin sera jugé.
    pub(crate) fn nouveau(racine: &Path) -> anyhow::Result<Self> {
        let racine = std::fs::canonicalize(racine)
            .map_err(|e| anyhow::anyhow!("dossier partagé {} : {e}", racine.display()))?;
        anyhow::ensure!(
            racine.is_dir(),
            "dossier partagé {} : ce n'est pas un dossier",
            racine.display()
        );
        Ok(Self {
            racine,
            ouverts: HashMap::new(),
            prochain: 1,
        })
    }

    /// Traite une requête du serveur et rend les réponses à lui écrire.
    pub(crate) fn traiter(&mut self, req: ServerDriveIoRequest) -> Vec<RdpdrPdu> {
        match req {
            ServerDriveIoRequest::ServerCreateDriveRequest(r) => vec![self.creer(r)],
            ServerDriveIoRequest::DeviceCloseRequest(r) => vec![self.fermer(r)],
            ServerDriveIoRequest::DeviceReadRequest(r) => vec![self.lire(r)],
            ServerDriveIoRequest::DeviceWriteRequest(r) => vec![self.ecrire(r)],
            ServerDriveIoRequest::ServerDriveQueryInformationRequest(r) => {
                vec![self.informations(r)]
            }
            ServerDriveIoRequest::ServerDriveQueryDirectoryRequest(r) => vec![self.enumerer(r)],
            ServerDriveIoRequest::ServerDriveQueryVolumeInformationRequest(r) => {
                vec![self.volume(r)]
            }
            ServerDriveIoRequest::ServerDriveSetInformationRequest(r) => {
                self.modifier(r).into_iter().collect()
            }
            ServerDriveIoRequest::ServerDriveNotifyChangeDirectoryRequest(r) => {
                vec![Self::surveillance(r)]
            }
            ServerDriveIoRequest::ServerDriveLockControlRequest(r) => vec![Self::verrou(r)],
            ServerDriveIoRequest::DeviceControlRequest(r) => {
                // Aucun contrôle de périphérique n'a de sens pour un dossier :
                // succès sans tampon de sortie, comme FreeRDP.
                vec![DeviceControlResponse::new(r, NtStatus::SUCCESS, None).into()]
            }
        }
    }

    /// Ramène un chemin du serveur (`\dossier\fichier`) sous la racine.
    ///
    /// Le parent est résolu par le système (liens compris) et doit rester sous
    /// la racine ; le dernier composant, lui, n'est pas suivi s'il est un lien :
    /// un lien vers `/etc/shadow` déposé dans le dossier partagé ne donne rien.
    fn resoudre(&self, distant: &str) -> Result<PathBuf, NtStatus> {
        let composants: Vec<&str> = distant
            .split(['\\', '/'])
            .filter(|c| !c.is_empty())
            .collect();
        if composants
            .iter()
            .any(|c| *c == "." || *c == ".." || c.contains('\0'))
        {
            return Err(NtStatus::from(STATUS_OBJECT_NAME_INVALID));
        }
        let Some((dernier, parents)) = composants.split_last() else {
            return Ok(self.racine.clone());
        };
        let mut parent = self.racine.clone();
        parent.extend(parents);
        let parent = std::fs::canonicalize(&parent)
            .map_err(|_| NtStatus::from(STATUS_OBJECT_PATH_NOT_FOUND))?;
        if !parent.starts_with(&self.racine) {
            return Err(NtStatus::ACCESS_DENIED);
        }
        let chemin = parent.join(dernier);
        if std::fs::symlink_metadata(&chemin).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(NtStatus::ACCESS_DENIED);
        }
        Ok(chemin)
    }

    fn ouvert(&mut self, file_id: u32) -> Result<&mut Ouvert, NtStatus> {
        self.ouverts
            .get_mut(&file_id)
            .ok_or_else(|| NtStatus::from(STATUS_INVALID_HANDLE))
    }

    fn creer(&mut self, req: DeviceCreateRequest) -> RdpdrPdu {
        let io = req.device_io_request.clone();
        let (statut, file_id, information) = match self.ouvrir(&req) {
            Ok((id, info)) => (NtStatus::SUCCESS, id, info),
            Err(s) => (s, 0, Information::FILE_OPENED),
        };
        DeviceCreateResponse {
            device_io_reply: DeviceIoResponse::new(io, statut),
            file_id,
            information,
        }
        .into()
    }

    /// `IRP_MJ_CREATE` : ouvre ou crée, selon la disposition demandée.
    fn ouvrir(&mut self, req: &DeviceCreateRequest) -> Result<(u32, Information), NtStatus> {
        let chemin = self.resoudre(&req.path)?;
        let existant = std::fs::metadata(&chemin).ok();
        let veut_dossier = req
            .create_options
            .contains(CreateOptions::FILE_DIRECTORY_FILE);
        let ecriture = req.desired_access.intersects(
            DesiredAccess::GENERIC_WRITE
                | DesiredAccess::GENERIC_ALL
                | DesiredAccess::FILE_WRITE_DATA_OR_FILE_ADD_FILE
                | DesiredAccess::FILE_APPEND_DATA_OR_FILE_ADD_SUBDIRECTORY
                | DesiredAccess::MAXIMUM_ALLOWED,
        );
        let existe = existant.is_some();
        let est_dossier = existant.as_ref().is_some_and(std::fs::Metadata::is_dir);
        let disposition = req.create_disposition;

        // Un dossier : rien à tronquer, rien à ouvrir, il existe ou on le crée.
        if veut_dossier || (existe && est_dossier) {
            let information = match disposition {
                CreateDisposition::FILE_OPEN => {
                    if !existe {
                        return Err(NtStatus::NO_SUCH_FILE);
                    }
                    if !est_dossier {
                        return Err(NtStatus::NOT_A_DIRECTORY);
                    }
                    Information::FILE_OPENED
                }
                CreateDisposition::FILE_CREATE => {
                    if existe {
                        return Err(NtStatus::OBJECT_NAME_COLLISION);
                    }
                    std::fs::create_dir(&chemin).map_err(statut_io)?;
                    Information::from_bits_retain(FILE_CREATED)
                }
                CreateDisposition::FILE_OPEN_IF => {
                    if existe {
                        if !est_dossier {
                            return Err(NtStatus::NOT_A_DIRECTORY);
                        }
                        Information::FILE_OPENED
                    } else {
                        std::fs::create_dir(&chemin).map_err(statut_io)?;
                        Information::from_bits_retain(FILE_CREATED)
                    }
                }
                _ => return Err(NtStatus::from(STATUS_FILE_IS_A_DIRECTORY)),
            };
            return Ok((
                self.enregistrer(Ouvert {
                    chemin,
                    fichier: None,
                    repertoire: true,
                    supprimer: false,
                    entrees: Vec::new(),
                    position: 0,
                }),
                information,
            ));
        }

        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(ecriture);
        let information = match disposition {
            CreateDisposition::FILE_OPEN => {
                if !existe {
                    return Err(NtStatus::NO_SUCH_FILE);
                }
                Information::FILE_OPENED
            }
            CreateDisposition::FILE_CREATE => {
                if existe {
                    return Err(NtStatus::OBJECT_NAME_COLLISION);
                }
                options.write(true).create_new(true);
                Information::from_bits_retain(FILE_CREATED)
            }
            CreateDisposition::FILE_OPEN_IF => {
                if existe {
                    Information::FILE_OPENED
                } else {
                    options.write(true).create(true);
                    Information::from_bits_retain(FILE_CREATED)
                }
            }
            CreateDisposition::FILE_OVERWRITE => {
                if !existe {
                    return Err(NtStatus::NO_SUCH_FILE);
                }
                options.write(true).truncate(true);
                Information::FILE_OVERWRITTEN
            }
            CreateDisposition::FILE_OVERWRITE_IF => {
                options.write(true).create(true).truncate(true);
                if existe {
                    Information::FILE_OVERWRITTEN
                } else {
                    Information::from_bits_retain(FILE_CREATED)
                }
            }
            CreateDisposition::FILE_SUPERSEDE => {
                options.write(true).create(true).truncate(true);
                Information::FILE_SUPERSEDED
            }
            _ => return Err(NtStatus::from(STATUS_OBJECT_NAME_INVALID)),
        };
        // Un fichier qu'on ne peut pas ouvrir en écriture s'ouvre en lecture
        // quand le serveur ne demandait que des attributs : l'explorateur
        // interroge chaque fichier avant de l'afficher.
        let fichier = match options.open(&chemin) {
            Ok(f) => f,
            Err(e) if ecriture && e.kind() == std::io::ErrorKind::PermissionDenied => {
                return Err(NtStatus::ACCESS_DENIED);
            }
            Err(e) => return Err(statut_io(e)),
        };
        Ok((
            self.enregistrer(Ouvert {
                chemin,
                fichier: Some(fichier),
                repertoire: false,
                supprimer: false,
                entrees: Vec::new(),
                position: 0,
            }),
            information,
        ))
    }

    fn enregistrer(&mut self, o: Ouvert) -> u32 {
        // Zéro n'est jamais un identifiant : c'est celui des échecs.
        let mut id = self.prochain;
        while id == 0 || self.ouverts.contains_key(&id) {
            id = id.wrapping_add(1);
        }
        self.prochain = id.wrapping_add(1);
        self.ouverts.insert(id, o);
        id
    }

    fn fermer(&mut self, req: DeviceCloseRequest) -> RdpdrPdu {
        let io = req.device_io_request;
        let statut = match self.ouverts.remove(&io.file_id) {
            Some(o) => {
                // Le fichier est lâché AVANT la suppression : Windows (le poste,
                // ici) refuse de supprimer un fichier ouvert.
                let Ouvert {
                    chemin,
                    fichier,
                    repertoire,
                    supprimer,
                    ..
                } = o;
                drop(fichier);
                if supprimer {
                    let issue = if repertoire {
                        std::fs::remove_dir(&chemin)
                    } else {
                        std::fs::remove_file(&chemin)
                    };
                    issue.map_or_else(statut_io, |()| NtStatus::SUCCESS)
                } else {
                    NtStatus::SUCCESS
                }
            }
            None => NtStatus::from(STATUS_INVALID_HANDLE),
        };
        DeviceCloseResponse {
            device_io_response: DeviceIoResponse::new(io, statut),
        }
        .into()
    }

    fn lire(&mut self, req: DeviceReadRequest) -> RdpdrPdu {
        let io = req.device_io_request.clone();
        let (statut, read_data) = match self.lire_bloc(&req) {
            Ok(d) => (NtStatus::SUCCESS, d),
            Err(s) => (s, Vec::new()),
        };
        DeviceReadResponse {
            device_io_reply: DeviceIoResponse::new(io, statut),
            read_data,
        }
        .into()
    }

    fn lire_bloc(&mut self, req: &DeviceReadRequest) -> Result<Vec<u8>, NtStatus> {
        let o = self.ouvert(req.device_io_request.file_id)?;
        let Some(f) = o.fichier.as_mut() else {
            return Err(NtStatus::from(STATUS_FILE_IS_A_DIRECTORY));
        };
        f.seek(SeekFrom::Start(req.offset)).map_err(statut_io)?;
        let voulu = req.length.min(LECTURE_MAX) as usize;
        let mut tampon = vec![0u8; voulu];
        let mut lu = 0;
        // Jusqu'au compte ou à la fin du fichier : le redirecteur de Windows
        // prend une lecture courte pour la fin du fichier.
        while lu < voulu {
            match f.read(&mut tampon[lu..]) {
                Ok(0) => break,
                Ok(n) => lu += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(statut_io(e)),
            }
        }
        // À la fin du fichier, un succès sans octet, comme `ReadFile` et donc
        // comme FreeRDP : c'est ce que le redirecteur de Windows sait lire.
        tampon.truncate(lu);
        Ok(tampon)
    }

    fn ecrire(&mut self, req: DeviceWriteRequest) -> RdpdrPdu {
        let io = req.device_io_request.clone();
        let statut = match self.ecrire_bloc(&req) {
            Ok(()) => NtStatus::SUCCESS,
            Err(s) => s,
        };
        let length = if statut == NtStatus::SUCCESS {
            u32::try_from(req.write_data.len()).unwrap_or(u32::MAX)
        } else {
            0
        };
        DeviceWriteResponse {
            device_io_reply: DeviceIoResponse::new(io, statut),
            length,
        }
        .into()
    }

    fn ecrire_bloc(&mut self, req: &DeviceWriteRequest) -> Result<(), NtStatus> {
        let o = self.ouvert(req.device_io_request.file_id)?;
        let Some(f) = o.fichier.as_mut() else {
            return Err(NtStatus::from(STATUS_FILE_IS_A_DIRECTORY));
        };
        f.seek(SeekFrom::Start(req.offset)).map_err(statut_io)?;
        f.write_all(&req.write_data).map_err(statut_io)
    }

    fn informations(&mut self, req: ServerDriveQueryInformationRequest) -> RdpdrPdu {
        let io = req.device_io_request.clone();
        let (statut, buffer) = match self.informations_de(&req) {
            Ok(b) => (NtStatus::SUCCESS, Some(b)),
            Err(s) => (s, None),
        };
        ClientDriveQueryInformationResponse {
            device_io_response: DeviceIoResponse::new(io, statut),
            buffer,
        }
        .into()
    }

    fn informations_de(
        &mut self,
        req: &ServerDriveQueryInformationRequest,
    ) -> Result<FileInformationClass, NtStatus> {
        let o = self.ouvert(req.device_io_request.file_id)?;
        let meta = std::fs::metadata(&o.chemin).map_err(statut_io)?;
        let nom = nom_de(&o.chemin);
        let classe = match req.file_info_class_lvl {
            FileInformationClassLevel::FILE_BASIC_INFORMATION => {
                FileInformationClass::Basic(basique(&meta, &nom))
            }
            FileInformationClassLevel::FILE_STANDARD_INFORMATION => {
                FileInformationClass::Standard(FileStandardInformation {
                    allocation_size: allocation(&meta),
                    end_of_file: taille(&meta),
                    number_of_links: 1,
                    delete_pending: booleen(o.supprimer),
                    directory: booleen(meta.is_dir()),
                })
            }
            FileInformationClassLevel::FILE_ATTRIBUTE_TAG_INFORMATION => {
                FileInformationClass::AttributeTag(FileAttributeTagInformation {
                    file_attributes: attributs(&meta, &nom),
                    reparse_tag: 0,
                })
            }
            _ => return Err(NtStatus::NOT_SUPPORTED),
        };
        Ok(classe)
    }

    fn enumerer(&mut self, req: ServerDriveQueryDirectoryRequest) -> RdpdrPdu {
        let io = req.device_io_request.clone();
        let (statut, buffer) = match self.entree_suivante(&req) {
            Ok(b) => (NtStatus::SUCCESS, Some(b)),
            Err(s) => (s, None),
        };
        ClientDriveQueryDirectoryResponse {
            device_io_reply: DeviceIoResponse::new(io, statut),
            buffer,
        }
        .into()
    }

    /// `IRP_MN_QUERY_DIRECTORY` : une entrée par requête, comme FreeRDP. La
    /// première requête (`initial_query`) fixe le motif ; `.` et `..` ouvrent
    /// la liste quand le motif les prend, c'est ce que l'explorateur attend.
    fn entree_suivante(
        &mut self,
        req: &ServerDriveQueryDirectoryRequest,
    ) -> Result<FileInformationClass, NtStatus> {
        let o = self.ouvert(req.device_io_request.file_id)?;
        if !o.repertoire {
            return Err(NtStatus::NOT_A_DIRECTORY);
        }
        if req.initial_query != 0 {
            let motif = req
                .path
                .rsplit(['\\', '/'])
                .next()
                .filter(|m| !m.is_empty())
                .unwrap_or("*");
            let mut entrees = Vec::new();
            let meta_dossier = std::fs::metadata(&o.chemin).map_err(statut_io)?;
            for special in [".", ".."] {
                if correspond(motif, special) {
                    entrees.push(Entree {
                        nom: special.to_owned(),
                        meta: meta_dossier.clone(),
                    });
                }
            }
            let mut fichiers: Vec<Entree> = std::fs::read_dir(&o.chemin)
                .map_err(statut_io)?
                .filter_map(Result::ok)
                .filter_map(|e| {
                    let nom = e.file_name().to_string_lossy().into_owned();
                    // Un lien est décrit tel quel (sans le suivre) : lister
                    // n'expose rien, l'ouverture, elle, le refusera.
                    let meta = e.metadata().ok()?;
                    correspond(motif, &nom).then_some(Entree { nom, meta })
                })
                .collect();
            fichiers.sort_by(|a, b| a.nom.cmp(&b.nom));
            entrees.extend(fichiers);
            if entrees.is_empty() {
                return Err(NtStatus::NO_SUCH_FILE);
            }
            o.entrees = entrees;
            o.position = 0;
        }
        let Some(e) = o.entrees.get(o.position) else {
            return Err(NtStatus::NO_MORE_FILES);
        };
        o.position += 1;
        let (m, nom) = (&e.meta, e.nom.clone());
        let attrs = attributs(m, &nom);
        let classe = match req.file_info_class_lvl {
            FileInformationClassLevel::FILE_BOTH_DIRECTORY_INFORMATION => {
                FileInformationClass::BothDirectory(FileBothDirectoryInformation::new(
                    creation(m),
                    acces(m),
                    modification(m),
                    modification(m),
                    taille(m),
                    attrs,
                    nom,
                ))
            }
            FileInformationClassLevel::FILE_FULL_DIRECTORY_INFORMATION => {
                FileInformationClass::FullDirectory(FileFullDirectoryInformation::new(
                    creation(m),
                    acces(m),
                    modification(m),
                    modification(m),
                    taille(m),
                    attrs,
                    nom,
                ))
            }
            FileInformationClassLevel::FILE_DIRECTORY_INFORMATION => {
                FileInformationClass::Directory(FileDirectoryInformation::new(
                    creation(m),
                    acces(m),
                    modification(m),
                    modification(m),
                    taille(m),
                    attrs,
                    nom,
                ))
            }
            FileInformationClassLevel::FILE_NAMES_INFORMATION => {
                FileInformationClass::Names(FileNamesInformation::new(nom))
            }
            _ => return Err(NtStatus::NOT_SUPPORTED),
        };
        Ok(classe)
    }

    fn volume(&mut self, req: ServerDriveQueryVolumeInformationRequest) -> RdpdrPdu {
        let io = req.device_io_request.clone();
        let (statut, buffer) = match self.volume_de(&req) {
            Ok(b) => (NtStatus::SUCCESS, Some(b)),
            Err(s) => (s, None),
        };
        ClientDriveQueryVolumeInformationResponse::new(io, statut, buffer).into()
    }

    fn volume_de(
        &mut self,
        req: &ServerDriveQueryVolumeInformationRequest,
    ) -> Result<FileSystemInformationClass, NtStatus> {
        self.ouvert(req.device_io_request.file_id)?;
        let espace = espace_disque(&self.racine);
        let classe = match req.fs_info_class_lvl {
            FileSystemInformationClassLevel::FILE_FS_VOLUME_INFORMATION => {
                let meta = std::fs::metadata(&self.racine).map_err(statut_io)?;
                FileSystemInformationClass::FileFsVolumeInformation(FileFsVolumeInformation {
                    volume_creation_time: creation(&meta),
                    volume_serial_number: serie_du_volume(&self.racine),
                    supports_objects: Boolean::False,
                    volume_label: ETIQUETTE_VOLUME.to_owned(),
                })
            }
            FileSystemInformationClassLevel::FILE_FS_SIZE_INFORMATION => {
                FileSystemInformationClass::FileFsSizeInformation(FileFsSizeInformation {
                    total_alloc_units: espace.total_unites,
                    available_alloc_units: espace.libres_unites,
                    sectors_per_alloc_unit: espace.secteurs_par_unite,
                    bytes_per_sector: espace.octets_par_secteur,
                })
            }
            FileSystemInformationClassLevel::FILE_FS_FULL_SIZE_INFORMATION => {
                FileSystemInformationClass::FileFsFullSizeInformation(FileFsFullSizeInformation {
                    total_alloc_units: espace.total_unites,
                    caller_available_alloc_units: espace.libres_unites,
                    actual_available_alloc_units: espace.libres_unites,
                    sectors_per_alloc_unit: espace.secteurs_par_unite,
                    bytes_per_sector: espace.octets_par_secteur,
                })
            }
            FileSystemInformationClassLevel::FILE_FS_ATTRIBUTE_INFORMATION => {
                // NTFS, pas FAT32 : l'explorateur refuse de copier plus de
                // quatre gigaoctets vers ce qu'il croit être du FAT32.
                FileSystemInformationClass::FileFsAttributeInformation(FileFsAttributeInformation {
                    file_system_attributes: FileSystemAttributes::FILE_CASE_SENSITIVE_SEARCH
                        | FileSystemAttributes::FILE_CASE_PRESERVED_NAMES
                        | FileSystemAttributes::FILE_UNICODE_ON_DISK,
                    max_component_name_len: 255,
                    file_system_name: "NTFS".to_owned(),
                })
            }
            FileSystemInformationClassLevel::FILE_FS_DEVICE_INFORMATION => {
                FileSystemInformationClass::FileFsDeviceInformation(FileFsDeviceInformation {
                    device_type: FILE_DEVICE_DISK,
                    characteristics: Characteristics::FILE_REMOTE_DEVICE,
                })
            }
            _ => return Err(NtStatus::NOT_SUPPORTED),
        };
        Ok(classe)
    }

    fn modifier(&mut self, req: ServerDriveSetInformationRequest) -> Option<RdpdrPdu> {
        let statut = match self.modifier_selon(&req) {
            Ok(()) => NtStatus::SUCCESS,
            Err(s) => s,
        };
        ClientDriveSetInformationResponse::new(&req, statut)
            .ok()
            .map(Into::into)
    }

    fn modifier_selon(&mut self, req: &ServerDriveSetInformationRequest) -> Result<(), NtStatus> {
        let file_id = req.device_io_request.file_id;
        match &req.set_buffer {
            FileInformationClass::Basic(b) => {
                let o = self.ouvert(file_id)?;
                // Seuls les attributs sont honorés : lecture seule posée ou
                // retirée. Les dates que Windows pousse à chaque copie n'ont
                // pas à réécrire celles du poste.
                let lecture_seule = b
                    .file_attributes
                    .contains(FileAttributes::FILE_ATTRIBUTE_READONLY);
                if !b.file_attributes.is_empty() {
                    if let Ok(meta) = std::fs::metadata(&o.chemin) {
                        let mut droits = meta.permissions();
                        if droits.readonly() != lecture_seule {
                            #[allow(clippy::permissions_set_readonly_false)]
                            droits.set_readonly(lecture_seule);
                            let _ = std::fs::set_permissions(&o.chemin, droits);
                        }
                    }
                }
                Ok(())
            }
            FileInformationClass::EndOfFile(e) => {
                let o = self.ouvert(file_id)?;
                let f = o
                    .fichier
                    .as_mut()
                    .ok_or_else(|| NtStatus::from(STATUS_FILE_IS_A_DIRECTORY))?;
                let longueur = u64::try_from(e.end_of_file)
                    .map_err(|_| NtStatus::from(STATUS_OBJECT_NAME_INVALID))?;
                f.set_len(longueur).map_err(statut_io)
            }
            FileInformationClass::Allocation(a) => {
                let o = self.ouvert(file_id)?;
                let f = o
                    .fichier
                    .as_mut()
                    .ok_or_else(|| NtStatus::from(STATUS_FILE_IS_A_DIRECTORY))?;
                let longueur = u64::try_from(a.allocation_size)
                    .map_err(|_| NtStatus::from(STATUS_OBJECT_NAME_INVALID))?;
                // L'allocation ne rétrécit jamais le contenu : Windows la pose
                // avant d'écrire, le fichier grandit ensuite de lui-même.
                let actuelle = f.metadata().map_err(statut_io)?.len();
                if longueur < actuelle {
                    f.set_len(longueur).map_err(statut_io)?;
                }
                Ok(())
            }
            FileInformationClass::Disposition(d) => {
                let o = self.ouvert(file_id)?;
                let supprimer = d.delete_pending != 0;
                if supprimer && o.repertoire {
                    let vide = std::fs::read_dir(&o.chemin)
                        .map_err(statut_io)?
                        .next()
                        .is_none();
                    if !vide {
                        return Err(NtStatus::DIRECTORY_NOT_EMPTY);
                    }
                }
                o.supprimer = supprimer;
                Ok(())
            }
            FileInformationClass::Rename(r) => {
                let destination = self.resoudre(&r.file_name)?;
                let o = self.ouvert(file_id)?;
                if destination.exists() && r.replace_if_exists == Boolean::False {
                    return Err(NtStatus::OBJECT_NAME_COLLISION);
                }
                if destination == o.chemin {
                    return Ok(());
                }
                std::fs::rename(&o.chemin, &destination).map_err(statut_io)?;
                o.chemin = destination;
                Ok(())
            }
            _ => Err(NtStatus::NOT_SUPPORTED),
        }
    }

    /// `IRP_MN_NOTIFY_CHANGE_DIRECTORY` : pas de surveillance, dit franchement
    /// (l'explorateur se rabat alors sur ses propres rafraîchissements).
    fn surveillance(req: ServerDriveNotifyChangeDirectoryRequest) -> RdpdrPdu {
        DeviceCloseResponse {
            device_io_response: DeviceIoResponse::new(
                req.device_io_request,
                NtStatus::NOT_SUPPORTED,
            ),
        }
        .into()
    }

    /// `IRP_MJ_LOCK_CONTROL` : aucun verrou n'est tenu, tout est accordé,
    /// comme FreeRDP. La réponse a la forme d'une complétion sans corps.
    fn verrou(req: ServerDriveLockControlRequest) -> RdpdrPdu {
        DeviceCloseResponse {
            device_io_response: DeviceIoResponse::new(req.device_io_request, NtStatus::SUCCESS),
        }
        .into()
    }
}

/// Traduit une erreur du poste en statut NT, comme FreeRDP (drive_file.c).
fn statut_io(e: std::io::Error) -> NtStatus {
    use std::io::ErrorKind as K;
    match e.kind() {
        K::NotFound => NtStatus::NO_SUCH_FILE,
        K::PermissionDenied => NtStatus::ACCESS_DENIED,
        K::AlreadyExists => NtStatus::OBJECT_NAME_COLLISION,
        K::NotADirectory => NtStatus::NOT_A_DIRECTORY,
        K::IsADirectory => NtStatus::from(STATUS_FILE_IS_A_DIRECTORY),
        K::DirectoryNotEmpty => NtStatus::DIRECTORY_NOT_EMPTY,
        _ => NtStatus::UNSUCCESSFUL,
    }
}

fn booleen(b: bool) -> Boolean {
    if b {
        Boolean::True
    } else {
        Boolean::False
    }
}

fn nom_de(chemin: &Path) -> String {
    chemin
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Heure Windows : centaines de nanosecondes depuis le 1er janvier 1601.
fn heure_windows(t: std::io::Result<std::time::SystemTime>) -> i64 {
    const ECART_1601_1970: i64 = 11_644_473_600;
    let Ok(t) = t else { return 0 };
    match t.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => (i64::try_from(d.as_secs()).unwrap_or(i64::MAX / 10_000_000) + ECART_1601_1970)
            .saturating_mul(10_000_000)
            .saturating_add(i64::from(d.subsec_nanos() / 100)),
        Err(_) => 0,
    }
}

fn creation(m: &std::fs::Metadata) -> i64 {
    // Sans date de création (ext4 sans statx, réseau), celle de modification
    // vaut mieux qu'un zéro que l'explorateur affiche en 1601.
    let c = heure_windows(m.created());
    if c == 0 {
        modification(m)
    } else {
        c
    }
}

fn modification(m: &std::fs::Metadata) -> i64 {
    heure_windows(m.modified())
}

fn acces(m: &std::fs::Metadata) -> i64 {
    heure_windows(m.accessed())
}

fn taille(m: &std::fs::Metadata) -> i64 {
    if m.is_dir() {
        0
    } else {
        i64::try_from(m.len()).unwrap_or(i64::MAX)
    }
}

/// Taille allouée : arrondie au bloc de 4 Kio, ce que Windows affiche comme
/// « taille sur le disque ».
fn allocation(m: &std::fs::Metadata) -> i64 {
    let t = taille(m);
    t.saturating_add(4095) / 4096 * 4096
}

fn basique(m: &std::fs::Metadata, nom: &str) -> FileBasicInformation {
    FileBasicInformation {
        creation_time: creation(m),
        last_access_time: acces(m),
        last_write_time: modification(m),
        change_time: modification(m),
        file_attributes: attributs(m, nom),
    }
}

/// Attributs Windows d'une entrée du poste : dossier ou archive, lecture
/// seule d'après les droits, caché pour les fichiers à point (convention
/// Unix, que l'explorateur applique alors comme sur un partage Samba).
fn attributs(m: &std::fs::Metadata, nom: &str) -> FileAttributes {
    let mut a = if m.is_dir() {
        FileAttributes::FILE_ATTRIBUTE_DIRECTORY
    } else {
        FileAttributes::FILE_ATTRIBUTE_ARCHIVE
    };
    if m.permissions().readonly() {
        a |= FileAttributes::FILE_ATTRIBUTE_READONLY;
    }
    if nom.starts_with('.') && nom != "." && nom != ".." {
        a |= FileAttributes::FILE_ATTRIBUTE_HIDDEN;
    }
    a
}

/// Correspondance de motif DOS (`*`, `?`), sans tenir compte de la casse :
/// c'est ainsi que le serveur Windows filtre une énumération.
pub(crate) fn correspond(motif: &str, nom: &str) -> bool {
    fn rec(m: &[char], n: &[char]) -> bool {
        match (m.first(), n.first()) {
            (None, None) => true,
            (Some('*'), _) => rec(&m[1..], n) || (!n.is_empty() && rec(m, &n[1..])),
            (Some('?'), Some(_)) => rec(&m[1..], &n[1..]),
            (Some(a), Some(b)) => a.eq_ignore_ascii_case(b) && rec(&m[1..], &n[1..]),
            _ => false,
        }
    }
    let m: Vec<char> = motif.to_lowercase().chars().collect();
    let n: Vec<char> = nom.to_lowercase().chars().collect();
    rec(&m, &n)
}

/// Numéro de série du volume : dérivé de la racine, stable d'une session à
/// l'autre (Windows s'en sert pour reconnaître un lecteur).
fn serie_du_volume(racine: &Path) -> u32 {
    use std::hash::{Hash as _, Hasher as _};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    racine.hash(&mut h);
    u32::try_from(h.finish() & 0xFFFF_FFFF).unwrap_or(0x4156_4148)
}

/// Ce que le système dit de l'espace du volume qui porte la racine.
struct Espace {
    total_unites: i64,
    libres_unites: i64,
    secteurs_par_unite: u32,
    octets_par_secteur: u32,
}

// Les champs de `statvfs` n'ont pas le même type d'une plateforme à l'autre
// (u64 sous Linux, u32 sous macOS pour les compteurs de blocs) : la
// conversion est nécessaire là, superflue ici, et clippy ne voit qu'ici.
#[cfg(unix)]
#[allow(clippy::useless_conversion)]
fn espace_disque(racine: &Path) -> Espace {
    use std::os::unix::ffi::OsStrExt as _;
    let chemin = std::ffi::CString::new(racine.as_os_str().as_bytes());
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    let ok = chemin
        .as_ref()
        .is_ok_and(|c| unsafe { libc::statvfs(c.as_ptr(), &raw mut s) } == 0);
    if !ok || s.f_frsize == 0 {
        return espace_inconnu();
    }
    // Unité d'allocation = bloc du système de fichiers, en secteurs de 512.
    let bloc = u64::from(s.f_frsize);
    Espace {
        total_unites: i64::try_from(u64::from(s.f_blocks)).unwrap_or(i64::MAX),
        libres_unites: i64::try_from(u64::from(s.f_bavail)).unwrap_or(i64::MAX),
        secteurs_par_unite: u32::try_from(bloc / 512).unwrap_or(8).max(1),
        octets_par_secteur: 512,
    }
}

#[cfg(windows)]
fn espace_disque(racine: &Path) -> Espace {
    use std::os::windows::ffi::OsStrExt as _;
    let mut chemin: Vec<u16> = racine.as_os_str().encode_wide().collect();
    chemin.push(0);
    let (mut libres, mut total, mut libres_total) = (0u64, 0u64, 0u64);
    let ok = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            chemin.as_ptr(),
            &raw mut libres,
            &raw mut total,
            &raw mut libres_total,
        )
    } != 0;
    if !ok {
        return espace_inconnu();
    }
    Espace {
        total_unites: i64::try_from(total / 4096).unwrap_or(i64::MAX),
        libres_unites: i64::try_from(libres / 4096).unwrap_or(i64::MAX),
        secteurs_par_unite: 8,
        octets_par_secteur: 512,
    }
}

/// Quand le système ne répond pas : un volume d'un téraoctet à moitié libre,
/// plutôt que zéro, qui ferait refuser toute copie.
fn espace_inconnu() -> Espace {
    Espace {
        total_unites: 1 << 28,
        libres_unites: 1 << 27,
        secteurs_par_unite: 8,
        octets_par_secteur: 512,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironrdp::rdpdr::pdu::efs::{
        DeviceIoRequest, FileDispositionInformation, FileEndOfFileInformation,
        FileRenameInformation, MajorFunction, MinorFunction, SharedAccess,
    };

    fn bac(nom: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("avash-disque-{}-{nom}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn io(file_id: u32, completion_id: u32, majeure: MajorFunction) -> DeviceIoRequest {
        DeviceIoRequest {
            device_id: LECTEUR_ID,
            file_id,
            completion_id,
            major_function: majeure,
            minor_function: MinorFunction::from(0),
        }
    }

    fn demande_creation(
        chemin: &str,
        disposition: CreateDisposition,
        dossier: bool,
        ecriture: bool,
    ) -> ServerDriveIoRequest {
        let mut acces = DesiredAccess::GENERIC_READ;
        if ecriture {
            acces |= DesiredAccess::GENERIC_WRITE;
        }
        ServerDriveIoRequest::ServerCreateDriveRequest(DeviceCreateRequest {
            device_io_request: io(0, 1, MajorFunction::Create),
            desired_access: acces,
            allocation_size: 0,
            file_attributes: FileAttributes::empty(),
            shared_access: SharedAccess::empty(),
            create_disposition: disposition,
            create_options: if dossier {
                CreateOptions::FILE_DIRECTORY_FILE
            } else {
                CreateOptions::empty()
            },
            path: chemin.to_owned(),
        })
    }

    /// Ouvre et rend (statut, `file_id`, information).
    fn ouvrir(
        l: &mut Lecteur,
        chemin: &str,
        disposition: CreateDisposition,
        dossier: bool,
        ecriture: bool,
    ) -> (NtStatus, u32, u8) {
        let mut r = l.traiter(demande_creation(chemin, disposition, dossier, ecriture));
        match r.pop() {
            Some(RdpdrPdu::DeviceCreateResponse(c)) => {
                (c.device_io_reply.io_status, c.file_id, c.information.bits())
            }
            _ => panic!("une réponse de création"),
        }
    }

    fn fermer(l: &mut Lecteur, file_id: u32) -> NtStatus {
        let mut r = l.traiter(ServerDriveIoRequest::DeviceCloseRequest(
            DeviceCloseRequest {
                device_io_request: io(file_id, 2, MajorFunction::Close),
            },
        ));
        match r.pop() {
            Some(RdpdrPdu::DeviceCloseResponse(c)) => c.device_io_response.io_status,
            _ => panic!("une réponse de fermeture"),
        }
    }

    fn lire(l: &mut Lecteur, file_id: u32, offset: u64, length: u32) -> (NtStatus, Vec<u8>) {
        let mut r = l.traiter(ServerDriveIoRequest::DeviceReadRequest(DeviceReadRequest {
            device_io_request: io(file_id, 3, MajorFunction::Read),
            length,
            offset,
        }));
        match r.pop() {
            Some(RdpdrPdu::DeviceReadResponse(c)) => (c.device_io_reply.io_status, c.read_data),
            _ => panic!("une réponse de lecture"),
        }
    }

    fn ecrire(l: &mut Lecteur, file_id: u32, offset: u64, data: &[u8]) -> (NtStatus, u32) {
        let mut r = l.traiter(ServerDriveIoRequest::DeviceWriteRequest(
            DeviceWriteRequest {
                device_io_request: io(file_id, 4, MajorFunction::Write),
                offset,
                write_data: data.to_vec(),
            },
        ));
        match r.pop() {
            Some(RdpdrPdu::DeviceWriteResponse(c)) => (c.device_io_reply.io_status, c.length),
            _ => panic!("une réponse d'écriture"),
        }
    }

    fn enumerer(
        l: &mut Lecteur,
        file_id: u32,
        motif: &str,
        initial: bool,
    ) -> Result<String, NtStatus> {
        let mut r = l.traiter(ServerDriveIoRequest::ServerDriveQueryDirectoryRequest(
            ServerDriveQueryDirectoryRequest {
                device_io_request: io(file_id, 5, MajorFunction::DirectoryControl),
                file_info_class_lvl: FileInformationClassLevel::FILE_BOTH_DIRECTORY_INFORMATION,
                initial_query: u8::from(initial),
                path: motif.to_owned(),
            },
        ));
        match r.pop() {
            Some(RdpdrPdu::ClientDriveQueryDirectoryResponse(c)) => {
                if c.device_io_reply.io_status == NtStatus::SUCCESS {
                    match c.buffer {
                        Some(FileInformationClass::BothDirectory(b)) => Ok(b.file_name),
                        _ => panic!("une entrée BothDirectory"),
                    }
                } else {
                    Err(c.device_io_reply.io_status)
                }
            }
            _ => panic!("une réponse d'énumération"),
        }
    }

    fn modifier(l: &mut Lecteur, file_id: u32, classe: FileInformationClass) -> NtStatus {
        let mut r = l.traiter(ServerDriveIoRequest::ServerDriveSetInformationRequest(
            ServerDriveSetInformationRequest {
                device_io_request: io(file_id, 6, MajorFunction::SetInformation),
                set_buffer: classe,
            },
        ));
        match r.pop() {
            Some(RdpdrPdu::ClientDriveSetInformationResponse(_)) => NtStatus::SUCCESS,
            None => panic!("une réponse de modification"),
            _ => panic!("une réponse de modification"),
        }
    }

    /// `..`, un lien qui sort, un chemin absolu déguisé : rien ne passe.
    #[test]
    fn un_chemin_qui_sort_de_la_racine_est_refuse() {
        let d = bac("hors");
        std::fs::write(d.join("dedans.txt"), b"ok").unwrap();
        let mut l = Lecteur::nouveau(&d).unwrap();
        for mauvais in [
            "\\..\\dedans.txt",
            "\\a\\..\\..\\etc\\passwd",
            "\\.\\dedans.txt",
        ] {
            let (s, id, _) = ouvrir(&mut l, mauvais, CreateDisposition::FILE_OPEN, false, false);
            assert_ne!(s, NtStatus::SUCCESS, "{mauvais}");
            assert_eq!(id, 0);
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc", d.join("lien")).unwrap();
            let (s, _, _) = ouvrir(&mut l, "\\lien", CreateDisposition::FILE_OPEN, true, false);
            assert_eq!(
                s,
                NtStatus::ACCESS_DENIED,
                "un lien en dernier composant n'est pas suivi"
            );
            let (s, _, _) = ouvrir(
                &mut l,
                "\\lien\\hostname",
                CreateDisposition::FILE_OPEN,
                false,
                false,
            );
            assert_eq!(
                s,
                NtStatus::ACCESS_DENIED,
                "un lien traversé qui sort de la racine"
            );
        }
        // Ce qui est dedans passe, avec ou sans barre de tête, barres mêlées.
        let (s, id, info) = ouvrir(
            &mut l,
            "dedans.txt",
            CreateDisposition::FILE_OPEN,
            false,
            false,
        );
        assert_eq!(
            (s, info),
            (NtStatus::SUCCESS, Information::FILE_OPENED.bits())
        );
        assert_eq!(fermer(&mut l, id), NtStatus::SUCCESS);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Créer, écrire, fermer, rouvrir, lire : l'aller-retour complet, avec les
    /// informations que l'explorateur demande entre-temps.
    #[test]
    fn creer_ecrire_puis_relire_font_l_aller_retour() {
        let d = bac("aller-retour");
        let mut l = Lecteur::nouveau(&d).unwrap();
        let (s, id, info) = ouvrir(
            &mut l,
            "\\neuf.txt",
            CreateDisposition::FILE_CREATE,
            false,
            true,
        );
        assert_eq!((s, info), (NtStatus::SUCCESS, FILE_CREATED));
        assert_eq!(ecrire(&mut l, id, 0, b"bonjour "), (NtStatus::SUCCESS, 8));
        assert_eq!(ecrire(&mut l, id, 8, b"le monde"), (NtStatus::SUCCESS, 8));
        // Recréer par-dessus est refusé, rouvrir « si besoin » ouvre.
        let (s, _, _) = ouvrir(
            &mut l,
            "\\neuf.txt",
            CreateDisposition::FILE_CREATE,
            false,
            true,
        );
        assert_eq!(s, NtStatus::OBJECT_NAME_COLLISION);
        assert_eq!(fermer(&mut l, id), NtStatus::SUCCESS);

        let (s, id, info) = ouvrir(
            &mut l,
            "\\neuf.txt",
            CreateDisposition::FILE_OPEN_IF,
            false,
            false,
        );
        assert_eq!(
            (s, info),
            (NtStatus::SUCCESS, Information::FILE_OPENED.bits())
        );
        assert_eq!(
            lire(&mut l, id, 0, 7),
            (NtStatus::SUCCESS, b"bonjour".to_vec())
        );
        assert_eq!(
            lire(&mut l, id, 8, 100),
            (NtStatus::SUCCESS, b"le monde".to_vec())
        );
        assert_eq!(
            lire(&mut l, id, 16, 100),
            (NtStatus::SUCCESS, Vec::new()),
            "fin de fichier : succès sans octet"
        );
        // Les informations : standard dit la taille, basique les attributs.
        let mut r = l.traiter(ServerDriveIoRequest::ServerDriveQueryInformationRequest(
            ServerDriveQueryInformationRequest {
                device_io_request: io(id, 7, MajorFunction::QueryInformation),
                file_info_class_lvl: FileInformationClassLevel::FILE_STANDARD_INFORMATION,
            },
        ));
        match r.pop() {
            Some(RdpdrPdu::ClientDriveQueryInformationResponse(c)) => match c.buffer {
                Some(FileInformationClass::Standard(st)) => {
                    assert_eq!(st.end_of_file, 16);
                    assert_eq!(st.directory, Boolean::False);
                }
                _ => panic!("une information standard"),
            },
            _ => panic!("une réponse d'information"),
        }
        assert_eq!(fermer(&mut l, id), NtStatus::SUCCESS);
        // Un identifiant inconnu ne fait rien, et le dit.
        assert_eq!(fermer(&mut l, 99), NtStatus::from(STATUS_INVALID_HANDLE));
        assert_eq!(
            std::fs::read(d.join("neuf.txt")).unwrap(),
            b"bonjour le monde"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// `.` et `..` ouvrent la liste, les entrées suivent triées, un motif
    /// filtre, et la fin se dit `NO_MORE_FILES` ; un motif sans rien donne
    /// `NO_SUCH_FILE` dès la première requête.
    #[test]
    fn l_enumeration_suit_le_motif_et_dit_quand_c_est_fini() {
        let d = bac("enumeration");
        std::fs::write(d.join("b.log"), b"").unwrap();
        std::fs::write(d.join("A.txt"), b"xyz").unwrap();
        std::fs::create_dir(d.join("sous")).unwrap();
        let mut l = Lecteur::nouveau(&d).unwrap();
        let (s, id, _) = ouvrir(&mut l, "\\", CreateDisposition::FILE_OPEN, true, false);
        assert_eq!(s, NtStatus::SUCCESS);
        let mut noms = vec![enumerer(&mut l, id, "\\*", true).unwrap()];
        while let Ok(n) = enumerer(&mut l, id, "\\*", false) {
            noms.push(n);
        }
        assert_eq!(noms, [".", "..", "A.txt", "b.log", "sous"]);
        assert_eq!(
            enumerer(&mut l, id, "\\*", false),
            Err(NtStatus::NO_MORE_FILES)
        );
        assert_eq!(enumerer(&mut l, id, "\\*.TXT", true).unwrap(), "A.txt");
        assert_eq!(
            enumerer(&mut l, id, "\\*.TXT", false),
            Err(NtStatus::NO_MORE_FILES)
        );
        assert_eq!(
            enumerer(&mut l, id, "\\rien*", true),
            Err(NtStatus::NO_SUCH_FILE)
        );
        // Énumérer un fichier n'a pas de sens.
        let (_, f, _) = ouvrir(
            &mut l,
            "\\A.txt",
            CreateDisposition::FILE_OPEN,
            false,
            false,
        );
        assert_eq!(
            enumerer(&mut l, f, "\\*", true),
            Err(NtStatus::NOT_A_DIRECTORY)
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Supprimer passe par la disposition puis la fermeture ; renommer par
    /// `FileRenameInformation`, qui refuse d'écraser sans qu'on le lui dise.
    #[test]
    fn supprimer_et_renommer_passent_par_set_information() {
        let d = bac("modifier");
        std::fs::write(d.join("un.txt"), b"1").unwrap();
        std::fs::write(d.join("deux.txt"), b"2").unwrap();
        std::fs::create_dir(d.join("plein")).unwrap();
        std::fs::write(d.join("plein").join("x"), b"").unwrap();
        let mut l = Lecteur::nouveau(&d).unwrap();

        let (_, id, _) = ouvrir(
            &mut l,
            "\\un.txt",
            CreateDisposition::FILE_OPEN,
            false,
            true,
        );
        let collision = modifier(
            &mut l,
            id,
            FileInformationClass::Rename(FileRenameInformation {
                replace_if_exists: Boolean::False,
                file_name: "\\deux.txt".to_owned(),
            }),
        );
        assert_eq!(collision, NtStatus::SUCCESS, "la réponse existe");
        assert!(
            d.join("un.txt").exists(),
            "rien n'a bougé sans replace_if_exists"
        );
        modifier(
            &mut l,
            id,
            FileInformationClass::Rename(FileRenameInformation {
                replace_if_exists: Boolean::True,
                file_name: "\\trois.txt".to_owned(),
            }),
        );
        assert!(!d.join("un.txt").exists() && d.join("trois.txt").exists());
        modifier(
            &mut l,
            id,
            FileInformationClass::EndOfFile(FileEndOfFileInformation { end_of_file: 0 }),
        );
        assert_eq!(fermer(&mut l, id), NtStatus::SUCCESS);
        assert_eq!(std::fs::metadata(d.join("trois.txt")).unwrap().len(), 0);

        let (_, id, _) = ouvrir(
            &mut l,
            "\\deux.txt",
            CreateDisposition::FILE_OPEN,
            false,
            true,
        );
        modifier(
            &mut l,
            id,
            FileInformationClass::Disposition(FileDispositionInformation { delete_pending: 1 }),
        );
        assert!(
            d.join("deux.txt").exists(),
            "la suppression attend la fermeture"
        );
        assert_eq!(fermer(&mut l, id), NtStatus::SUCCESS);
        assert!(!d.join("deux.txt").exists());

        // Un dossier plein ne se supprime pas ; vidé, si.
        let (_, id, _) = ouvrir(&mut l, "\\plein", CreateDisposition::FILE_OPEN, true, false);
        let mut r = l.traiter(ServerDriveIoRequest::ServerDriveSetInformationRequest(
            ServerDriveSetInformationRequest {
                device_io_request: io(id, 8, MajorFunction::SetInformation),
                set_buffer: FileInformationClass::Disposition(FileDispositionInformation {
                    delete_pending: 1,
                }),
            },
        ));
        assert!(matches!(
            r.pop(),
            Some(RdpdrPdu::ClientDriveSetInformationResponse(_))
        ));
        assert_eq!(fermer(&mut l, id), NtStatus::SUCCESS);
        assert!(
            d.join("plein").exists(),
            "DIRECTORY_NOT_EMPTY ne pose pas la suppression"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Le volume se présente comme un disque distant NTFS avec de la place.
    #[test]
    fn le_volume_se_presente_en_ntfs_distant() {
        let d = bac("volume");
        let mut l = Lecteur::nouveau(&d).unwrap();
        let (_, id, _) = ouvrir(&mut l, "\\", CreateDisposition::FILE_OPEN, true, false);
        let demande = |niveau| {
            ServerDriveIoRequest::ServerDriveQueryVolumeInformationRequest(
                ServerDriveQueryVolumeInformationRequest {
                    device_io_request: io(id, 9, MajorFunction::QueryVolumeInformation),
                    fs_info_class_lvl: niveau,
                },
            )
        };
        assert!(matches!(
            l.traiter(demande(
                FileSystemInformationClassLevel::FILE_FS_VOLUME_INFORMATION
            ))
            .pop(),
            Some(RdpdrPdu::ClientDriveQueryVolumeInformationResponse(_))
        ));
        assert!(matches!(
            l.traiter(demande(
                FileSystemInformationClassLevel::FILE_FS_ATTRIBUTE_INFORMATION
            ))
            .pop(),
            Some(RdpdrPdu::ClientDriveQueryVolumeInformationResponse(_))
        ));
        let espace = espace_disque(&d);
        assert!(espace.total_unites > 0 && espace.octets_par_secteur == 512);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Le gestionnaire du canal ne répond jamais dans l'appel : la réponse
    /// vient du fil, avec le bon `completion_id`.
    #[test]
    fn le_gestionnaire_repond_par_le_fil_et_non_dans_l_appel() {
        let d = bac("fil");
        std::fs::write(d.join("f"), b"contenu").unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut backend = demarrer(&d, tx).unwrap();
        let immediat = backend
            .handle_drive_io_request(demande_creation(
                "\\f",
                CreateDisposition::FILE_OPEN,
                false,
                false,
            ))
            .unwrap();
        assert!(immediat.is_empty());
        let reponse = rx.blocking_recv().expect("une réponse par le fil");
        match reponse {
            RdpdrPdu::DeviceCreateResponse(c) => {
                assert_eq!(c.device_io_reply.completion_id, 1);
                assert_eq!(c.device_io_reply.io_status, NtStatus::SUCCESS);
                assert_ne!(c.file_id, 0);
            }
            _ => panic!("une réponse de création"),
        }
        assert!(
            Lecteur::nouveau(&d.join("absent")).is_err(),
            "une racine absente est refusée"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Motifs DOS : `*` et `?`, casse ignorée.
    #[test]
    fn le_motif_dos_ignore_la_casse() {
        assert!(correspond("*", "n'importe"));
        assert!(correspond("*.TXT", "notes.txt"));
        assert!(!correspond("*.txt", "notes.md"));
        assert!(correspond("n?tes.*", "Notes.md"));
        assert!(correspond("*", ".."));
        assert!(!correspond("a*", ".."));
        assert!(correspond("", ""));
    }

    /// Les dates passent en heure Windows ; la taille allouée s'arrondit au bloc.
    #[test]
    fn dates_et_tailles_sont_traduites() {
        let epoch = heure_windows(Ok(std::time::UNIX_EPOCH));
        assert_eq!(epoch, 116_444_736_000_000_000);
        assert_eq!(heure_windows(Err(std::io::Error::other("sans date"))), 0);
        let d = bac("dates");
        std::fs::write(d.join("f"), vec![0u8; 5000]).unwrap();
        let m = std::fs::metadata(d.join("f")).unwrap();
        assert_eq!(taille(&m), 5000);
        assert_eq!(allocation(&m), 8192);
        assert!(creation(&m) > epoch && modification(&m) > epoch);
        assert!(attributs(&m, "f").contains(FileAttributes::FILE_ATTRIBUTE_ARCHIVE));
        assert!(attributs(&m, ".cache").contains(FileAttributes::FILE_ATTRIBUTE_HIDDEN));
        let _ = std::fs::remove_dir_all(&d);
    }
}
