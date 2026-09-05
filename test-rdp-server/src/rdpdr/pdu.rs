//! Codage manuel des PDU RDPDR ([MS-RDPEFS]) que `ironrdp-rdpdr` 0.7 ne sait
//! faire que dans un sens. Le paquet est écrit pour un client : il encode ses
//! réponses et décode les requêtes du serveur. Ici on joue le serveur, il faut
//! donc décoder les réponses du client et encoder les requêtes.
//!
//! Ce qu'on réutilise du paquet : l'en-tête commun (`SharedHeader`), les PDU
//! serveur → client qui ont un `encode` (annonce, capacités, confirmation,
//! réponse d'annonce de périphérique, en-tête d'IRP, `UserLoggedOn`) et
//! `DeviceIoResponse::decode` pour l'en-tête des complétions. Les corps d'IRP
//! (`DR_CREATE_REQ` et les autres) n'ont qu'un `decode` dans le paquet, ce qui
//! sert aux tests : chaque encodeur d'ici est relu par le décodeur du paquet.
//!
//! [MS-RDPEFS]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpefs/34d9de58-b2b5-40b6-b970-f82d4603bdb5

use ironrdp::core::{
    cast_length, encode_vec, ensure_size, invalid_field_err, DecodeResult, Encode, EncodeResult,
    ReadCursor, WriteCursor,
};
use ironrdp::pdu::utils::{decode_string, from_utf16_bytes, CharacterSet};
use ironrdp::rdpdr::pdu::efs::{
    CapabilityMessage, CoreCapability, CoreCapabilityKind, DeviceIoRequest, MajorFunction,
    MinorFunction, NtStatus, ServerDeviceAnnounceResponse, VersionAndIdPdu, VersionAndIdPduKind,
    VERSION_MAJOR, VERSION_MINOR_12,
};
use ironrdp::rdpdr::pdu::RdpdrPdu;
use ironrdp::svc::SvcEncode;

/// Version mineure annoncée par le serveur : 1.12, comme un Windows récent.
///
/// Elle décide du moment où le client annonce ses lecteurs. Avec 0x0005
/// (RDP 5.1), `ironrdp-rdpdr` annonce tout dès `ServerClientIdConfirm` ; avec
/// 0x000C il n'annonce que les cartes à puce à ce moment-là et garde les
/// lecteurs pour `ServerUserLoggedOn`, exactement le chemin qu'il suit face
/// à un vrai Windows. C'est ce chemin-là que la suite bout en bout doit
/// éprouver, d'où 1.12 et l'envoi de `UserLoggedOn` juste après la
/// confirmation, dans l'ordre où `FreeRDP` serveur le fait.
pub const VERSION_MINEURE: u16 = VERSION_MINOR_12;

/// Identifiant que le serveur attribue au client (2.2.2.2, `ClientId`) ; le
/// client le renvoie tel quel, la valeur n'a pas d'autre sens.
pub const CLIENT_ID: u32 = 1;

/// `CAP_GENERAL_TYPE` (2.2.1.2.1).
pub const CAP_GENERAL_TYPE: u16 = 0x0001;
/// `CAP_DRIVE_TYPE`.
pub const CAP_DRIVE_TYPE: u16 = 0x0004;
/// `RDPDR_USER_LOGGEDON_PDU` dans `extendedPDU` (2.2.2.7.1) : le client accepte
/// un `ServerUserLoggedOn`.
pub const RDPDR_USER_LOGGEDON_PDU: u32 = 0x0000_0004;
/// `RDPDR_DTYP_FILESYSTEM` (2.2.1.3).
pub const RDPDR_DTYP_FILESYSTEM: u32 = 0x0000_0008;
/// `FILE_ATTRIBUTE_DIRECTORY` ([MS-FSCC] 2.6).
pub const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;

/// `FILE_OPEN` et `FILE_OVERWRITE_IF` ([MS-SMB2] 2.2.13, `CreateDisposition`).
const FILE_OPEN: u32 = 0x0000_0001;
const FILE_OVERWRITE_IF: u32 = 0x0000_0005;
/// `FILE_SHARE_READ` | `FILE_SHARE_WRITE` | `FILE_SHARE_DELETE`.
const FILE_SHARE_TOUT: u32 = 0x0000_0007;
/// `FileFsVolumeInformation` et `FileBothDirectoryInformation` ([MS-FSCC] 2.5 et 2.4).
pub const FILE_FS_VOLUME_INFORMATION: u32 = 1;
pub const FILE_BOTH_DIRECTORY_INFORMATION: u32 = 3;

// ---------------------------------------------------------------------------
// Client → serveur : décodeurs.
// ---------------------------------------------------------------------------

/// [2.2.2.3] Client Announce Reply (`DR_CORE_CLIENT_ANNOUNCE_RSP`).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct AnnonceClient {
    pub version_majeure: u16,
    pub version_mineure: u16,
    pub client_id: u32,
}

impl AnnonceClient {
    pub fn decode(src: &mut ReadCursor<'_>) -> DecodeResult<Self> {
        ensure_size!(ctx: "DR_CORE_CLIENT_ANNOUNCE_RSP", in: src, size: 8);
        let version_majeure = src.read_u16();
        let version_mineure = src.read_u16();
        let client_id = src.read_u32();
        Ok(Self {
            version_majeure,
            version_mineure,
            client_id,
        })
    }
}

/// [2.2.2.4] Client Name Request (`DR_CORE_CLIENT_NAME_REQ`) : le nom du poste.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct NomClient(pub String);

impl NomClient {
    pub fn decode(src: &mut ReadCursor<'_>) -> DecodeResult<Self> {
        ensure_size!(ctx: "DR_CORE_CLIENT_NAME_REQ", in: src, size: 12);
        let unicode = src.read_u32() & 1 == 1;
        let _code_page = src.read_u32();
        let longueur: usize =
            cast_length!("DR_CORE_CLIENT_NAME_REQ", "ComputerNameLen", src.read_u32())?;
        ensure_size!(ctx: "DR_CORE_CLIENT_NAME_REQ", in: src, size: longueur);
        let jeu = if unicode {
            CharacterSet::Unicode
        } else {
            CharacterSet::Ansi
        };
        Ok(Self(decode_string(src.read_slice(longueur), jeu, true)?))
    }
}

/// [2.2.2.8] Client Core Capability Response (`DR_CORE_CAPABILITY_RSP`), réduit
/// à ce qui décide de la suite : les types de capacités annoncés, la version
/// mineure et l'acceptation de `UserLoggedOn`.
#[derive(Debug, Default, PartialEq, Eq, Clone)]
pub struct CapacitesClient {
    /// `CapabilityType` de chaque `CAPABILITY_SET`, dans l'ordre reçu.
    pub types: Vec<u16>,
    /// `protocolMinorVersion` du `GENERAL_CAPS_SET`, s'il est là.
    pub version_mineure: Option<u16>,
    /// `RDPDR_USER_LOGGEDON_PDU` dans `extendedPDU`.
    pub user_logged_on: bool,
}

impl CapacitesClient {
    pub fn lecteur(&self) -> bool {
        self.types.contains(&CAP_DRIVE_TYPE)
    }

    pub fn decode(src: &mut ReadCursor<'_>) -> DecodeResult<Self> {
        ensure_size!(ctx: "DR_CORE_CAPABILITY_RSP", in: src, size: 4);
        let nombre = src.read_u16();
        let _padding = src.read_u16();
        let mut capacites = Self::default();
        for _ in 0..nombre {
            // CAPABILITY_HEADER (2.2.1.2) : la longueur comprend l'en-tête.
            ensure_size!(ctx: "CAPABILITY_HEADER", in: src, size: 8);
            let type_ = src.read_u16();
            let longueur = usize::from(src.read_u16());
            let _version = src.read_u32();
            let Some(corps) = longueur.checked_sub(8) else {
                return Err(invalid_field_err!(
                    "CAPABILITY_HEADER",
                    "CapabilityLength",
                    "plus courte que l'en-tête"
                ));
            };
            ensure_size!(ctx: "CAPABILITY_SET", in: src, size: corps);
            let donnees = src.read_slice(corps);
            if type_ == CAP_GENERAL_TYPE {
                // GENERAL_CAPS_SET (2.2.2.7.1) jusqu'à extendedPDU ; le
                // SpecialTypeDeviceCap de la version 2 ne nous sert pas.
                let mut general = ReadCursor::new(donnees);
                ensure_size!(ctx: "GENERAL_CAPS_SET", in: general, size: 28);
                let _os_type = general.read_u32();
                let _os_version = general.read_u32();
                let _version_majeure = general.read_u16();
                let version_mineure = general.read_u16();
                let _io_code_1 = general.read_u32();
                let _io_code_2 = general.read_u32();
                let etendus = general.read_u32();
                capacites.version_mineure = Some(version_mineure);
                capacites.user_logged_on = etendus & RDPDR_USER_LOGGEDON_PDU != 0;
            }
            capacites.types.push(type_);
        }
        Ok(capacites)
    }
}

/// Un `DEVICE_ANNOUNCE` (2.2.1.3) tel que le client l'annonce.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Peripherique {
    /// `DeviceType` brut : `RDPDR_DTYP_FILESYSTEM` pour un lecteur.
    pub type_: u32,
    pub id: u32,
    /// `PreferredDosName`, huit octets ASCII terminés par NUL.
    pub nom_dos: String,
    /// Le nom utile : `DeviceData` s'il est là (lecteur en `DRIVE_CAPABILITY_VERSION_02`),
    /// sinon le nom DOS.
    pub nom: String,
}

impl Peripherique {
    pub fn est_lecteur(&self) -> bool {
        self.type_ == RDPDR_DTYP_FILESYSTEM
    }
}

/// [2.2.2.9] Client Device List Announce Request (`DR_CORE_DEVICELIST_ANNOUNCE_REQ`).
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct AnnoncePeripheriques(pub Vec<Peripherique>);

impl AnnoncePeripheriques {
    pub fn decode(src: &mut ReadCursor<'_>) -> DecodeResult<Self> {
        ensure_size!(ctx: "DR_CORE_DEVICELIST_ANNOUNCE_REQ", in: src, size: 4);
        let nombre = src.read_u32();
        // Pas de réservation à la taille annoncée : un DeviceCount menteur
        // échoue sur le premier périphérique qui manque, sans rien allouer.
        let mut liste = Vec::new();
        for _ in 0..nombre {
            ensure_size!(ctx: "DEVICE_ANNOUNCE", in: src, size: 20);
            let type_ = src.read_u32();
            let id = src.read_u32();
            let dos = src.read_slice(8);
            let longueur: usize =
                cast_length!("DEVICE_ANNOUNCE", "DeviceDataLength", src.read_u32())?;
            ensure_size!(ctx: "DEVICE_ANNOUNCE", in: src, size: longueur);
            let donnees = src.read_slice(longueur);
            let nom_dos = jusqu_au_nul(dos);
            let nom = if donnees.is_empty() {
                nom_dos.clone()
            } else {
                nom_depuis_donnees(donnees)
            };
            liste.push(Peripherique {
                type_,
                id,
                nom_dos,
                nom,
            });
        }
        Ok(Self(liste))
    }
}

/// Le nom complet d'un lecteur, dans `DeviceData`.
///
/// MS-RDPEFS 2.2.1.3 dit « chaîne Unicode terminée par NUL ». `ironrdp-rdpdr`
/// envoie de l'UTF-8 terminé par NUL (« empirically this wants null terminated
/// UTF-8 », dit son code), `FreeRDP` aussi, et Windows accepte les deux. On
/// reconnaît l'UTF-16LE à l'octet haut nul de son premier caractère, un nom
/// de lecteur commençant par une lettre.
fn nom_depuis_donnees(donnees: &[u8]) -> String {
    if donnees.len() >= 2 && donnees[1] == 0 {
        from_utf16_bytes(donnees).trim_end_matches('\0').to_owned()
    } else {
        String::from_utf8_lossy(donnees)
            .trim_end_matches('\0')
            .to_owned()
    }
}

fn jusqu_au_nul(octets: &[u8]) -> String {
    let fin = octets.iter().position(|&o| o == 0).unwrap_or(octets.len());
    String::from_utf8_lossy(&octets[..fin]).into_owned()
}

/// [2.2.1.5.1] Device Create Response (`DR_CREATE_RSP`), après l'en-tête de
/// complétion.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct ReponseCreation {
    pub file_id: u32,
    /// `FILE_SUPERSEDED`, `FILE_OPENED` ou `FILE_OVERWRITTEN` ; 0 s'il manque.
    pub information: u8,
}

impl ReponseCreation {
    pub fn decode(src: &mut ReadCursor<'_>) -> DecodeResult<Self> {
        ensure_size!(ctx: "DR_CREATE_RSP", in: src, size: 4);
        let file_id = src.read_u32();
        // FreeRDP et ironrdp l'écrivent toujours ; on ne s'arrête pas sur un
        // client qui l'omettrait, le FileId suffit à la suite.
        let information = if src.is_empty() { 0 } else { src.read_u8() };
        Ok(Self {
            file_id,
            information,
        })
    }
}

/// [2.2.1.5.3] Device Read Response (`DR_READ_RSP`) : les octets lus.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ReponseLecture(pub Vec<u8>);

impl ReponseLecture {
    pub fn decode(src: &mut ReadCursor<'_>) -> DecodeResult<Self> {
        ensure_size!(ctx: "DR_READ_RSP", in: src, size: 4);
        let longueur: usize = cast_length!("DR_READ_RSP", "Length", src.read_u32())?;
        ensure_size!(ctx: "DR_READ_RSP", in: src, size: longueur);
        Ok(Self(src.read_slice(longueur).to_vec()))
    }
}

/// [2.2.1.5.4] Device Write Response (`DR_WRITE_RSP`) : le nombre d'octets
/// écrits. L'octet de remplissage qui suit est facultatif, on ne le lit pas.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct ReponseEcriture {
    pub longueur: u32,
}

impl ReponseEcriture {
    pub fn decode(src: &mut ReadCursor<'_>) -> DecodeResult<Self> {
        ensure_size!(ctx: "DR_WRITE_RSP", in: src, size: 4);
        Ok(Self {
            longueur: src.read_u32(),
        })
    }
}

/// Une entrée de [2.2.3.4.10] Client Drive Query Directory Response, en
/// `FileBothDirectoryInformation` ([MS-FSCC] 2.4.8).
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct EntreeRepertoire {
    pub nom: String,
    pub taille: i64,
    pub attributs: u32,
}

impl EntreeRepertoire {
    /// Taille fixe de `FileBothDirectoryInformation` sur le fil, sans l'octet
    /// Reserved de MS-FSCC : `FreeRDP` et ironrdp ne l'écrivent pas (« MUST NOT
    /// be added », `drive_file.c`), et Windows le lit ainsi.
    const PARTIE_FIXE: usize = 4 + 4 + 8 * 6 + 4 + 4 + 4 + 1 + 24;

    pub fn est_dossier(&self) -> bool {
        self.attributs & FILE_ATTRIBUTE_DIRECTORY != 0
    }

    pub fn decode(src: &mut ReadCursor<'_>) -> DecodeResult<Self> {
        ensure_size!(ctx: "DR_DRIVE_QUERY_DIRECTORY_RSP", in: src, size: 4);
        let longueur: usize =
            cast_length!("DR_DRIVE_QUERY_DIRECTORY_RSP", "Length", src.read_u32())?;
        ensure_size!(ctx: "DR_DRIVE_QUERY_DIRECTORY_RSP", in: src, size: longueur);
        let mut corps = ReadCursor::new(src.read_slice(longueur));
        ensure_size!(ctx: "FileBothDirectoryInformation", in: corps, size: Self::PARTIE_FIXE);
        let _next_entry_offset = corps.read_u32();
        let _file_index = corps.read_u32();
        let _creation = corps.read_i64();
        let _dernier_acces = corps.read_i64();
        let _derniere_ecriture = corps.read_i64();
        let _changement = corps.read_i64();
        let taille = corps.read_i64();
        let _allocation = corps.read_i64();
        let attributs = corps.read_u32();
        let longueur_nom: usize = cast_length!(
            "FileBothDirectoryInformation",
            "FileNameLength",
            corps.read_u32()
        )?;
        let _ea_size = corps.read_u32();
        let _longueur_nom_court = corps.read_u8();
        let _nom_court = corps.read_slice(24);
        ensure_size!(ctx: "FileBothDirectoryInformation", in: corps, size: longueur_nom);
        let nom = from_utf16_bytes(corps.read_slice(longueur_nom))
            .trim_end_matches('\0')
            .to_owned();
        Ok(Self {
            nom,
            taille,
            attributs,
        })
    }
}

/// [2.2.3.4.6] Client Drive Query Volume Information Response en
/// `FileFsVolumeInformation` ([MS-FSCC] 2.5.9), sans l'octet Reserved, comme
/// pour les entrées de répertoire.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct InfosVolume {
    pub etiquette: String,
    pub numero_serie: u32,
}

impl InfosVolume {
    pub fn decode(src: &mut ReadCursor<'_>) -> DecodeResult<Self> {
        ensure_size!(ctx: "DR_DRIVE_QUERY_VOLUME_INFORMATION_RSP", in: src, size: 4);
        let longueur: usize = cast_length!(
            "DR_DRIVE_QUERY_VOLUME_INFORMATION_RSP",
            "Length",
            src.read_u32()
        )?;
        ensure_size!(ctx: "DR_DRIVE_QUERY_VOLUME_INFORMATION_RSP", in: src, size: longueur);
        let mut corps = ReadCursor::new(src.read_slice(longueur));
        ensure_size!(ctx: "FileFsVolumeInformation", in: corps, size: 8 + 4 + 4 + 1);
        let _creation = corps.read_i64();
        let numero_serie = corps.read_u32();
        let longueur_etiquette: usize = cast_length!(
            "FileFsVolumeInformation",
            "VolumeLabelLength",
            corps.read_u32()
        )?;
        let _supports_objects = corps.read_u8();
        ensure_size!(ctx: "FileFsVolumeInformation", in: corps, size: longueur_etiquette);
        // ironrdp compte le NUL final dans la longueur, FreeRDP non : on lit
        // jusqu'au NUL s'il y en a un, tout sinon.
        let etiquette = decode_string(
            corps.read_slice(longueur_etiquette),
            CharacterSet::Unicode,
            true,
        )?;
        Ok(Self {
            etiquette,
            numero_serie,
        })
    }
}

// ---------------------------------------------------------------------------
// Serveur → client : encodeurs.
// ---------------------------------------------------------------------------

/// [2.2.2.2] Server Announce Request.
pub fn annonce_serveur() -> RdpdrPdu {
    RdpdrPdu::VersionAndIdPdu(VersionAndIdPdu {
        version_major: VERSION_MAJOR,
        version_minor: VERSION_MINEURE,
        client_id: CLIENT_ID,
        kind: VersionAndIdPduKind::ServerAnnounceRequest,
    })
}

/// [2.2.2.7] Server Core Capability Request : général, imprimante, lecteur,
/// carte à puce, ce qu'un Windows annonce (moins le port série, que le paquet
/// ne sait pas construire et dont personne ici n'a besoin).
pub fn demande_capacites() -> RdpdrPdu {
    RdpdrPdu::CoreCapability(CoreCapability {
        capabilities: vec![
            CapabilityMessage::new_general(0),
            CapabilityMessage::new_printer(),
            CapabilityMessage::new_drive(),
            CapabilityMessage::new_smartcard(),
        ],
        kind: CoreCapabilityKind::ServerCoreCapabilityRequest,
    })
}

/// [2.2.2.6] Server Client ID Confirm.
pub fn confirmation_client_id() -> RdpdrPdu {
    RdpdrPdu::VersionAndIdPdu(VersionAndIdPdu {
        version_major: VERSION_MAJOR,
        version_minor: VERSION_MINEURE,
        client_id: CLIENT_ID,
        kind: VersionAndIdPduKind::ServerClientIdConfirm,
    })
}

/// [2.2.2.5] Server User Logged On.
pub fn user_logged_on() -> RdpdrPdu {
    RdpdrPdu::UserLoggedon
}

/// [2.2.2.1] Server Device Announce Response.
pub fn reponse_annonce(device_id: u32, statut: NtStatus) -> RdpdrPdu {
    RdpdrPdu::ServerDeviceAnnounceResponse(ServerDeviceAnnounceResponse {
        device_id,
        result_code: statut,
    })
}

/// Un IRP complet (en-tête RDPDR, en-tête `DR_DEVICE_IOREQUEST`, corps), prêt à
/// partir sur le canal. Le paquet n'encode que l'en-tête ; le corps est à nous.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Irp {
    nom: &'static str,
    octets: Vec<u8>,
}

impl Encode for Irp {
    fn encode(&self, dst: &mut WriteCursor<'_>) -> EncodeResult<()> {
        ensure_size!(in: dst, size: self.octets.len());
        dst.write_slice(&self.octets);
        Ok(())
    }

    fn name(&self) -> &'static str {
        self.nom
    }

    fn size(&self) -> usize {
        self.octets.len()
    }
}

impl SvcEncode for Irp {}

/// Les quatre champs d'un `DR_CREATE_REQ` qui disent ce qu'on ouvre et comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ouverture {
    pub acces: u32,
    pub disposition: u32,
    pub options: u32,
    pub attributs: u32,
}

impl Ouverture {
    /// Un dossier existant, pour l'énumérer : `FILE_LIST_DIRECTORY` |
    /// `FILE_READ_ATTRIBUTES` | SYNCHRONIZE, `FILE_DIRECTORY_FILE` |
    /// `FILE_SYNCHRONOUS_IO_NONALERT`. Ce que Windows envoie pour un `dir`.
    pub const DOSSIER: Self = Self {
        acces: 0x0010_0081,
        disposition: FILE_OPEN,
        options: 0x0000_0021,
        attributs: 0,
    };

    /// Un fichier existant, en lecture : `FILE_READ_DATA` | `FILE_READ_EA` |
    /// `FILE_READ_ATTRIBUTES` | SYNCHRONIZE, `FILE_NON_DIRECTORY_FILE` |
    /// `FILE_SYNCHRONOUS_IO_NONALERT`.
    pub const LECTURE: Self = Self {
        acces: 0x0010_0089,
        disposition: FILE_OPEN,
        options: 0x0000_0060,
        attributs: 0,
    };

    /// Un fichier créé ou écrasé, en écriture : `FILE_WRITE_DATA` |
    /// `FILE_APPEND_DATA` | `FILE_WRITE_EA` | `FILE_WRITE_ATTRIBUTES` | SYNCHRONIZE,
    /// `FILE_OVERWRITE_IF` pour que le scénario se rejoue, `FILE_ATTRIBUTE_NORMAL`.
    pub const ECRITURE: Self = Self {
        acces: 0x0010_0116,
        disposition: FILE_OVERWRITE_IF,
        options: 0x0000_0060,
        attributs: 0x0000_0080,
    };
}

fn en_tete_irp(
    device_id: u32,
    file_id: u32,
    completion_id: u32,
    major: MajorFunction,
    minor: MinorFunction,
) -> EncodeResult<Vec<u8>> {
    encode_vec(&RdpdrPdu::DeviceIoRequest(DeviceIoRequest {
        device_id,
        file_id,
        completion_id,
        major_function: major,
        minor_function: minor,
    }))
}

fn utf16_nul(texte: &str) -> Vec<u8> {
    texte
        .encode_utf16()
        .chain(core::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

fn u32_le(octets: &mut Vec<u8>, valeur: u32) {
    octets.extend_from_slice(&valeur.to_le_bytes());
}

fn u64_le(octets: &mut Vec<u8>, valeur: u64) {
    octets.extend_from_slice(&valeur.to_le_bytes());
}

fn remplissage(octets: &mut Vec<u8>, n: usize) {
    octets.resize(octets.len() + n, 0);
}

/// [2.2.3.3.1] Server Create Drive Request (`DR_DRIVE_CREATE_REQ`). Le chemin
/// est en UTF-16 terminé par NUL, longueur NUL compris.
pub fn irp_creation(
    device_id: u32,
    completion_id: u32,
    chemin: &str,
    ouverture: Ouverture,
) -> EncodeResult<Irp> {
    let mut octets = en_tete_irp(
        device_id,
        0,
        completion_id,
        MajorFunction::Create,
        MinorFunction::from(0),
    )?;
    u32_le(&mut octets, ouverture.acces);
    u64_le(&mut octets, 0); // AllocationSize
    u32_le(&mut octets, ouverture.attributs);
    u32_le(&mut octets, FILE_SHARE_TOUT);
    u32_le(&mut octets, ouverture.disposition);
    u32_le(&mut octets, ouverture.options);
    let chemin = utf16_nul(chemin);
    u32_le(
        &mut octets,
        cast_length!("DR_CREATE_REQ", "PathLength", chemin.len())?,
    );
    octets.extend_from_slice(&chemin);
    Ok(Irp {
        nom: "DR_CREATE_REQ",
        octets,
    })
}

/// [2.2.1.4.2] Device Close Request (`DR_CLOSE_REQ`) : 32 octets de remplissage.
pub fn irp_fermeture(device_id: u32, file_id: u32, completion_id: u32) -> EncodeResult<Irp> {
    let mut octets = en_tete_irp(
        device_id,
        file_id,
        completion_id,
        MajorFunction::Close,
        MinorFunction::from(0),
    )?;
    remplissage(&mut octets, 32);
    Ok(Irp {
        nom: "DR_CLOSE_REQ",
        octets,
    })
}

/// [2.2.1.4.3] Device Read Request (`DR_READ_REQ`).
pub fn irp_lecture(
    device_id: u32,
    file_id: u32,
    completion_id: u32,
    longueur: u32,
    position: u64,
) -> EncodeResult<Irp> {
    let mut octets = en_tete_irp(
        device_id,
        file_id,
        completion_id,
        MajorFunction::Read,
        MinorFunction::from(0),
    )?;
    u32_le(&mut octets, longueur);
    u64_le(&mut octets, position);
    remplissage(&mut octets, 20);
    Ok(Irp {
        nom: "DR_READ_REQ",
        octets,
    })
}

/// [2.2.1.4.4] Device Write Request (`DR_WRITE_REQ`).
pub fn irp_ecriture(
    device_id: u32,
    file_id: u32,
    completion_id: u32,
    position: u64,
    donnees: &[u8],
) -> EncodeResult<Irp> {
    let mut octets = en_tete_irp(
        device_id,
        file_id,
        completion_id,
        MajorFunction::Write,
        MinorFunction::from(0),
    )?;
    u32_le(
        &mut octets,
        cast_length!("DR_WRITE_REQ", "Length", donnees.len())?,
    );
    u64_le(&mut octets, position);
    remplissage(&mut octets, 20);
    octets.extend_from_slice(donnees);
    Ok(Irp {
        nom: "DR_WRITE_REQ",
        octets,
    })
}

/// [2.2.3.3.6] Server Drive Query Volume Information Request, tampon vide.
pub fn irp_volume(device_id: u32, file_id: u32, completion_id: u32) -> EncodeResult<Irp> {
    let mut octets = en_tete_irp(
        device_id,
        file_id,
        completion_id,
        MajorFunction::QueryVolumeInformation,
        MinorFunction::from(0),
    )?;
    u32_le(&mut octets, FILE_FS_VOLUME_INFORMATION);
    u32_le(&mut octets, 0); // Length
    remplissage(&mut octets, 24);
    Ok(Irp {
        nom: "DR_DRIVE_QUERY_VOLUME_INFORMATION_REQ",
        octets,
    })
}

/// [2.2.3.3.10] Server Drive Query Directory Request en
/// `FileBothDirectoryInformation`. Le chemin (`\*`) n'accompagne que la requête
/// initiale ; les suivantes ont un `PathLength` nul, comme Windows les envoie.
pub fn irp_repertoire(
    device_id: u32,
    file_id: u32,
    completion_id: u32,
    chemin_initial: Option<&str>,
) -> EncodeResult<Irp> {
    let mut octets = en_tete_irp(
        device_id,
        file_id,
        completion_id,
        MajorFunction::DirectoryControl,
        MinorFunction::IRP_MN_QUERY_DIRECTORY,
    )?;
    u32_le(&mut octets, FILE_BOTH_DIRECTORY_INFORMATION);
    octets.push(u8::from(chemin_initial.is_some())); // InitialQuery
    let chemin = chemin_initial.map(utf16_nul).unwrap_or_default();
    u32_le(
        &mut octets,
        cast_length!("DR_DRIVE_QUERY_DIRECTORY_REQ", "PathLength", chemin.len())?,
    );
    remplissage(&mut octets, 23);
    octets.extend_from_slice(&chemin);
    Ok(Irp {
        nom: "DR_DRIVE_QUERY_DIRECTORY_REQ",
        octets,
    })
}
