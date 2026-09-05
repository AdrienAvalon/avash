//! Côté serveur de la redirection de lecteur (canal statique `rdpdr`,
//! [MS-RDPEFS]) : la poignée de main, puis un scénario fixe sur le premier
//! lecteur que le client annonce, avec une ligne sur la sortie standard à
//! chaque étape. C'est par ces lignes que la suite bout en bout vérifie que
//! le lecteur du sidecar est bien servi, comme elle lit celles du serveur VNC
//! de test.
//!
//! Tout est chaîné : le serveur n'émet un IRP qu'en réponse à la complétion
//! du précédent, jamais de lui-même. L'automate (`Scenario`) ne fait aucune
//! entrée-sortie, il rend des messages et des lignes ; le canal
//! (`CanalRdpdr`) les imprime. Le scénario :
//!
//! 1. poignée de main (3.3.5.1) ; ligne `rdpdr: lecteur <nom> annoncé (id <n>)` ;
//! 2. ouverture de `\`, volume (`rdpdr: volume <étiquette>`) ;
//! 3. énumération de `\*` (`rdpdr: entrée <nom> <taille> <dir|fichier>`), fermeture ;
//! 4. lecture de `\bonjour.txt` par morceaux de 4096 octets
//!    (`rdpdr: lu bonjour.txt <n> octets sha256=<hex>`) ;
//! 5. écriture de `depuis le serveur\n` dans `\ecrit.txt` (`rdpdr: écrit ecrit.txt`) ;
//! 6. `rdpdr: scénario terminé`.
//!
//! Une complétion en échec ou un PDU illisible donne `rdpdr: échec <étape> :
//! <détail>` et arrête le scénario ; le canal reste ouvert, le serveur ne
//! tombe pas.
//!
//! [MS-RDPEFS]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpefs/34d9de58-b2b5-40b6-b970-f82d4603bdb5

mod pdu;
#[cfg(test)]
mod tests;

use core::fmt::Display;

use ironrdp::core::{EncodeResult, ReadCursor};
use ironrdp::pdu::gcc::ChannelName;
use ironrdp::pdu::PduResult;
use ironrdp::rdpdr::pdu::efs::{DeviceIoResponse, NtStatus};
use ironrdp::rdpdr::pdu::{PacketId, SharedHeader};
use ironrdp::rdpdr::Rdpdr;
use ironrdp::server::SvcServerFactory;
use ironrdp::svc::{SvcMessage, SvcProcessor, SvcServerProcessor};
use sha2::{Digest as _, Sha256};
use tracing::{debug, trace};

use self::pdu::{
    annonce_serveur, confirmation_client_id, demande_capacites, irp_creation, irp_ecriture,
    irp_fermeture, irp_lecture, irp_repertoire, irp_volume, reponse_annonce, user_logged_on,
    AnnonceClient, AnnoncePeripheriques, CapacitesClient, EntreeRepertoire, InfosVolume, Irp,
    NomClient, Ouverture, ReponseCreation, ReponseEcriture, ReponseLecture,
};

/// Taille des lectures : le scénario lit jusqu'à un morceau court ou vide.
pub const MORCEAU: u32 = 4096;

/// L'empreinte en hexadécimal minuscule, telle que `sha256sum` l'écrit.
fn hex(octets: &[u8]) -> String {
    use core::fmt::Write as _;
    octets.iter().fold(String::new(), |mut sortie, octet| {
        // Écrire dans une String ne peut pas échouer.
        let _ = write!(sortie, "{octet:02x}");
        sortie
    })
}
/// Ce que le serveur écrit dans `\ecrit.txt`.
pub const CONTENU_ECRIT: &[u8] = b"depuis le serveur\n";

/// Où en est le scénario. À partir d'`OuvreRacine`, chaque étape est l'IRP en
/// vol dont on attend la complétion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Etape {
    Annonce,
    Nom,
    Capacites,
    Peripheriques,
    OuvreRacine,
    Volume,
    Liste,
    FermeRacine,
    OuvreBonjour,
    Lit,
    FermeBonjour,
    CreeEcrit,
    Ecrit,
    FermeEcrit,
    Termine,
}

impl Etape {
    /// Le nom de l'étape dans la ligne `rdpdr: échec <étape> : <détail>`.
    fn libelle(self) -> &'static str {
        match self {
            Etape::Annonce => "annonce",
            Etape::Nom => "nom du client",
            Etape::Capacites => "capacités",
            Etape::Peripheriques => "annonce des périphériques",
            Etape::OuvreRacine => "ouverture de \\",
            Etape::Volume => "volume",
            Etape::Liste => "énumération de \\*",
            Etape::FermeRacine => "fermeture de \\",
            Etape::OuvreBonjour => "ouverture de \\bonjour.txt",
            Etape::Lit => "lecture de bonjour.txt",
            Etape::FermeBonjour => "fermeture de bonjour.txt",
            Etape::CreeEcrit => "création de ecrit.txt",
            Etape::Ecrit => "écriture de ecrit.txt",
            Etape::FermeEcrit => "fermeture de ecrit.txt",
            Etape::Termine => "scénario terminé",
        }
    }
}

/// Ce qui arrête le scénario, avec l'étape où c'est arrivé.
#[derive(Debug)]
struct Echec {
    etape: &'static str,
    detail: String,
}

impl Echec {
    fn new(etape: &'static str, detail: impl Display) -> Self {
        Self {
            etape,
            detail: detail.to_string(),
        }
    }
}

type Reponse = Result<Vec<SvcMessage>, Echec>;

/// L'automate du scénario, sans entrée-sortie.
#[derive(Debug)]
pub struct Scenario {
    etape: Etape,
    lignes: Vec<String>,
    prochain_completion: u32,
    /// `DeviceId` du lecteur servi, dès que le client l'a annoncé.
    lecteur: Option<u32>,
    /// `CompletionId` de l'IRP en vol ; toute complétion qui ne le porte pas
    /// est ignorée.
    attente: Option<u32>,
    /// `FileId` rendu par le client à la dernière ouverture.
    file_id: u32,
    /// Ce qu'on a lu de `bonjour.txt` jusqu'ici.
    lu: Vec<u8>,
}

impl Default for Scenario {
    fn default() -> Self {
        Self::new()
    }
}

impl Scenario {
    pub fn new() -> Self {
        Self {
            etape: Etape::Annonce,
            lignes: Vec::new(),
            prochain_completion: 1,
            lecteur: None,
            attente: None,
            file_id: 0,
            lu: Vec::new(),
        }
    }

    /// Le scénario a fini, en succès ou en échec.
    pub fn termine(&self) -> bool {
        self.etape == Etape::Termine
    }

    /// Les lignes produites depuis le dernier appel, dans l'ordre.
    pub fn lignes(&mut self) -> Vec<String> {
        core::mem::take(&mut self.lignes)
    }

    /// Le canal vient d'être joint : le serveur parle en premier (3.3.5.1.1).
    pub fn demarrer(&mut self) -> Vec<SvcMessage> {
        self.etape = Etape::Annonce;
        vec![SvcMessage::from(annonce_serveur())]
    }

    /// Un PDU complet (déchunké) du client ; rend ce qu'il faut lui renvoyer.
    pub fn recevoir(&mut self, octets: &[u8]) -> Vec<SvcMessage> {
        let mut src = ReadCursor::new(octets);
        let en_tete = match SharedHeader::decode(&mut src) {
            Ok(en_tete) => en_tete,
            Err(erreur) => {
                self.echec(Echec::new("en-tête", erreur));
                return Vec::new();
            }
        };
        let resultat = match en_tete.packet_id {
            PacketId::CoreClientidConfirm => self.sur_annonce(&mut src),
            PacketId::CoreClientName => self.sur_nom(&mut src),
            PacketId::CoreClientCapability => self.sur_capacites(&mut src),
            PacketId::CoreDevicelistAnnounce => self.sur_peripheriques(&mut src),
            // Une complétion après la fin (tardive, ou d'un IRP qu'on
            // n'attend plus) n'a plus de suite à donner.
            PacketId::CoreDeviceIoCompletion if !self.termine() => self.sur_completion(&mut src),
            autre => {
                debug!(?autre, "PDU rdpdr ignoré");
                Ok(Vec::new())
            }
        };
        match resultat {
            Ok(messages) => messages,
            Err(echec) => {
                self.echec(echec);
                Vec::new()
            }
        }
    }

    fn ligne(&mut self, texte: String) {
        self.lignes.push(texte);
    }

    fn echec(&mut self, echec: Echec) {
        let Echec { etape, detail } = echec;
        self.ligne(format!("rdpdr: échec {etape} : {detail}"));
        self.etape = Etape::Termine;
        self.attente = None;
    }

    fn sur_annonce(&mut self, src: &mut ReadCursor<'_>) -> Reponse {
        let annonce =
            AnnonceClient::decode(src).map_err(|e| Echec::new(Etape::Annonce.libelle(), e))?;
        debug!(?annonce, "rdpdr : réponse à l'annonce");
        if self.etape == Etape::Annonce {
            self.etape = Etape::Nom;
        }
        Ok(Vec::new())
    }

    fn sur_nom(&mut self, src: &mut ReadCursor<'_>) -> Reponse {
        let nom = NomClient::decode(src).map_err(|e| Echec::new(Etape::Nom.libelle(), e))?;
        debug!(nom = nom.0, "rdpdr : nom du client");
        if !matches!(self.etape, Etape::Annonce | Etape::Nom) {
            return Ok(Vec::new());
        }
        self.etape = Etape::Capacites;
        Ok(vec![SvcMessage::from(demande_capacites())])
    }

    fn sur_capacites(&mut self, src: &mut ReadCursor<'_>) -> Reponse {
        let capacites =
            CapacitesClient::decode(src).map_err(|e| Echec::new(Etape::Capacites.libelle(), e))?;
        debug!(?capacites, "rdpdr : capacités du client");
        if self.etape != Etape::Capacites {
            return Ok(Vec::new());
        }
        if !capacites.lecteur() {
            // Sans CAP_DRIVE_TYPE, le client n'annoncera pas de lecteur : la
            // poignée de main va au bout, le scénario ne démarre pas.
            debug!("rdpdr : le client n'annonce pas la redirection de lecteur");
        }
        self.etape = Etape::Peripheriques;
        let mut messages = vec![SvcMessage::from(confirmation_client_id())];
        // 3.3.5.1.7 : pas de UserLoggedOn à un client qui ne l'a pas demandé.
        // Sans lui, un client 1.12 garde ses lecteurs pour lui ; c'est alors
        // le client qui l'a voulu, le scénario reste inerte.
        if capacites.user_logged_on {
            messages.push(SvcMessage::from(user_logged_on()));
        } else {
            debug!("rdpdr : le client refuse UserLoggedOn, on ne l'envoie pas");
        }
        Ok(messages)
    }

    fn sur_peripheriques(&mut self, src: &mut ReadCursor<'_>) -> Reponse {
        let annonce = AnnoncePeripheriques::decode(src)
            .map_err(|e| Echec::new(Etape::Peripheriques.libelle(), e))?;
        let mut messages = Vec::new();
        for peripherique in &annonce.0 {
            messages.push(SvcMessage::from(reponse_annonce(
                peripherique.id,
                NtStatus::SUCCESS,
            )));
            if peripherique.est_lecteur() {
                self.ligne(format!(
                    "rdpdr: lecteur {} annoncé (id {})",
                    peripherique.nom, peripherique.id
                ));
            }
        }
        // Le premier lecteur annoncé après la poignée de main lance le
        // scénario ; un lecteur annoncé plus tard est accepté sans rien de plus.
        if self.etape == Etape::Peripheriques {
            if let Some(lecteur) = annonce.0.iter().find(|p| p.est_lecteur()) {
                self.lecteur = Some(lecteur.id);
                messages.extend(self.emettre(Etape::OuvreRacine, |lecteur, _, completion| {
                    irp_creation(lecteur, completion, "\\", Ouverture::DOSSIER)
                })?);
            }
        }
        Ok(messages)
    }

    /// Émet l'IRP de l'étape `suivante` et attend sa complétion. La fabrique
    /// reçoit le lecteur, le `FileId` courant et le `CompletionId` attribué.
    fn emettre<F>(&mut self, suivante: Etape, fabrique: F) -> Reponse
    where
        F: FnOnce(u32, u32, u32) -> EncodeResult<Irp>,
    {
        let Some(lecteur) = self.lecteur else {
            return Err(Echec::new(suivante.libelle(), "aucun lecteur annoncé"));
        };
        let completion = self.prochain_completion;
        self.prochain_completion = self.prochain_completion.wrapping_add(1);
        let irp = fabrique(lecteur, self.file_id, completion)
            .map_err(|e| Echec::new(suivante.libelle(), e))?;
        self.etape = suivante;
        self.attente = Some(completion);
        Ok(vec![SvcMessage::from(irp)])
    }

    /// Un statut non nul arrête le scénario, sauf sur une fermeture.
    ///
    /// Une fermeture en erreur n'arrête rien : le handle est parti de toute
    /// façon, et un vrai serveur continue. Vu sur le fil avec `FreeRDP` 3.31,
    /// qui répond à `IRP_MJ_CLOSE` avec la dernière erreur de son fil, ici
    /// `STATUS_NO_MORE_FILES` juste après l'énumération (octets reçus :
    /// 72444349 01000000 08000000 06000080, puis cinq de remplissage).
    fn verifier_statut(etape: Etape, statut: NtStatus) -> Result<(), Echec> {
        if statut == NtStatus::SUCCESS {
            return Ok(());
        }
        let fermeture = matches!(
            etape,
            Etape::FermeRacine | Etape::FermeBonjour | Etape::FermeEcrit
        );
        if !fermeture {
            return Err(Echec::new(etape.libelle(), format!("{statut:?}")));
        }
        debug!(
            ?statut,
            etape = etape.libelle(),
            "rdpdr : fermeture en erreur, tolérée"
        );
        Ok(())
    }

    /// Le `FileId` que le client vient de rendre à une ouverture.
    fn noter_file_id(
        &mut self,
        src: &mut ReadCursor<'_>,
        libelle: &'static str,
    ) -> Result<(), Echec> {
        self.file_id = ReponseCreation::decode(src)
            .map_err(|e| Echec::new(libelle, e))?
            .file_id;
        Ok(())
    }

    fn sur_completion(&mut self, src: &mut ReadCursor<'_>) -> Reponse {
        let etape = self.etape;
        let libelle = etape.libelle();
        let en_tete = DeviceIoResponse::decode(src).map_err(|e| Echec::new(libelle, e))?;
        if self.lecteur != Some(en_tete.device_id) || self.attente != Some(en_tete.completion_id) {
            debug!(?en_tete, "rdpdr : complétion inattendue, ignorée");
            return Ok(Vec::new());
        }
        self.attente = None;
        let statut = en_tete.io_status;
        // L'énumération finit sur STATUS_NO_MORE_FILES, seul statut non nul
        // attendu du scénario.
        if etape == Etape::Liste && statut == NtStatus::NO_MORE_FILES {
            return self.emettre(Etape::FermeRacine, irp_fermeture);
        }
        Self::verifier_statut(etape, statut)?;
        match etape {
            Etape::OuvreRacine => {
                self.noter_file_id(src, libelle)?;
                self.emettre(Etape::Volume, irp_volume)
            }
            Etape::Volume => {
                let volume = InfosVolume::decode(src).map_err(|e| Echec::new(libelle, e))?;
                self.ligne(format!("rdpdr: volume {}", volume.etiquette));
                self.emettre(Etape::Liste, |lecteur, file_id, completion| {
                    irp_repertoire(lecteur, file_id, completion, Some("\\*"))
                })
            }
            Etape::Liste => {
                let entree = EntreeRepertoire::decode(src).map_err(|e| Echec::new(libelle, e))?;
                let sorte = if entree.est_dossier() {
                    "dir"
                } else {
                    "fichier"
                };
                self.ligne(format!(
                    "rdpdr: entrée {} {} {sorte}",
                    entree.nom, entree.taille
                ));
                self.emettre(Etape::Liste, |lecteur, file_id, completion| {
                    irp_repertoire(lecteur, file_id, completion, None)
                })
            }
            Etape::FermeRacine => self.emettre(Etape::OuvreBonjour, |lecteur, _, completion| {
                irp_creation(lecteur, completion, "\\bonjour.txt", Ouverture::LECTURE)
            }),
            Etape::OuvreBonjour => {
                self.noter_file_id(src, libelle)?;
                self.lu.clear();
                self.emettre(Etape::Lit, |lecteur, file_id, completion| {
                    irp_lecture(lecteur, file_id, completion, MORCEAU, 0)
                })
            }
            Etape::Lit => self.sur_lecture(src),
            Etape::FermeBonjour => self.emettre(Etape::CreeEcrit, |lecteur, _, completion| {
                irp_creation(lecteur, completion, "\\ecrit.txt", Ouverture::ECRITURE)
            }),
            Etape::CreeEcrit => {
                self.noter_file_id(src, libelle)?;
                self.emettre(Etape::Ecrit, |lecteur, file_id, completion| {
                    irp_ecriture(lecteur, file_id, completion, 0, CONTENU_ECRIT)
                })
            }
            Etape::Ecrit => {
                let ecrit = ReponseEcriture::decode(src).map_err(|e| Echec::new(libelle, e))?;
                let attendu = u32::try_from(CONTENU_ECRIT.len()).unwrap_or(u32::MAX);
                if ecrit.longueur != attendu {
                    return Err(Echec::new(
                        libelle,
                        format!("{} octets écrits sur {attendu}", ecrit.longueur),
                    ));
                }
                self.ligne("rdpdr: écrit ecrit.txt".to_owned());
                self.emettre(Etape::FermeEcrit, irp_fermeture)
            }
            Etape::FermeEcrit => {
                self.ligne("rdpdr: scénario terminé".to_owned());
                self.etape = Etape::Termine;
                Ok(Vec::new())
            }
            Etape::Annonce
            | Etape::Nom
            | Etape::Capacites
            | Etape::Peripheriques
            | Etape::Termine => {
                // Aucun IRP en vol dans ces étapes : `attente` est vide, on
                // n'arrive pas ici.
                Ok(Vec::new())
            }
        }
    }

    /// Un morceau de `bonjour.txt` : on continue jusqu'à un morceau court.
    fn sur_lecture(&mut self, src: &mut ReadCursor<'_>) -> Reponse {
        let libelle = Etape::Lit.libelle();
        let ReponseLecture(donnees) =
            ReponseLecture::decode(src).map_err(|e| Echec::new(libelle, e))?;
        let court = u32::try_from(donnees.len()).is_ok_and(|n| n < MORCEAU);
        self.lu.extend_from_slice(&donnees);
        if !court {
            let position = u64::try_from(self.lu.len()).map_err(|e| Echec::new(libelle, e))?;
            return self.emettre(Etape::Lit, |lecteur, file_id, completion| {
                irp_lecture(lecteur, file_id, completion, MORCEAU, position)
            });
        }
        let empreinte = hex(&Sha256::digest(&self.lu));
        self.ligne(format!(
            "rdpdr: lu bonjour.txt {} octets sha256={empreinte}",
            self.lu.len()
        ));
        self.emettre(Etape::FermeBonjour, irp_fermeture)
    }
}

/// Le canal statique `rdpdr` du serveur de test : l'automate, plus la sortie
/// standard.
#[derive(Debug)]
pub struct CanalRdpdr {
    scenario: Scenario,
}

ironrdp::core::impl_as_any!(CanalRdpdr);

impl CanalRdpdr {
    pub fn new() -> Self {
        Self {
            scenario: Scenario::new(),
        }
    }

    fn publier(&mut self) {
        for ligne in self.scenario.lignes() {
            println!("{ligne}");
        }
    }
}

impl Default for CanalRdpdr {
    fn default() -> Self {
        Self::new()
    }
}

impl SvcProcessor for CanalRdpdr {
    fn channel_name(&self) -> ChannelName {
        Rdpdr::NAME
    }

    fn start(&mut self) -> PduResult<Vec<SvcMessage>> {
        let messages = self.scenario.demarrer();
        self.publier();
        Ok(messages)
    }

    fn process(&mut self, payload: &[u8]) -> PduResult<Vec<SvcMessage>> {
        // Les octets bruts, pour lire ce qu'un client a vraiment envoyé quand
        // une étape échoue : IRONRDP_LOG=test_rdp_server=trace.
        trace!(octets = hex(payload), "rdpdr : PDU du client");
        // Jamais d'erreur vers le serveur : un PDU illisible arrête le
        // scénario, en le disant, mais pas la session.
        let messages = self.scenario.recevoir(payload);
        self.publier();
        Ok(messages)
    }
}

impl SvcServerProcessor for CanalRdpdr {}

/// Un canal neuf par connexion, pour le point d'attache ajouté au serveur
/// porté (`vendor/README.md`).
#[derive(Debug, Clone, Copy)]
pub struct FabriqueRdpdr;

impl SvcServerFactory for FabriqueRdpdr {
    fn build_svc(&self) -> Box<dyn SvcServerProcessor> {
        Box::new(CanalRdpdr::new())
    }
}
