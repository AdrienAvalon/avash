//! Presse-papiers (CLIPRDR) : le dos d'IronRDP, relié à la boucle de session par un canal.

use ironrdp::cliprdr::backend::CliprdrBackend;
use ironrdp::cliprdr::pdu::{
    ClipboardFormat, ClipboardFormatId, ClipboardFormatName, ClipboardGeneralCapabilityFlags,
    FileContentsRequest, FileContentsResponse, FileDescriptor, FormatDataRequest,
    FormatDataResponse, LockDataId, OwnedFormatDataResponse,
};
use ironrdp::core::IntoOwned;

/// Presse-papiers local partagé (texte), alimenté par le front, servi au serveur.
pub(crate) type LocalClip = std::sync::Arc<std::sync::Mutex<Option<String>>>;

/// Requêtes du backend CLIPRDR vers la boucle principale. Le backend est
/// encapsulé dans l'ActiveStage et ne peut pas la rappeler : il passe par ce
/// canal, la boucle exécute l'action SVC correspondante.
#[derive(Debug)]
pub(crate) enum ClipReq {
    /// Annoncer au serveur qu'on a du texte (initiate_copy).
    Advertise,
    /// Servir des données réclamées par le serveur (submit_format_data).
    ServeData(OwnedFormatDataResponse),
    /// Réclamer au serveur les données d'un format (initiate_paste).
    RequestPaste(ClipboardFormatId),
    /// Texte reçu du serveur → à pousser vers le presse-papiers du poste.
    RemoteText(String),
    /// Le distant a copié des fichiers : leur liste (chemins déjà assainis par
    /// IronRDP) et le verrou posé sur son presse-papiers, à porter dans les
    /// requêtes de contenu.
    FichiersDistants(Vec<FileDescriptor>, Option<u32>),
    /// Un morceau de fichier demandé au distant est arrivé (`None` : refus).
    ContenuRecu(u32, Option<Vec<u8>>),
    /// Le distant réclame un morceau d'un fichier que le poste lui a offert.
    ServirContenu(FileContentsRequest),
    /// Le distant verrouille (ou libère) la liste offerte : la boucle garde une
    /// copie de l'offre sous cet identifiant, servie même si l'offre change.
    Verrou(LockDataId),
    Deverrou(LockDataId),
}

/// Pont entre le canal CLIPRDR et le presse-papiers du poste (via le front) :
/// texte (CF_UNICODETEXT) et fichiers (FileGroupDescriptorW par flux).
#[derive(Debug)]
pub(crate) struct ClipBackend {
    pub(crate) local_text: LocalClip,
    pub(crate) tx: tokio::sync::mpsc::UnboundedSender<ClipReq>,
    /// Le partage de presse-papiers est-il autorisé ? Piloté par l'interface
    /// (message `[12]`), dans les **deux** sens : le réglage ne gardait que le
    /// sens sortant, alors qu'un bureau hostile pouvait remplacer en boucle le
    /// presse-papiers du poste — on copie une commande depuis sa documentation,
    /// on colle dans son terminal local, on exécute celle de l'attaquant.
    pub(crate) partage: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

ironrdp::core::impl_as_any!(ClipBackend);

impl ClipBackend {
    pub(crate) fn partage_actif(&self) -> bool {
        self.partage.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// Le format « liste de fichiers » parmi ceux qu'annonce le distant, s'il y est.
pub(crate) fn format_liste_de_fichiers(formats: &[ClipboardFormat]) -> Option<ClipboardFormatId> {
    formats
        .iter()
        .find(|f| {
            f.name
                .as_ref()
                .is_some_and(|n| n.value() == ClipboardFormatName::FILE_LIST.value())
        })
        .map(|f| f.id)
}

impl CliprdrBackend for ClipBackend {
    #[allow(clippy::unnecessary_literal_bound)]
    fn temporary_directory(&self) -> &str {
        "."
    }
    /// Les fichiers passent en flux (`STREAM_FILECLIP_ENABLED`), le distant
    /// peut verrouiller notre liste (`CAN_LOCK_CLIPDATA`), les fichiers de
    /// plus de 2 Gio ont droit à leur position (`HUGE_FILE_SUPPORT_ENABLED`),
    /// et jamais de chemin absolu dans une liste (`FILECLIP_NO_FILE_PATHS`).
    fn client_capabilities(&self) -> ClipboardGeneralCapabilityFlags {
        ClipboardGeneralCapabilityFlags::STREAM_FILECLIP_ENABLED
            | ClipboardGeneralCapabilityFlags::CAN_LOCK_CLIPDATA
            | ClipboardGeneralCapabilityFlags::HUGE_FILE_SUPPORT_ENABLED
            | ClipboardGeneralCapabilityFlags::FILECLIP_NO_FILE_PATHS
    }
    fn on_ready(&mut self) {
        if self.partage_actif() && self.local_text.lock().is_ok_and(|t| t.is_some()) {
            let _ = self.tx.send(ClipReq::Advertise);
        }
    }
    fn on_request_format_list(&mut self) {
        if self.partage_actif() {
            let _ = self.tx.send(ClipReq::Advertise);
        }
    }
    fn on_process_negotiated_capabilities(&mut self, _caps: ClipboardGeneralCapabilityFlags) {}
    fn on_remote_copy(&mut self, formats: &[ClipboardFormat]) {
        if !self.partage_actif() {
            return; // on ne réclame même pas les données au serveur
        }
        // Des fichiers : on demande leur liste (noms et tailles, quelques
        // kilooctets), jamais leur contenu ; c'est l'utilisateur qui décide de
        // recevoir, depuis l'interface. Du texte : on le demande tel quel.
        if let Some(id) = format_liste_de_fichiers(formats) {
            let _ = self.tx.send(ClipReq::RequestPaste(id));
        } else if formats
            .iter()
            .any(|f| f.id == ClipboardFormatId::CF_UNICODETEXT)
        {
            let _ = self
                .tx
                .send(ClipReq::RequestPaste(ClipboardFormatId::CF_UNICODETEXT));
        }
    }
    fn on_format_data_request(&mut self, req: FormatDataRequest) {
        let resp = if self.partage_actif() && req.format == ClipboardFormatId::CF_UNICODETEXT {
            match self.local_text.lock().ok().and_then(|t| t.clone()) {
                Some(text) => FormatDataResponse::new_unicode_string(&text).into_owned(),
                None => FormatDataResponse::new_error().into_owned(),
            }
        } else {
            FormatDataResponse::new_error().into_owned()
        };
        let _ = self.tx.send(ClipReq::ServeData(resp));
    }
    fn on_format_data_response(&mut self, resp: FormatDataResponse<'_>) {
        if !resp.is_error() {
            if let Ok(text) = resp.to_unicode_string() {
                // Plafond anti-abus : un serveur ne sature pas la mémoire via un
                // presse-papiers géant (le texte normal reste très en dessous).
                if text.len() <= 8 * 1024 * 1024 {
                    let _ = self.tx.send(ClipReq::RemoteText(text));
                }
            }
        }
    }
    fn on_remote_file_list(&mut self, files: &[FileDescriptor], clip_data_id: Option<u32>) {
        if self.partage_actif() {
            let _ = self
                .tx
                .send(ClipReq::FichiersDistants(files.to_vec(), clip_data_id));
        }
    }
    fn on_file_contents_request(&mut self, req: FileContentsRequest) {
        if self.partage_actif() {
            let _ = self.tx.send(ClipReq::ServirContenu(req));
        }
    }
    fn on_file_contents_response(&mut self, resp: FileContentsResponse<'_>) {
        let donnees = (!resp.is_error()).then(|| resp.data().to_vec());
        let _ = self
            .tx
            .send(ClipReq::ContenuRecu(resp.stream_id(), donnees));
    }
    fn on_lock(&mut self, id: LockDataId) {
        let _ = self.tx.send(ClipReq::Verrou(id));
    }
    fn on_unlock(&mut self, id: LockDataId) {
        let _ = self.tx.send(ClipReq::Deverrou(id));
    }
}

#[cfg(test)]
mod tests_formats {
    use super::format_liste_de_fichiers;
    use ironrdp::cliprdr::pdu::{ClipboardFormat, ClipboardFormatId, ClipboardFormatName};

    /// Le distant choisit l'identifiant de son format de liste : seul le nom
    /// « FileGroupDescriptorW » le désigne.
    #[test]
    fn la_liste_de_fichiers_se_reconnait_a_son_nom() {
        let formats = [
            ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT),
            ClipboardFormat::new(ClipboardFormatId::new(0xC0A1))
                .with_name(ClipboardFormatName::FILE_LIST),
        ];
        assert_eq!(
            format_liste_de_fichiers(&formats),
            Some(ClipboardFormatId::new(0xC0A1))
        );
        assert_eq!(format_liste_de_fichiers(&formats[..1]), None);
    }
}
