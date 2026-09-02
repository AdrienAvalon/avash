//! Presse-papiers (CLIPRDR) : le dos d'IronRDP, relié à la boucle de session par un canal.

use ironrdp::cliprdr::backend::CliprdrBackend;
use ironrdp::cliprdr::pdu::{
    ClipboardFormat, ClipboardFormatId, ClipboardGeneralCapabilityFlags, FileContentsRequest,
    FileContentsResponse, FormatDataRequest, FormatDataResponse, LockDataId,
    OwnedFormatDataResponse,
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
}

/// Pont entre le canal CLIPRDR et le presse-papiers du poste (via le front).
/// Texte seulement (CF_UNICODETEXT).
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

impl CliprdrBackend for ClipBackend {
    #[allow(clippy::unnecessary_literal_bound)]
    fn temporary_directory(&self) -> &str {
        "."
    }
    fn client_capabilities(&self) -> ClipboardGeneralCapabilityFlags {
        ClipboardGeneralCapabilityFlags::empty()
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
        if formats
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
    fn on_file_contents_request(&mut self, _req: FileContentsRequest) {}
    fn on_file_contents_response(&mut self, _resp: FileContentsResponse<'_>) {}
    fn on_lock(&mut self, _id: LockDataId) {}
    fn on_unlock(&mut self, _id: LockDataId) {}
}
