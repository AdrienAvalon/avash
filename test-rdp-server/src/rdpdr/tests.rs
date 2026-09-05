//! Le côté serveur de rdpdr, éprouvé sans réseau.
//!
//! Trois familles : les décodeurs écrits à la main relus contre les encodeurs
//! du paquet `ironrdp-rdpdr` (et des octets composés d'après MS-RDPEFS quand
//! le paquet n'encode pas) ; les encodeurs d'IRP relus par les décodeurs du
//! paquet ; et l'automate, d'abord contre des complétions simulées, puis dans
//! un dialogue complet avec le client `Rdpdr` du paquet servant un dossier
//! temporaire, les deux `process()` reliés directement.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};

use ironrdp::core::{decode, encode_vec, impl_as_any, ReadCursor};
use ironrdp::pdu::PduResult;
use ironrdp::rdpdr::backend::RdpdrBackend;
use ironrdp::rdpdr::pdu::efs::{
    Boolean, Capabilities, ClientDriveQueryDirectoryResponse,
    ClientDriveQueryVolumeInformationResponse, ClientNameRequest, ClientNameRequestUnicodeFlag,
    CoreCapability, CreateDisposition, CreateOptions, DesiredAccess, DeviceCloseResponse,
    DeviceControlRequest, DeviceCreateResponse, DeviceIoRequest, DeviceIoResponse,
    DeviceReadResponse, DeviceWriteResponse, FileAttributes, FileBothDirectoryInformation,
    FileFsVolumeInformation, FileInformationClassLevel, FileSystemInformationClassLevel,
    Information, MajorFunction, MinorFunction, NtStatus, ServerDeviceAnnounceResponse,
    ServerDriveIoRequest, VersionAndIdPdu, VersionAndIdPduKind, VERSION_MAJOR,
};
use ironrdp::rdpdr::pdu::esc::{ScardCall, ScardIoCtlCode};
use ironrdp::rdpdr::pdu::RdpdrPdu;
use ironrdp::rdpdr::{NoopRdpdrBackend, Rdpdr};
use ironrdp::svc::{SvcMessage, SvcProcessor as _};
use sha2::{Digest as _, Sha256};

use super::pdu::{
    irp_creation, irp_ecriture, irp_fermeture, irp_lecture, irp_repertoire, irp_volume,
    AnnonceClient, AnnoncePeripheriques, CapacitesClient, EntreeRepertoire, InfosVolume, NomClient,
    Ouverture, ReponseCreation, ReponseEcriture, ReponseLecture, CAP_DRIVE_TYPE, CAP_GENERAL_TYPE,
    CLIENT_ID, FILE_ATTRIBUTE_DIRECTORY, FILE_BOTH_DIRECTORY_INFORMATION,
    FILE_FS_VOLUME_INFORMATION, RDPDR_DTYP_FILESYSTEM, VERSION_MINEURE,
};
use super::{hex, Scenario, CONTENU_ECRIT, MORCEAU};

// ---------------------------------------------------------------------------
// Outils.
// ---------------------------------------------------------------------------

/// Les octets d'un PDU du paquet, en-tête compris.
fn octets(pdu: &RdpdrPdu) -> Vec<u8> {
    encode_vec(pdu).expect("encodage du paquet")
}

/// Saute l'en-tête commun et rend le curseur sur le corps.
fn corps(octets: &[u8]) -> ReadCursor<'_> {
    let mut src = ReadCursor::new(octets);
    let _ = src.read_u32();
    src
}

/// Les octets d'un message émis par l'automate.
fn brut(message: &SvcMessage) -> Vec<u8> {
    message.encode_unframed_pdu().expect("encodage du message")
}

/// Décode un IRP émis par l'automate avec les décodeurs du paquet.
fn irp(message: &SvcMessage) -> ServerDriveIoRequest {
    let octets = brut(message);
    let mut src = ReadCursor::new(&octets);
    let RdpdrPdu::DeviceIoRequest(en_tete) = decode::<RdpdrPdu>(&octets).expect("en-tête d'IRP")
    else {
        panic!("pas un IRP");
    };
    let _ = src.read_slice(4 + 20);
    ServerDriveIoRequest::decode(en_tete, &mut src).expect("corps d'IRP")
}

fn completion(device_id: u32, completion_id: u32, statut: NtStatus) -> DeviceIoResponse {
    DeviceIoResponse {
        device_id,
        completion_id,
        io_status: statut,
    }
}

/// Un dossier temporaire vide, propre à ce test.
fn dossier_temporaire(nom: &str) -> PathBuf {
    let chemin = std::env::temp_dir().join(format!(
        "test-rdp-server-rdpdr-{}-{nom}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&chemin);
    fs::create_dir_all(&chemin).expect("dossier temporaire");
    chemin
}

// ---------------------------------------------------------------------------
// Décodeurs client → serveur.
// ---------------------------------------------------------------------------

#[test]
fn l_annonce_du_client_se_relit_depuis_l_encodeur_du_paquet() {
    let pdu = RdpdrPdu::VersionAndIdPdu(VersionAndIdPdu {
        version_major: VERSION_MAJOR,
        version_minor: 0x000C,
        client_id: 42,
        kind: VersionAndIdPduKind::ClientAnnounceReply,
    });
    let o = octets(&pdu);
    let annonce = AnnonceClient::decode(&mut corps(&o)).expect("décodage");
    assert_eq!(
        annonce,
        AnnonceClient {
            version_majeure: 1,
            version_mineure: 0x000C,
            client_id: 42
        }
    );
}

#[test]
fn le_nom_du_client_se_relit_en_unicode_et_en_ascii() {
    for (drapeau, nom) in [
        (ClientNameRequestUnicodeFlag::Unicode, "poste-été"),
        (ClientNameRequestUnicodeFlag::Ascii, "poste"),
    ] {
        let pdu = RdpdrPdu::ClientNameRequest(ClientNameRequest::new(nom.to_owned(), drapeau));
        let o = octets(&pdu);
        let lu = NomClient::decode(&mut corps(&o)).expect("décodage");
        assert_eq!(lu.0, nom);
    }
}

#[test]
fn les_capacites_du_client_disent_le_lecteur_et_user_logged_on() {
    let mut capacites = Capabilities::new();
    capacites.add_drive();
    let pdu = RdpdrPdu::CoreCapability(CoreCapability::new_response(capacites.clone_inner()));
    let o = octets(&pdu);
    let lu = CapacitesClient::decode(&mut corps(&o)).expect("décodage");
    assert_eq!(lu.types, vec![CAP_GENERAL_TYPE, CAP_DRIVE_TYPE]);
    assert!(lu.lecteur());
    assert!(lu.user_logged_on);
    assert_eq!(lu.version_mineure, Some(0x000C));

    // Sans lecteur.
    let pdu = RdpdrPdu::CoreCapability(CoreCapability::new_response(
        Capabilities::new().clone_inner(),
    ));
    let o = octets(&pdu);
    let lu = CapacitesClient::decode(&mut corps(&o)).expect("décodage");
    assert!(!lu.lecteur());
}

#[test]
fn une_capacite_plus_courte_que_son_en_tete_est_refusee() {
    // numCapabilities 1, padding, puis CAPABILITY_HEADER type 4 longueur 4 :
    // moins que les huit octets de l'en-tête lui-même.
    let o = [1, 0, 0, 0, 4, 0, 4, 0, 2, 0, 0, 0];
    assert!(CapacitesClient::decode(&mut ReadCursor::new(&o)).is_err());
}

#[test]
fn l_annonce_d_un_lecteur_par_le_paquet_donne_son_nom_utf8() {
    let mut client = Rdpdr::new(Box::new(NoopRdpdrBackend), "poste".to_owned());
    let pdu = RdpdrPdu::ClientDeviceListAnnounce(client.add_drive(1, "Avash".to_owned()));
    let o = octets(&pdu);
    let lu = AnnoncePeripheriques::decode(&mut corps(&o)).expect("décodage");
    assert_eq!(lu.0.len(), 1);
    let p = &lu.0[0];
    assert!(p.est_lecteur());
    assert_eq!(p.type_, RDPDR_DTYP_FILESYSTEM);
    assert_eq!(p.id, 1);
    assert_eq!(p.nom, "Avash");
    assert_eq!(p.nom_dos, "ignored");
}

#[test]
fn l_annonce_d_un_lecteur_a_la_facon_de_mstsc_est_lue_en_utf16() {
    // DEVICE_ANNOUNCE composé d'après 2.2.1.3 : DeviceData en UTF-16LE terminé
    // par NUL, comme mstsc le fait, et un second périphérique sans DeviceData,
    // nommé par son seul PreferredDosName.
    let mut o = Vec::new();
    o.extend_from_slice(&2u32.to_le_bytes());
    o.extend_from_slice(&RDPDR_DTYP_FILESYSTEM.to_le_bytes());
    o.extend_from_slice(&7u32.to_le_bytes());
    o.extend_from_slice(b"C:\0\0\0\0\0\0");
    let nom: Vec<u8> = "Docs"
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect();
    o.extend_from_slice(&u32::try_from(nom.len()).unwrap().to_le_bytes());
    o.extend_from_slice(&nom);
    o.extend_from_slice(&0x20u32.to_le_bytes()); // carte à puce
    o.extend_from_slice(&9u32.to_le_bytes());
    o.extend_from_slice(b"SCARD\0\0\0");
    o.extend_from_slice(&0u32.to_le_bytes());
    let lu = AnnoncePeripheriques::decode(&mut ReadCursor::new(&o)).expect("décodage");
    assert_eq!(lu.0.len(), 2);
    assert_eq!(lu.0[0].nom, "Docs");
    assert_eq!(lu.0[0].nom_dos, "C:");
    assert!(!lu.0[1].est_lecteur());
    assert_eq!(lu.0[1].nom, "SCARD");
}

#[test]
fn un_devicecount_menteur_echoue_sans_allouer() {
    let o = u32::MAX.to_le_bytes();
    assert!(AnnoncePeripheriques::decode(&mut ReadCursor::new(&o)).is_err());
}

#[test]
fn les_completions_du_paquet_se_relisent() {
    let en_tete = completion(1, 7, NtStatus::SUCCESS);

    let o = octets(&RdpdrPdu::DeviceCreateResponse(DeviceCreateResponse {
        device_io_reply: en_tete.clone(),
        file_id: 0x1234,
        information: Information::FILE_OPENED,
    }));
    let mut src = corps(&o);
    let lu = DeviceIoResponse::decode(&mut src).expect("en-tête");
    assert_eq!(lu, en_tete);
    assert_eq!(
        ReponseCreation::decode(&mut src).expect("création"),
        ReponseCreation {
            file_id: 0x1234,
            information: 1
        }
    );

    let o = octets(&RdpdrPdu::DeviceReadResponse(DeviceReadResponse {
        device_io_reply: en_tete.clone(),
        read_data: b"bonjour".to_vec(),
    }));
    let mut src = corps(&o);
    DeviceIoResponse::decode(&mut src).expect("en-tête");
    assert_eq!(
        ReponseLecture::decode(&mut src).expect("lecture").0,
        b"bonjour"
    );

    let o = octets(&RdpdrPdu::DeviceWriteResponse(DeviceWriteResponse {
        device_io_reply: en_tete.clone(),
        length: 18,
    }));
    let mut src = corps(&o);
    DeviceIoResponse::decode(&mut src).expect("en-tête");
    assert_eq!(
        ReponseEcriture::decode(&mut src)
            .expect("écriture")
            .longueur,
        18
    );

    let o = octets(&RdpdrPdu::DeviceCloseResponse(DeviceCloseResponse {
        device_io_response: en_tete.clone(),
    }));
    let mut src = corps(&o);
    DeviceIoResponse::decode(&mut src).expect("en-tête");
    assert_eq!(src.len(), 4, "les quatre octets de remplissage restent");
}

#[test]
fn une_creation_sans_octet_information_passe_quand_meme() {
    // En-tête de complétion puis FileId seul : FreeRDP l'écrit toujours mais
    // rien n'oblige un autre client.
    let mut o = Vec::new();
    o.extend_from_slice(&1u32.to_le_bytes());
    o.extend_from_slice(&7u32.to_le_bytes());
    o.extend_from_slice(&0u32.to_le_bytes());
    o.extend_from_slice(&5u32.to_le_bytes());
    let mut src = ReadCursor::new(&o);
    DeviceIoResponse::decode(&mut src).expect("en-tête");
    let lu = ReponseCreation::decode(&mut src).expect("création");
    assert_eq!(lu.file_id, 5);
    assert_eq!(lu.information, 0);
}

#[test]
fn une_entree_de_repertoire_du_paquet_se_relit() {
    let entree = FileBothDirectoryInformation::new(
        1,
        2,
        3,
        4,
        1234,
        FileAttributes::FILE_ATTRIBUTE_DIRECTORY,
        "Été".to_owned(),
    );
    let o = octets(&RdpdrPdu::ClientDriveQueryDirectoryResponse(
        ClientDriveQueryDirectoryResponse {
            device_io_reply: completion(1, 3, NtStatus::SUCCESS),
            buffer: Some(entree.into()),
        },
    ));
    let mut src = corps(&o);
    DeviceIoResponse::decode(&mut src).expect("en-tête");
    let lu = EntreeRepertoire::decode(&mut src).expect("entrée");
    assert_eq!(lu.nom, "Été");
    assert_eq!(lu.taille, 1234);
    assert!(lu.est_dossier());
    assert_eq!(lu.attributs, FILE_ATTRIBUTE_DIRECTORY);
    assert_eq!(src.len(), 0, "tout est consommé");
}

#[test]
fn les_infos_de_volume_du_paquet_se_relisent() {
    let volume = FileFsVolumeInformation {
        volume_creation_time: 0,
        volume_serial_number: 0xCAFE_1234,
        supports_objects: Boolean::False,
        volume_label: "AVASH".to_owned(),
    };
    let o = octets(&RdpdrPdu::ClientDriveQueryVolumeInformationResponse(
        ClientDriveQueryVolumeInformationResponse::new(
            DeviceIoRequest {
                device_id: 1,
                file_id: 9,
                completion_id: 2,
                major_function: MajorFunction::QueryVolumeInformation,
                minor_function: MinorFunction::from(0),
            },
            NtStatus::SUCCESS,
            Some(volume.into()),
        ),
    ));
    let mut src = corps(&o);
    DeviceIoResponse::decode(&mut src).expect("en-tête");
    let lu = InfosVolume::decode(&mut src).expect("volume");
    assert_eq!(lu.etiquette, "AVASH");
    assert_eq!(lu.numero_serie, 0xCAFE_1234);
}

#[test]
fn un_pdu_tronque_ne_fait_pas_paniquer_les_decodeurs() {
    // Chaque préfixe strict d'un PDU valide doit être refusé proprement,
    // jamais par une panique : c'est ce qu'un client hostile enverrait.
    let en_tete = completion(1, 7, NtStatus::SUCCESS);
    let entree = FileBothDirectoryInformation::new(
        0,
        0,
        0,
        0,
        8,
        FileAttributes::FILE_ATTRIBUTE_NORMAL,
        "bonjour.txt".to_owned(),
    );
    let mut client = Rdpdr::new(Box::new(NoopRdpdrBackend), "poste".to_owned());
    let complets = [
        octets(&RdpdrPdu::ClientDriveQueryDirectoryResponse(
            ClientDriveQueryDirectoryResponse {
                device_io_reply: en_tete.clone(),
                buffer: Some(entree.into()),
            },
        )),
        octets(&RdpdrPdu::DeviceReadResponse(DeviceReadResponse {
            device_io_reply: en_tete.clone(),
            read_data: vec![7; 40],
        })),
        octets(&RdpdrPdu::ClientNameRequest(ClientNameRequest::new(
            "poste".to_owned(),
            ClientNameRequestUnicodeFlag::Unicode,
        ))),
        octets(&RdpdrPdu::ClientDeviceListAnnounce(
            client.add_drive(1, "Avash".to_owned()),
        )),
        octets(&RdpdrPdu::CoreCapability(CoreCapability::new_response(
            Capabilities::new().clone_inner(),
        ))),
    ];
    for complet in &complets {
        for fin in 0..complet.len() {
            let tronque = &complet[..fin];
            let mut src = ReadCursor::new(tronque);
            // Peu importe le décodeur : aucun ne doit paniquer.
            let _ = AnnonceClient::decode(&mut src);
            let mut src = ReadCursor::new(tronque);
            let _ = NomClient::decode(&mut src);
            let mut src = ReadCursor::new(tronque);
            let _ = CapacitesClient::decode(&mut src);
            let mut src = ReadCursor::new(tronque);
            let _ = AnnoncePeripheriques::decode(&mut src);
            let mut src = ReadCursor::new(tronque);
            let _ = ReponseCreation::decode(&mut src);
            let mut src = ReadCursor::new(tronque);
            let _ = ReponseLecture::decode(&mut src);
            let mut src = ReadCursor::new(tronque);
            let _ = EntreeRepertoire::decode(&mut src);
            let mut src = ReadCursor::new(tronque);
            let _ = InfosVolume::decode(&mut src);
            // Et l'automate, à toute étape, l'avale sans tomber.
            let mut scenario = Scenario::new();
            let _ = scenario.recevoir(tronque);
        }
    }
}

// ---------------------------------------------------------------------------
// Encodeurs serveur → client, relus par le paquet.
// ---------------------------------------------------------------------------

#[test]
fn les_constantes_d_ouverture_sont_celles_du_paquet() {
    assert_eq!(
        Ouverture::DOSSIER.disposition,
        u32::from(CreateDisposition::FILE_OPEN)
    );
    assert_eq!(
        Ouverture::ECRITURE.disposition,
        u32::from(CreateDisposition::FILE_OVERWRITE_IF)
    );
    assert!(CreateOptions::from_bits_retain(Ouverture::DOSSIER.options)
        .contains(CreateOptions::FILE_DIRECTORY_FILE));
    assert!(CreateOptions::from_bits_retain(Ouverture::LECTURE.options)
        .contains(CreateOptions::FILE_NON_DIRECTORY_FILE));
    assert!(DesiredAccess::from_bits_retain(Ouverture::LECTURE.acces)
        .contains(DesiredAccess::FILE_READ_DATA_OR_FILE_LIST_DIRECTORY));
    assert!(DesiredAccess::from_bits_retain(Ouverture::ECRITURE.acces)
        .contains(DesiredAccess::FILE_WRITE_DATA_OR_FILE_ADD_FILE));
    assert_eq!(
        Ouverture::ECRITURE.attributs,
        FileAttributes::FILE_ATTRIBUTE_NORMAL.bits()
    );
    assert_eq!(
        FILE_ATTRIBUTE_DIRECTORY,
        FileAttributes::FILE_ATTRIBUTE_DIRECTORY.bits()
    );
    assert_eq!(
        u32::from(FileInformationClassLevel::FILE_BOTH_DIRECTORY_INFORMATION),
        FILE_BOTH_DIRECTORY_INFORMATION
    );
    assert_eq!(
        FileSystemInformationClassLevel::from(FILE_FS_VOLUME_INFORMATION),
        FileSystemInformationClassLevel::FILE_FS_VOLUME_INFORMATION
    );
}

#[test]
fn l_irp_de_creation_se_relit_par_le_paquet() {
    let m = SvcMessage::from(
        irp_creation(1, 5, "\\bonjour.txt", Ouverture::LECTURE).expect("encodage"),
    );
    let ServerDriveIoRequest::ServerCreateDriveRequest(r) = irp(&m) else {
        panic!("pas une création");
    };
    assert_eq!(r.device_io_request.device_id, 1);
    assert_eq!(r.device_io_request.file_id, 0);
    assert_eq!(r.device_io_request.completion_id, 5);
    assert_eq!(r.device_io_request.major_function, MajorFunction::Create);
    assert_eq!(r.path, "\\bonjour.txt");
    assert_eq!(r.create_disposition, CreateDisposition::FILE_OPEN);
    assert_eq!(r.allocation_size, 0);
    assert_eq!(r.desired_access.bits(), Ouverture::LECTURE.acces);
    assert_eq!(r.create_options.bits(), Ouverture::LECTURE.options);
    assert_eq!(r.shared_access.bits(), 7);
}

#[test]
fn les_irp_de_lecture_ecriture_fermeture_se_relisent_par_le_paquet() {
    let m = SvcMessage::from(irp_lecture(1, 9, 6, 4096, 8192).expect("encodage"));
    let ServerDriveIoRequest::DeviceReadRequest(r) = irp(&m) else {
        panic!("pas une lecture");
    };
    assert_eq!(r.device_io_request.file_id, 9);
    assert_eq!(r.length, 4096);
    assert_eq!(r.offset, 8192);

    let m = SvcMessage::from(irp_ecriture(1, 9, 7, 3, b"abc").expect("encodage"));
    let ServerDriveIoRequest::DeviceWriteRequest(r) = irp(&m) else {
        panic!("pas une écriture");
    };
    assert_eq!(r.offset, 3);
    assert_eq!(r.write_data, b"abc");

    let m = SvcMessage::from(irp_fermeture(1, 9, 8).expect("encodage"));
    let ServerDriveIoRequest::DeviceCloseRequest(r) = irp(&m) else {
        panic!("pas une fermeture");
    };
    assert_eq!(r.device_io_request.completion_id, 8);
    // DR_CLOSE_REQ : 20 octets d'en-tête, 32 de remplissage, rien d'autre.
    assert_eq!(brut(&m).len(), 4 + 20 + 32);
}

#[test]
fn les_irp_de_volume_et_de_repertoire_se_relisent_par_le_paquet() {
    let m = SvcMessage::from(irp_volume(1, 9, 2).expect("encodage"));
    let ServerDriveIoRequest::ServerDriveQueryVolumeInformationRequest(r) = irp(&m) else {
        panic!("pas une demande de volume");
    };
    assert_eq!(
        r.fs_info_class_lvl,
        FileSystemInformationClassLevel::FILE_FS_VOLUME_INFORMATION
    );

    let m = SvcMessage::from(irp_repertoire(1, 9, 3, Some("\\*")).expect("encodage"));
    let ServerDriveIoRequest::ServerDriveQueryDirectoryRequest(r) = irp(&m) else {
        panic!("pas une énumération");
    };
    assert_eq!(
        r.file_info_class_lvl,
        FileInformationClassLevel::FILE_BOTH_DIRECTORY_INFORMATION
    );
    assert_eq!(r.initial_query, 1);
    assert_eq!(r.path, "\\*");
    assert_eq!(
        r.device_io_request.minor_function,
        MinorFunction::IRP_MN_QUERY_DIRECTORY
    );

    // Requête suivante : sans chemin, comme Windows.
    let m = SvcMessage::from(irp_repertoire(1, 9, 4, None).expect("encodage"));
    let ServerDriveIoRequest::ServerDriveQueryDirectoryRequest(r) = irp(&m) else {
        panic!("pas une énumération");
    };
    assert_eq!(r.initial_query, 0);
    assert_eq!(r.path, "");
}

// ---------------------------------------------------------------------------
// L'automate, contre des complétions simulées.
// ---------------------------------------------------------------------------

/// Joue la poignée de main jusqu'à l'annonce du lecteur `1` et rend l'IRP
/// d'ouverture de `\` que l'automate émet en retour.
fn poignee_de_main(scenario: &mut Scenario) -> Vec<SvcMessage> {
    let demarrage = scenario.demarrer();
    assert_eq!(demarrage.len(), 1);
    let RdpdrPdu::VersionAndIdPdu(annonce) =
        decode::<RdpdrPdu>(&brut(&demarrage[0])).expect("annonce")
    else {
        panic!("pas une annonce");
    };
    assert_eq!(annonce.kind, VersionAndIdPduKind::ServerAnnounceRequest);
    assert_eq!(annonce.version_minor, VERSION_MINEURE);
    assert_eq!(annonce.client_id, CLIENT_ID);

    let reponse = scenario.recevoir(&octets(&RdpdrPdu::VersionAndIdPdu(VersionAndIdPdu {
        version_major: VERSION_MAJOR,
        version_minor: 0x000C,
        client_id: CLIENT_ID,
        kind: VersionAndIdPduKind::ClientAnnounceReply,
    })));
    assert!(reponse.is_empty());

    let reponse = scenario.recevoir(&octets(&RdpdrPdu::ClientNameRequest(
        ClientNameRequest::new("poste".to_owned(), ClientNameRequestUnicodeFlag::Unicode),
    )));
    assert_eq!(reponse.len(), 1);
    let RdpdrPdu::CoreCapability(capacites) =
        decode::<RdpdrPdu>(&brut(&reponse[0])).expect("capacités")
    else {
        panic!("pas une demande de capacités");
    };
    assert_eq!(capacites.capabilities.len(), 4);

    let mut mes_capacites = Capabilities::new();
    mes_capacites.add_drive();
    let reponse = scenario.recevoir(&octets(&RdpdrPdu::CoreCapability(
        CoreCapability::new_response(mes_capacites.clone_inner()),
    )));
    assert_eq!(reponse.len(), 2, "confirmation puis UserLoggedOn");
    let RdpdrPdu::VersionAndIdPdu(confirmation) =
        decode::<RdpdrPdu>(&brut(&reponse[0])).expect("confirmation")
    else {
        panic!("pas une confirmation");
    };
    assert_eq!(
        confirmation.kind,
        VersionAndIdPduKind::ServerClientIdConfirm
    );
    assert!(matches!(
        decode::<RdpdrPdu>(&brut(&reponse[1])).expect("UserLoggedOn"),
        RdpdrPdu::UserLoggedon
    ));

    let mut client = Rdpdr::new(Box::new(NoopRdpdrBackend), "poste".to_owned());
    let reponse = scenario.recevoir(&octets(&RdpdrPdu::ClientDeviceListAnnounce(
        client.add_drive(1, "Avash".to_owned()),
    )));
    assert_eq!(
        scenario.lignes(),
        vec!["rdpdr: lecteur Avash annoncé (id 1)".to_owned()]
    );
    assert_eq!(reponse.len(), 2, "réponse d'annonce puis IRP d'ouverture");
    let RdpdrPdu::ServerDeviceAnnounceResponse(accuse) =
        decode::<RdpdrPdu>(&brut(&reponse[0])).expect("accusé")
    else {
        panic!("pas un accusé");
    };
    assert_eq!(
        accuse,
        ServerDeviceAnnounceResponse {
            device_id: 1,
            result_code: NtStatus::SUCCESS
        }
    );
    reponse
}

fn ouverture_de(message: &SvcMessage, chemin: &str) -> DeviceIoRequest {
    let ServerDriveIoRequest::ServerCreateDriveRequest(r) = irp(message) else {
        panic!("pas une ouverture");
    };
    assert_eq!(r.path, chemin);
    r.device_io_request
}

/// Répond à une ouverture par le `FileId` donné.
fn ouvert(scenario: &mut Scenario, ouverture: DeviceIoRequest, file_id: u32) -> Vec<SvcMessage> {
    scenario.recevoir(&octets(&RdpdrPdu::DeviceCreateResponse(
        DeviceCreateResponse {
            device_io_reply: DeviceIoResponse::new(ouverture, NtStatus::SUCCESS),
            file_id,
            information: Information::FILE_OPENED,
        },
    )))
}

/// Répond à une fermeture, en vérifiant le `FileId` fermé.
fn ferme(scenario: &mut Scenario, message: &SvcMessage, file_id: u32) -> Vec<SvcMessage> {
    let ServerDriveIoRequest::DeviceCloseRequest(fermeture) = irp(message) else {
        panic!("pas une fermeture");
    };
    assert_eq!(fermeture.device_io_request.file_id, file_id);
    scenario.recevoir(&octets(&RdpdrPdu::DeviceCloseResponse(
        DeviceCloseResponse {
            device_io_response: DeviceIoResponse::new(
                fermeture.device_io_request,
                NtStatus::SUCCESS,
            ),
        },
    )))
}

/// Poignée de main, ouverture de `\` (`FileId` 40), volume `AVASH`, énumération
/// des `entrees` puis `STATUS_NO_MORE_FILES`, fermeture : rend l'ouverture de
/// `\bonjour.txt` que l'automate émet ensuite, en vérifiant chaque IRP.
fn jusqu_a_bonjour(
    scenario: &mut Scenario,
    entrees: &[(&str, i64, FileAttributes)],
) -> DeviceIoRequest {
    let reponse = poignee_de_main(scenario);
    let ouverture = ouverture_de(&reponse[1], "\\");
    assert_eq!(ouverture.completion_id, 1);

    let reponse = ouvert(scenario, ouverture, 40);
    assert_eq!(reponse.len(), 1);
    let ServerDriveIoRequest::ServerDriveQueryVolumeInformationRequest(volume) = irp(&reponse[0])
    else {
        panic!("pas une demande de volume");
    };
    assert_eq!(volume.device_io_request.file_id, 40);
    assert_eq!(volume.device_io_request.completion_id, 2);

    let reponse = scenario.recevoir(&octets(
        &RdpdrPdu::ClientDriveQueryVolumeInformationResponse(
            ClientDriveQueryVolumeInformationResponse::new(
                volume.device_io_request,
                NtStatus::SUCCESS,
                Some(
                    FileFsVolumeInformation {
                        volume_creation_time: 0,
                        volume_serial_number: 1,
                        supports_objects: Boolean::False,
                        volume_label: "AVASH".to_owned(),
                    }
                    .into(),
                ),
            ),
        ),
    ));
    assert_eq!(scenario.lignes(), vec!["rdpdr: volume AVASH".to_owned()]);
    let ServerDriveIoRequest::ServerDriveQueryDirectoryRequest(liste) = irp(&reponse[0]) else {
        panic!("pas une énumération");
    };
    assert_eq!(liste.initial_query, 1);
    assert_eq!(liste.path, "\\*");

    // Une requête suivante par entrée, sans chemin ; puis STATUS_NO_MORE_FILES
    // amène la fermeture de \.
    let mut requete = liste.device_io_request;
    for (nom, taille, attributs) in entrees {
        let reponse = scenario.recevoir(&octets(&RdpdrPdu::ClientDriveQueryDirectoryResponse(
            ClientDriveQueryDirectoryResponse {
                device_io_reply: DeviceIoResponse::new(requete, NtStatus::SUCCESS),
                buffer: Some(
                    FileBothDirectoryInformation::new(
                        0,
                        0,
                        0,
                        0,
                        *taille,
                        attributs.clone(),
                        (*nom).to_owned(),
                    )
                    .into(),
                ),
            },
        )));
        let ServerDriveIoRequest::ServerDriveQueryDirectoryRequest(suivante) = irp(&reponse[0])
        else {
            panic!("pas une énumération");
        };
        assert_eq!(suivante.initial_query, 0);
        assert_eq!(suivante.path, "");
        requete = suivante.device_io_request;
    }
    let reponse = scenario.recevoir(&octets(&RdpdrPdu::ClientDriveQueryDirectoryResponse(
        ClientDriveQueryDirectoryResponse {
            device_io_reply: DeviceIoResponse::new(requete, NtStatus::NO_MORE_FILES),
            buffer: None,
        },
    )));
    let reponse = ferme(scenario, &reponse[0], 40);
    ouverture_de(&reponse[0], "\\bonjour.txt")
}

/// Sert `contenu` comme `\bonjour.txt` (`FileId` 41) par morceaux de `MORCEAU`,
/// vérifie la ligne d'empreinte, répond à la fermeture : rend la création de
/// `\ecrit.txt` que l'automate émet ensuite.
fn lire_bonjour(
    scenario: &mut Scenario,
    ouverture: DeviceIoRequest,
    contenu: &[u8],
) -> DeviceIoRequest {
    let morceau = usize::try_from(MORCEAU).unwrap();
    let mut reponse = ouvert(scenario, ouverture, 41);
    let mut position = 0usize;
    loop {
        let ServerDriveIoRequest::DeviceReadRequest(lecture) = irp(&reponse[0]) else {
            panic!("pas une lecture");
        };
        assert_eq!(lecture.device_io_request.file_id, 41);
        assert_eq!(lecture.length, MORCEAU);
        assert_eq!(lecture.offset, u64::try_from(position).unwrap());
        let fin = (position + morceau).min(contenu.len());
        reponse = scenario.recevoir(&octets(&RdpdrPdu::DeviceReadResponse(DeviceReadResponse {
            device_io_reply: DeviceIoResponse::new(lecture.device_io_request, NtStatus::SUCCESS),
            read_data: contenu[position..fin].to_vec(),
        })));
        let court = fin - position < morceau;
        position = fin;
        if court {
            break;
        }
        assert!(
            scenario.lignes().is_empty(),
            "pas de ligne avant la fin de la lecture"
        );
    }
    assert_eq!(
        scenario.lignes(),
        vec![format!(
            "rdpdr: lu bonjour.txt {} octets sha256={}",
            contenu.len(),
            hex(&Sha256::digest(contenu))
        )]
    );
    let reponse = ferme(scenario, &reponse[0], 41);
    let ServerDriveIoRequest::ServerCreateDriveRequest(creation) = irp(&reponse[0]) else {
        panic!("pas une création");
    };
    assert_eq!(creation.path, "\\ecrit.txt");
    assert_eq!(
        creation.create_disposition,
        CreateDisposition::FILE_OVERWRITE_IF
    );
    creation.device_io_request
}

/// Répond à la création de `\ecrit.txt` (`FileId` 42), vérifie l'écriture
/// demandée et y répond avec `longueur_ecrite` : rend ce que l'automate émet.
fn ecrire(
    scenario: &mut Scenario,
    creation: DeviceIoRequest,
    longueur_ecrite: u32,
) -> Vec<SvcMessage> {
    let reponse = ouvert(scenario, creation, 42);
    let ServerDriveIoRequest::DeviceWriteRequest(ecriture) = irp(&reponse[0]) else {
        panic!("pas une écriture");
    };
    assert_eq!(ecriture.device_io_request.file_id, 42);
    assert_eq!(ecriture.offset, 0);
    assert_eq!(ecriture.write_data, CONTENU_ECRIT);
    scenario.recevoir(&octets(&RdpdrPdu::DeviceWriteResponse(
        DeviceWriteResponse {
            device_io_reply: DeviceIoResponse::new(ecriture.device_io_request, NtStatus::SUCCESS),
            length: longueur_ecrite,
        },
    )))
}

#[test]
fn le_scenario_emet_les_irp_attendus_et_les_lignes_attendues() {
    let mut scenario = Scenario::new();
    let ouverture = jusqu_a_bonjour(
        &mut scenario,
        &[
            (".", 0, FileAttributes::FILE_ATTRIBUTE_DIRECTORY),
            ("bonjour.txt", 8, FileAttributes::FILE_ATTRIBUTE_NORMAL),
        ],
    );
    assert_eq!(
        scenario.lignes(),
        vec![
            "rdpdr: entrée . 0 dir".to_owned(),
            "rdpdr: entrée bonjour.txt 8 fichier".to_owned()
        ]
    );

    // Un premier morceau plein (4096 octets), un second court : deux
    // lectures, la seconde à l'offset 4096, puis la ligne avec l'empreinte.
    let contenu: Vec<u8> = (0..5000u32)
        .map(|i| u8::try_from(i % 251).unwrap())
        .collect();
    let creation = lire_bonjour(&mut scenario, ouverture, &contenu);

    let reponse = ecrire(
        &mut scenario,
        creation,
        u32::try_from(CONTENU_ECRIT.len()).unwrap(),
    );
    assert_eq!(scenario.lignes(), vec!["rdpdr: écrit ecrit.txt".to_owned()]);
    let reponse = ferme(&mut scenario, &reponse[0], 42);
    assert!(reponse.is_empty());
    assert_eq!(
        scenario.lignes(),
        vec!["rdpdr: scénario terminé".to_owned()]
    );
    assert!(scenario.termine());
}
#[test]
fn une_ouverture_refusee_arrete_le_scenario_en_le_disant() {
    let mut scenario = Scenario::new();
    let reponse = poignee_de_main(&mut scenario);
    let ouverture = ouverture_de(&reponse[1], "\\");
    let reponse = scenario.recevoir(&octets(&RdpdrPdu::DeviceCreateResponse(
        DeviceCreateResponse {
            device_io_reply: DeviceIoResponse::new(ouverture, NtStatus::NO_SUCH_FILE),
            file_id: 0,
            information: Information::FILE_SUPERSEDED,
        },
    )));
    assert!(reponse.is_empty(), "plus rien n'est émis");
    assert_eq!(
        scenario.lignes(),
        vec!["rdpdr: échec ouverture de \\ : STATUS_NO_SUCH_FILE".to_owned()]
    );
    assert!(scenario.termine());

    // Une complétion tardive ne relance rien.
    let reponse = scenario.recevoir(&octets(&RdpdrPdu::DeviceCloseResponse(
        DeviceCloseResponse {
            device_io_response: completion(1, 1, NtStatus::SUCCESS),
        },
    )));
    assert!(reponse.is_empty());
    assert!(scenario.lignes().is_empty());
}

#[test]
fn une_ecriture_courte_est_un_echec() {
    let mut scenario = Scenario::new();
    let ouverture = jusqu_a_bonjour(&mut scenario, &[]);
    // Fichier vide : une lecture vide, donc courte.
    let creation = lire_bonjour(&mut scenario, ouverture, b"");
    let reponse = ecrire(&mut scenario, creation, 3);
    assert!(reponse.is_empty(), "plus rien n'est émis");
    assert_eq!(
        scenario.lignes(),
        vec!["rdpdr: échec écriture de ecrit.txt : 3 octets écrits sur 18".to_owned()]
    );
    assert!(scenario.termine());
}
#[test]
fn une_completion_d_un_autre_irp_est_ignoree() {
    let mut scenario = Scenario::new();
    let _ = poignee_de_main(&mut scenario);
    // Mauvais CompletionId, puis mauvais DeviceId : rien ne bouge.
    for (device_id, completion_id) in [(1, 99), (2, 1)] {
        let reponse = scenario.recevoir(&octets(&RdpdrPdu::DeviceCreateResponse(
            DeviceCreateResponse {
                device_io_reply: completion(device_id, completion_id, NtStatus::SUCCESS),
                file_id: 40,
                information: Information::FILE_OPENED,
            },
        )));
        assert!(reponse.is_empty());
        assert!(scenario.lignes().is_empty());
        assert!(!scenario.termine());
    }
}

#[test]
fn un_pdu_illisible_arrete_le_scenario_en_le_disant() {
    let mut scenario = Scenario::new();
    let _ = poignee_de_main(&mut scenario);
    // En-tête de complétion valide, corps de DR_CREATE_RSP absent.
    let mut o = Vec::new();
    o.extend_from_slice(&0x4472u16.to_le_bytes());
    o.extend_from_slice(&0x4943u16.to_le_bytes());
    o.extend_from_slice(&1u32.to_le_bytes());
    o.extend_from_slice(&1u32.to_le_bytes());
    o.extend_from_slice(&0u32.to_le_bytes());
    let reponse = scenario.recevoir(&o);
    assert!(reponse.is_empty());
    let lignes = scenario.lignes();
    assert_eq!(lignes.len(), 1);
    assert!(
        lignes[0].starts_with("rdpdr: échec ouverture de \\ : "),
        "{lignes:?}"
    );
    assert!(scenario.termine());

    // Un en-tête RDPDR inconnu aussi, à n'importe quelle étape.
    let mut scenario = Scenario::new();
    let _ = scenario.demarrer();
    let reponse = scenario.recevoir(&[0xFF, 0xFF, 0xFF, 0xFF]);
    assert!(reponse.is_empty());
    assert!(scenario.lignes()[0].starts_with("rdpdr: échec en-tête : "));
}

#[test]
fn sans_user_logged_on_cote_client_le_serveur_ne_l_envoie_pas() {
    // GENERAL_CAPS_SET composé à la main, extendedPDU sans
    // RDPDR_USER_LOGGEDON_PDU (2.2.2.7.1) : le client ne veut pas du PDU, le
    // serveur ne l'envoie pas (3.3.5.1.7).
    let mut scenario = Scenario::new();
    let _ = scenario.demarrer();
    let _ = scenario.recevoir(&octets(&RdpdrPdu::ClientNameRequest(
        ClientNameRequest::new("poste".to_owned(), ClientNameRequestUnicodeFlag::Ascii),
    )));
    let mut o = Vec::new();
    o.extend_from_slice(&0x4472u16.to_le_bytes());
    o.extend_from_slice(&0x4350u16.to_le_bytes());
    o.extend_from_slice(&1u16.to_le_bytes());
    o.extend_from_slice(&0u16.to_le_bytes());
    o.extend_from_slice(&CAP_GENERAL_TYPE.to_le_bytes());
    o.extend_from_slice(&44u16.to_le_bytes());
    o.extend_from_slice(&2u32.to_le_bytes());
    o.extend_from_slice(&0u32.to_le_bytes()); // osType
    o.extend_from_slice(&0u32.to_le_bytes()); // osVersion
    o.extend_from_slice(&1u16.to_le_bytes());
    o.extend_from_slice(&0x000Cu16.to_le_bytes());
    o.extend_from_slice(&0xFFFFu32.to_le_bytes()); // ioCode1
    o.extend_from_slice(&0u32.to_le_bytes());
    o.extend_from_slice(&0x0000_0003u32.to_le_bytes()); // extendedPDU sans USER_LOGGEDON
    o.extend_from_slice(&0u32.to_le_bytes());
    o.extend_from_slice(&0u32.to_le_bytes());
    o.extend_from_slice(&0u32.to_le_bytes());
    let reponse = scenario.recevoir(&o);
    assert_eq!(reponse.len(), 1, "la confirmation seule");
    assert!(matches!(
        decode::<RdpdrPdu>(&brut(&reponse[0])).expect("confirmation"),
        RdpdrPdu::VersionAndIdPdu(_)
    ));
}

#[test]
fn une_fermeture_en_erreur_est_toleree() {
    // Vu sur le fil avec xfreerdp3 3.31 : après STATUS_NO_MORE_FILES sur
    // l'énumération, la complétion de la fermeture de \ porte encore
    // STATUS_NO_MORE_FILES (la dernière erreur du fil de FreeRDP). Le
    // scénario doit passer outre et ouvrir \bonjour.txt.
    let mut scenario = Scenario::new();
    let reponse = poignee_de_main(&mut scenario);
    let ouverture = ouverture_de(&reponse[1], "\\");
    let reponse = ouvert(&mut scenario, ouverture, 40);
    let ServerDriveIoRequest::ServerDriveQueryVolumeInformationRequest(volume) = irp(&reponse[0])
    else {
        panic!("pas une demande de volume");
    };
    let reponse = scenario.recevoir(&octets(
        &RdpdrPdu::ClientDriveQueryVolumeInformationResponse(
            ClientDriveQueryVolumeInformationResponse::new(
                volume.device_io_request,
                NtStatus::SUCCESS,
                Some(
                    FileFsVolumeInformation {
                        volume_creation_time: 0,
                        volume_serial_number: 1,
                        supports_objects: Boolean::False,
                        volume_label: "FreeRDP".to_owned(),
                    }
                    .into(),
                ),
            ),
        ),
    ));
    let ServerDriveIoRequest::ServerDriveQueryDirectoryRequest(liste) = irp(&reponse[0]) else {
        panic!("pas une énumération");
    };
    let reponse = scenario.recevoir(&octets(&RdpdrPdu::ClientDriveQueryDirectoryResponse(
        ClientDriveQueryDirectoryResponse {
            device_io_reply: DeviceIoResponse::new(
                liste.device_io_request,
                NtStatus::NO_MORE_FILES,
            ),
            buffer: None,
        },
    )));
    let ServerDriveIoRequest::DeviceCloseRequest(fermeture) = irp(&reponse[0]) else {
        panic!("pas une fermeture");
    };
    // Les octets exacts de FreeRDP : en-tête de complétion, statut
    // STATUS_NO_MORE_FILES, cinq octets de remplissage (le paquet n'en met
    // que quatre).
    let mut o = Vec::new();
    o.extend_from_slice(&0x4472u16.to_le_bytes());
    o.extend_from_slice(&0x4943u16.to_le_bytes());
    o.extend_from_slice(&1u32.to_le_bytes());
    o.extend_from_slice(&fermeture.device_io_request.completion_id.to_le_bytes());
    o.extend_from_slice(&0x8000_0006u32.to_le_bytes());
    o.extend_from_slice(&[0; 5]);
    let reponse = scenario.recevoir(&o);
    assert_eq!(reponse.len(), 1, "le scénario continue");
    let _ = ouverture_de(&reponse[0], "\\bonjour.txt");
    assert!(scenario.lignes().iter().all(|l| !l.contains("échec")));
    assert!(!scenario.termine());
}

// ---------------------------------------------------------------------------
// Dialogue complet avec le client du paquet.
// ---------------------------------------------------------------------------

/// Un fichier ouvert côté client : son chemin et, pour un dossier, la liste
/// des entrées en cours d'énumération.
#[derive(Debug)]
struct Ouvert {
    chemin: PathBuf,
    entrees: Vec<(String, u64, bool)>,
    curseur: usize,
}

/// Un `RdpdrBackend` minimal qui sert un dossier du poste : ouverture,
/// volume, énumération, lecture, écriture, fermeture.
#[derive(Debug)]
struct DossierPartage {
    racine: PathBuf,
    ouverts: HashMap<u32, Ouvert>,
    prochain: u32,
}

impl_as_any!(DossierPartage);

impl DossierPartage {
    fn new(racine: &Path) -> Self {
        Self {
            racine: racine.to_path_buf(),
            ouverts: HashMap::new(),
            prochain: 100,
        }
    }

    fn chemin_local(&self, chemin_rdp: &str) -> PathBuf {
        let mut chemin = self.racine.clone();
        for composant in chemin_rdp.split('\\').filter(|c| !c.is_empty()) {
            chemin.push(composant);
        }
        chemin
    }

    fn ouvrir(&mut self, chemin: PathBuf) -> u32 {
        let id = self.prochain;
        self.prochain += 1;
        self.ouverts.insert(
            id,
            Ouvert {
                chemin,
                entrees: Vec::new(),
                curseur: 0,
            },
        );
        id
    }

    fn lister(chemin: &Path) -> Vec<(String, u64, bool)> {
        let mut entrees: Vec<_> = fs::read_dir(chemin)
            .expect("lecture du dossier")
            .map(|e| {
                let e = e.expect("entrée");
                let m = e.metadata().expect("métadonnées");
                // Un dossier a une taille sur disque sous Linux (40 octets
                // ici) ; on rend 0, comme un client Windows.
                let taille = if m.is_dir() { 0 } else { m.len() };
                (
                    e.file_name().to_string_lossy().into_owned(),
                    taille,
                    m.is_dir(),
                )
            })
            .collect();
        entrees.sort();
        entrees
    }

    fn creer(&mut self, r: ironrdp::rdpdr::pdu::efs::DeviceCreateRequest) -> RdpdrPdu {
        let chemin = self.chemin_local(&r.path);
        let (statut, file_id) = if r.create_disposition == CreateDisposition::FILE_OPEN {
            if chemin.exists() {
                (NtStatus::SUCCESS, self.ouvrir(chemin))
            } else {
                (NtStatus::NO_SUCH_FILE, 0)
            }
        } else {
            // FILE_OVERWRITE_IF et les autres : on crée ou on tronque.
            match fs::File::create(&chemin) {
                Ok(_) => (NtStatus::SUCCESS, self.ouvrir(chemin)),
                Err(_) => (NtStatus::ACCESS_DENIED, 0),
            }
        };
        RdpdrPdu::DeviceCreateResponse(DeviceCreateResponse {
            device_io_reply: DeviceIoResponse::new(r.device_io_request, statut),
            file_id,
            information: Information::FILE_OPENED,
        })
    }

    fn enumerer(
        &mut self,
        r: ironrdp::rdpdr::pdu::efs::ServerDriveQueryDirectoryRequest,
    ) -> RdpdrPdu {
        let Some(ouvert) = self.ouverts.get_mut(&r.device_io_request.file_id) else {
            return RdpdrPdu::ClientDriveQueryDirectoryResponse(
                ClientDriveQueryDirectoryResponse {
                    device_io_reply: DeviceIoResponse::new(
                        r.device_io_request,
                        NtStatus::NO_SUCH_FILE,
                    ),
                    buffer: None,
                },
            );
        };
        if r.initial_query != 0 {
            ouvert.entrees = Self::lister(&ouvert.chemin);
            ouvert.curseur = 0;
        }
        match ouvert.entrees.get(ouvert.curseur) {
            Some((nom, taille, dossier)) => {
                ouvert.curseur += 1;
                let attributs = if *dossier {
                    FileAttributes::FILE_ATTRIBUTE_DIRECTORY
                } else {
                    FileAttributes::FILE_ATTRIBUTE_NORMAL
                };
                RdpdrPdu::ClientDriveQueryDirectoryResponse(ClientDriveQueryDirectoryResponse {
                    device_io_reply: DeviceIoResponse::new(r.device_io_request, NtStatus::SUCCESS),
                    buffer: Some(
                        FileBothDirectoryInformation::new(
                            0,
                            0,
                            0,
                            0,
                            i64::try_from(*taille).unwrap(),
                            attributs,
                            nom.clone(),
                        )
                        .into(),
                    ),
                })
            }
            None => {
                RdpdrPdu::ClientDriveQueryDirectoryResponse(ClientDriveQueryDirectoryResponse {
                    device_io_reply: DeviceIoResponse::new(
                        r.device_io_request,
                        NtStatus::NO_MORE_FILES,
                    ),
                    buffer: None,
                })
            }
        }
    }
}

impl RdpdrBackend for DossierPartage {
    fn handle_server_device_announce_response(
        &mut self,
        pdu: ServerDeviceAnnounceResponse,
    ) -> PduResult<()> {
        assert_eq!(pdu.result_code, NtStatus::SUCCESS);
        Ok(())
    }

    fn handle_scard_call(
        &mut self,
        _req: DeviceControlRequest<ScardIoCtlCode>,
        _call: ScardCall,
    ) -> PduResult<()> {
        Ok(())
    }

    fn handle_drive_io_request(&mut self, req: ServerDriveIoRequest) -> PduResult<Vec<SvcMessage>> {
        let pdu = match req {
            ServerDriveIoRequest::ServerCreateDriveRequest(r) => self.creer(r),
            ServerDriveIoRequest::ServerDriveQueryVolumeInformationRequest(r) => {
                assert_eq!(
                    r.fs_info_class_lvl,
                    FileSystemInformationClassLevel::FILE_FS_VOLUME_INFORMATION
                );
                RdpdrPdu::ClientDriveQueryVolumeInformationResponse(
                    ClientDriveQueryVolumeInformationResponse::new(
                        r.device_io_request,
                        NtStatus::SUCCESS,
                        Some(
                            FileFsVolumeInformation {
                                volume_creation_time: 0,
                                volume_serial_number: 0x1234_5678,
                                supports_objects: Boolean::False,
                                volume_label: "AVASH".to_owned(),
                            }
                            .into(),
                        ),
                    ),
                )
            }
            ServerDriveIoRequest::ServerDriveQueryDirectoryRequest(r) => self.enumerer(r),
            ServerDriveIoRequest::DeviceReadRequest(r) => {
                let ouvert = &self.ouverts[&r.device_io_request.file_id];
                let mut fichier = fs::File::open(&ouvert.chemin).expect("ouverture");
                fichier
                    .seek(SeekFrom::Start(r.offset))
                    .expect("positionnement");
                let mut donnees = vec![0; usize::try_from(r.length).unwrap()];
                let mut lu = 0;
                loop {
                    let n = fichier.read(&mut donnees[lu..]).expect("lecture");
                    if n == 0 {
                        break;
                    }
                    lu += n;
                }
                donnees.truncate(lu);
                RdpdrPdu::DeviceReadResponse(DeviceReadResponse {
                    device_io_reply: DeviceIoResponse::new(r.device_io_request, NtStatus::SUCCESS),
                    read_data: donnees,
                })
            }
            ServerDriveIoRequest::DeviceWriteRequest(r) => {
                let ouvert = &self.ouverts[&r.device_io_request.file_id];
                let mut fichier = fs::OpenOptions::new()
                    .write(true)
                    .open(&ouvert.chemin)
                    .expect("ouverture en écriture");
                fichier
                    .seek(SeekFrom::Start(r.offset))
                    .expect("positionnement");
                fichier.write_all(&r.write_data).expect("écriture");
                RdpdrPdu::DeviceWriteResponse(DeviceWriteResponse {
                    device_io_reply: DeviceIoResponse::new(r.device_io_request, NtStatus::SUCCESS),
                    length: u32::try_from(r.write_data.len()).unwrap(),
                })
            }
            ServerDriveIoRequest::DeviceCloseRequest(r) => {
                self.ouverts.remove(&r.device_io_request.file_id);
                RdpdrPdu::DeviceCloseResponse(DeviceCloseResponse {
                    device_io_response: DeviceIoResponse::new(
                        r.device_io_request,
                        NtStatus::SUCCESS,
                    ),
                })
            }
            autre => panic!("IRP que le scénario n'émet pas : {autre:?}"),
        };
        Ok(vec![SvcMessage::from(pdu)])
    }
}

/// Relie les deux `process()` sans réseau : chaque message de l'un devient
/// l'entrée de l'autre, jusqu'au silence. Rend les lignes du serveur.
fn dialogue(serveur: &mut Scenario, client: &mut Rdpdr) -> Vec<String> {
    let mut file: VecDeque<(bool, Vec<u8>)> = VecDeque::new();
    for m in serveur.demarrer() {
        file.push_back((true, brut(&m)));
    }
    let mut tours = 0;
    while let Some((vers_client, o)) = file.pop_front() {
        tours += 1;
        assert!(tours < 10_000, "dialogue sans fin");
        if vers_client {
            for m in client
                .process(&o)
                .expect("le client refuse un PDU du serveur")
            {
                file.push_back((false, brut(&m)));
            }
        } else {
            for m in serveur.recevoir(&o) {
                file.push_back((true, brut(&m)));
            }
        }
    }
    serveur.lignes()
}

fn client_avec_lecteur(racine: &Path) -> Rdpdr {
    Rdpdr::new(Box::new(DossierPartage::new(racine)), "poste".to_owned())
        .with_drives(Some(vec![(1, "Avash".to_owned())]))
}

#[test]
fn le_dialogue_avec_le_client_du_paquet_produit_les_six_lignes() {
    let racine = dossier_temporaire("dialogue");
    fs::write(racine.join("bonjour.txt"), b"bonjour\n").unwrap();
    fs::create_dir(racine.join("dossier")).unwrap();

    let mut serveur = Scenario::new();
    let mut client = client_avec_lecteur(&racine);
    let lignes = dialogue(&mut serveur, &mut client);

    let empreinte = hex(&Sha256::digest(b"bonjour\n"));
    assert_eq!(
        lignes,
        vec![
            "rdpdr: lecteur Avash annoncé (id 1)".to_owned(),
            "rdpdr: volume AVASH".to_owned(),
            "rdpdr: entrée bonjour.txt 8 fichier".to_owned(),
            "rdpdr: entrée dossier 0 dir".to_owned(),
            format!("rdpdr: lu bonjour.txt 8 octets sha256={empreinte}"),
            "rdpdr: écrit ecrit.txt".to_owned(),
            "rdpdr: scénario terminé".to_owned(),
        ]
    );
    assert!(serveur.termine());
    assert_eq!(fs::read(racine.join("ecrit.txt")).unwrap(), CONTENU_ECRIT);
    // Tout est fermé côté client.
    let backend = client
        .downcast_backend::<DossierPartage>()
        .expect("backend");
    assert!(backend.ouverts.is_empty());
    let _ = fs::remove_dir_all(&racine);
}

#[test]
fn un_fichier_de_plusieurs_morceaux_est_lu_en_entier() {
    let racine = dossier_temporaire("morceaux");
    let contenu: Vec<u8> = (0..10_000u32)
        .map(|i| u8::try_from(i % 253).unwrap())
        .collect();
    fs::write(racine.join("bonjour.txt"), &contenu).unwrap();

    let mut serveur = Scenario::new();
    let mut client = client_avec_lecteur(&racine);
    let lignes = dialogue(&mut serveur, &mut client);

    let empreinte = hex(&Sha256::digest(&contenu));
    assert!(
        lignes.contains(&format!(
            "rdpdr: lu bonjour.txt 10000 octets sha256={empreinte}"
        )),
        "{lignes:?}"
    );
    assert_eq!(lignes.last().unwrap(), "rdpdr: scénario terminé");
    let _ = fs::remove_dir_all(&racine);
}

#[test]
fn sans_bonjour_txt_le_scenario_s_arrete_a_l_ouverture() {
    let racine = dossier_temporaire("sans-bonjour");
    let mut serveur = Scenario::new();
    let mut client = client_avec_lecteur(&racine);
    let lignes = dialogue(&mut serveur, &mut client);
    assert_eq!(
        lignes.last().unwrap(),
        "rdpdr: échec ouverture de \\bonjour.txt : STATUS_NO_SUCH_FILE"
    );
    assert!(serveur.termine());
    assert!(!racine.join("ecrit.txt").exists());
    let _ = fs::remove_dir_all(&racine);
}

#[test]
fn un_client_sans_lecteur_laisse_le_scenario_inerte() {
    let racine = dossier_temporaire("sans-lecteur");
    let mut serveur = Scenario::new();
    let mut client = Rdpdr::new(Box::new(DossierPartage::new(&racine)), "poste".to_owned());
    let lignes = dialogue(&mut serveur, &mut client);
    assert!(lignes.is_empty(), "{lignes:?}");
    assert!(!serveur.termine());
    let _ = fs::remove_dir_all(&racine);
}
