//! Canal graphique EGFX (MS-RDPEGFX).
//!
//! # Pourquoi ce module existe
//!
//! GNOME Remote Desktop — et les serveurs Windows récents quand on le leur
//! demande — n'envoient pas leurs images par les mises à jour bitmap
//! historiques, mais par un **canal dynamique** dédié. Sans lui, la connexion
//! aboutit et l'écran reste vide : c'est exactement ce qu'avash faisait.
//!
//! IronRDP fournit les *codecs* (`zgfx`, `progressive`, `clearcodec`) mais
//! aucune couche protocole : ni le canal, ni les PDU, ni la gestion des
//! surfaces. C'est ce que ce module apporte.
//!
//! # Ce que le canal transporte
//!
//! Tout le flux est comprimé par ZGFX. Une fois décomprimé, il contient une
//! suite de PDU précédés d'un en-tête commun (identifiant, drapeaux, longueur).

use ironrdp::core as ironrdp_core;
use ironrdp::dvc::{DvcMessage, DvcProcessor};
use ironrdp::pdu::PduResult;

use crate::progressif;

/// Nom du canal, imposé par la spécification.
pub const CHANNEL_NAME: &str = "Microsoft::Windows::RDS::Graphics";

// Identifiants de PDU (MS-RDPEGFX 2.2.1.5).
const CMD_WIRE_TO_SURFACE_1: u16 = 0x0001;
const CMD_WIRE_TO_SURFACE_2: u16 = 0x0002;
const CMD_DELETE_ENCODING_CONTEXT: u16 = 0x0003;
const CMD_SOLIDFILL: u16 = 0x0004;
const CMD_SURFACE_TO_SURFACE: u16 = 0x0005;
const CMD_SURFACE_TO_CACHE: u16 = 0x0006;
const CMD_CACHE_TO_SURFACE: u16 = 0x0007;
const CMD_EVICT_CACHE_ENTRY: u16 = 0x0008;
const CMD_CREATE_SURFACE: u16 = 0x0009;
const CMD_DELETE_SURFACE: u16 = 0x000A;
const CMD_START_FRAME: u16 = 0x000B;
const CMD_END_FRAME: u16 = 0x000C;
const CMD_FRAME_ACKNOWLEDGE: u16 = 0x000D;
const CMD_RESET_GRAPHICS: u16 = 0x000E;
const CMD_MAP_SURFACE_TO_OUTPUT: u16 = 0x000F;
const CMD_CACHE_IMPORT_OFFER: u16 = 0x0010;
const CMD_CACHE_IMPORT_REPLY: u16 = 0x0011;
/// RemoteFX Progressive : le seul codec que nous décodions, et celui que GNOME
/// Remote Desktop retient dès lors que le client n'annonce pas H.264.
const CODEC_CAPROGRESSIVE: u16 = 0x0009;
const CMD_CAPS_ADVERTISE: u16 = 0x0012;
const CMD_CAPS_CONFIRM: u16 = 0x0013;
const CMD_MAP_SURFACE_TO_WINDOW: u16 = 0x0014;
const CMD_QOE_FRAME_ACKNOWLEDGE: u16 = 0x0015;
const CMD_MAP_SURFACE_TO_SCALED_OUTPUT: u16 = 0x0016;
const CMD_MAP_SURFACE_TO_SCALED_WINDOW: u16 = 0x0017;

/// Nomme un identifiant de PDU, pour les traces.
#[must_use]
pub fn nom_pdu(id: u16) -> &'static str {
    match id {
        CMD_WIRE_TO_SURFACE_1 => "WireToSurface1",
        CMD_WIRE_TO_SURFACE_2 => "WireToSurface2",
        CMD_DELETE_ENCODING_CONTEXT => "DeleteEncodingContext",
        CMD_SOLIDFILL => "SolidFill",
        CMD_SURFACE_TO_SURFACE => "SurfaceToSurface",
        CMD_SURFACE_TO_CACHE => "SurfaceToCache",
        CMD_CACHE_TO_SURFACE => "CacheToSurface",
        CMD_EVICT_CACHE_ENTRY => "EvictCacheEntry",
        CMD_CREATE_SURFACE => "CreateSurface",
        CMD_DELETE_SURFACE => "DeleteSurface",
        CMD_START_FRAME => "StartFrame",
        CMD_END_FRAME => "EndFrame",
        CMD_FRAME_ACKNOWLEDGE => "FrameAcknowledge",
        CMD_RESET_GRAPHICS => "ResetGraphics",
        CMD_MAP_SURFACE_TO_OUTPUT => "MapSurfaceToOutput",
        CMD_CACHE_IMPORT_OFFER => "CacheImportOffer",
        CMD_CACHE_IMPORT_REPLY => "CacheImportReply",
        CMD_CAPS_ADVERTISE => "CapsAdvertise",
        CMD_CAPS_CONFIRM => "CapsConfirm",
        CMD_MAP_SURFACE_TO_WINDOW => "MapSurfaceToWindow",
        CMD_QOE_FRAME_ACKNOWLEDGE => "QoeFrameAcknowledge",
        CMD_MAP_SURFACE_TO_SCALED_OUTPUT => "MapSurfaceToScaledOutput",
        CMD_MAP_SURFACE_TO_SCALED_WINDOW => "MapSurfaceToScaledWindow",
        _ => "inconnu",
    }
}

/// Un PDU EGFX, en-tête retiré.
/// Accusé de réception d'une trame (MS-RDPEGFX 2.2.2.13).
///
/// Sans lui, le serveur considère que le client n'arrive plus à suivre et cesse
/// d'envoyer des images au bout de quelques trames.
fn frame_acknowledge(trame: u32, total: u32) -> Vec<u8> {
    let mut m = Vec::with_capacity(20);
    m.extend_from_slice(&CMD_FRAME_ACKNOWLEDGE.to_le_bytes());
    m.extend_from_slice(&0u16.to_le_bytes()); // drapeaux
    m.extend_from_slice(&20u32.to_le_bytes()); // longueur totale
    m.extend_from_slice(&0u32.to_le_bytes()); // profondeur de file : on suit
    m.extend_from_slice(&trame.to_le_bytes());
    m.extend_from_slice(&total.to_le_bytes());
    m
}

/// Détaille le contenu d'un flux RemoteFX Progressive, pour la mise au point.
fn inspecter_progressif(flux: &[u8]) {
    match ironrdp::pdu::codecs::rfx::progressive::decode_progressive_stream(flux) {
        Err(e) => eprintln!("  progressif : illisible ({e})"),
        Ok(blocs) => {
            use ironrdp::pdu::codecs::rfx::progressive::{ProgressiveBlock, ProgressiveTile};
            for b in &blocs {
                match b {
                    ProgressiveBlock::Sync(_) => eprintln!("  bloc Sync"),
                    ProgressiveBlock::Context(c) => eprintln!("  bloc Context {c:?}"),
                    ProgressiveBlock::FrameBegin(f) => eprintln!("  bloc FrameBegin {f:?}"),
                    ProgressiveBlock::FrameEnd(_) => eprintln!("  bloc FrameEnd"),
                    ProgressiveBlock::Region(r) => {
                        let genres = r
                            .tiles
                            .iter()
                            .map(|t| match t {
                                ProgressiveTile::Simple(_) => "simple",
                                ProgressiveTile::First(_) => "first",
                                ProgressiveTile::Upgrade(_) => "upgrade",
                            })
                            .collect::<std::collections::BTreeSet<_>>();
                        eprintln!(
                            "  bloc Region : {} rect, {} tuiles ({genres:?}), extrapolation {}",
                            r.rects.len(),
                            r.tiles.len(),
                            r.uses_reduce_extrapolate()
                        );
                    }
                }
            }
        }
    }
}

#[derive(Debug)]
pub struct Pdu {
    pub id: u16,
    pub charge: Vec<u8>,
}

/// Découpe un flux décomprimé en PDU.
///
/// Un segment en contient souvent plusieurs. S'arrêter au premier ferait perdre
/// silencieusement des images — un défaut qui ne se verrait qu'à l'œil.
#[must_use]
pub fn decouper(o: &[u8]) -> Vec<Pdu> {
    let mut v = Vec::new();
    let mut i = 0usize;
    while i + 8 <= o.len() {
        let id = u16::from_le_bytes([o[i], o[i + 1]]);
        let n = u32::from_le_bytes([o[i + 4], o[i + 5], o[i + 6], o[i + 7]]) as usize;
        // Une longueur mensongère ne doit ni déborder ni faire boucler sans fin.
        if n < 8 || i + n > o.len() {
            break;
        }
        v.push(Pdu {
            id,
            charge: o[i + 8..i + n].to_vec(),
        });
        i += n;
    }
    v
}

/// Versions de capacités annoncées (MS-RDPEGFX 2.2.3).
const CAPVERSION_8: u32 = 0x0008_0004;

/// Annonce de capacités du canal graphique (MS-RDPEGFX 2.2.2.1).
///
/// Un seul jeu, la version 8. GNOME Remote Desktop retient la plus récente
/// version qu'il connaît parmi celles annoncées ; s'arrêter à la 8 revient à
/// demander RemoteFX Progressive, le seul codec que nous décodions — les
/// versions 10 et suivantes ouvriraient la porte à H.264, que nous ne savons pas
/// lire, et il faudrait alors le refuser explicitement par un drapeau.
///
/// Rien n'est compressé ici : contrairement au sens serveur → client, le serveur
/// lit ce canal sans passer par ZGFX (`rdpgfx_server_receive_pdu` attaque
/// directement l'en-tête). Un segment ZGFX en tête ferait lire n'importe quoi.
pub fn caps_advertise() -> Vec<u8> {
    let corps = {
        let mut c = Vec::new();
        c.extend_from_slice(&1u16.to_le_bytes()); // un seul jeu
        c.extend_from_slice(&CAPVERSION_8.to_le_bytes());
        c.extend_from_slice(&4u32.to_le_bytes()); // longueur des données
        c.extend_from_slice(&0u32.to_le_bytes()); // aucun drapeau
        c
    };
    let mut m = Vec::with_capacity(corps.len() + 8);
    m.extend_from_slice(&CMD_CAPS_ADVERTISE.to_le_bytes());
    m.extend_from_slice(&0u16.to_le_bytes()); // drapeaux
    m.extend_from_slice(&u32::try_from(corps.len() + 8).unwrap_or(0).to_le_bytes());
    m.extend_from_slice(&corps);
    m
}

/// Ce serveur a-t-il besoin du canal graphique ?
///
/// La question ne peut pas se trancher à l'avance, et se tromper coûte cher
/// dans les deux sens. Un serveur Windows dessine par le chemin classique —
/// mais **le seul fait d'accepter le canal graphique le fait taire** : il tient
/// alors pour acquis que le client dessinera par là, et n'envoie plus rien
/// d'autre. Refuser le canal, à l'inverse, laisse GNOME Remote Desktop sans
/// aucun moyen d'afficher quoi que ce soit.
///
/// Il n'existe pas non plus de signe fiable pour les distinguer d'avance : la
/// redirection de session, tentante, se retrouve aussi devant une ferme
/// Windows. Alors on observe. On refuse le canal, on attend ; si rien n'est
/// dessiné, on se reconnecte en l'acceptant, et on l'écrit à côté des
/// empreintes de certificats. La lenteur ne se paie qu'une fois par serveur.
///
/// `AVASH_EGFX` tranche à la main si besoin : `toujours` ou `jamais`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Politique {
    /// Canal refusé : on verra bien si le serveur dessine autrement.
    Observer,
    /// Canal accepté : ce serveur a déjà montré qu'il n'a que celui-là.
    Accepter,
}

impl Politique {
    /// Ce que l'on sait de `cle` (« hôte:port »), mémoire et réglage compris.
    #[must_use]
    pub fn pour(cle: &str, memoire: Option<&std::path::Path>) -> Self {
        match std::env::var("AVASH_EGFX").as_deref() {
            Ok("toujours") => return Self::Accepter,
            Ok("jamais") => return Self::Observer,
            _ => {}
        }
        let Some(chemin) = memoire else {
            return Self::Observer;
        };
        match std::fs::read_to_string(chemin) {
            Ok(t) if t.lines().any(|l| l.trim() == cle) => Self::Accepter,
            _ => Self::Observer,
        }
    }
}

/// Retient que `cle` a besoin du canal graphique. Silencieux en cas d'échec :
/// ne pas pouvoir écrire coûte une reconnexion de plus la prochaine fois, ce
/// qui ne justifie pas de refuser la session en cours.
pub fn memoriser(cle: &str, chemin: &std::path::Path) {
    if Politique::pour(cle, Some(chemin)) == Politique::Accepter {
        return;
    }
    if let Some(parent) = chemin.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let ancien = std::fs::read_to_string(chemin).unwrap_or_default();
    let _ = std::fs::write(chemin, format!("{ancien}{cle}\n"));
}

/// Identifiant du canal graphique, partagé avec la boucle de session.
///
/// `None` tant que le serveur ne l'a pas créé ; l'annonce est émise puis la
/// case remise à `None`, pour n'annoncer qu'une fois.
pub type CanalPartage = std::sync::Arc<std::sync::Mutex<Option<u32>>>;

/// Un morceau d'image décodé, prêt à être peint aux coordonnées de l'écran.
#[derive(Debug)]
pub struct Trame {
    pub x: u16,
    pub y: u16,
    pub largeur: u16,
    pub hauteur: u16,
    pub pixels: Vec<u8>,
}

/// Ce que le canal graphique a produit et que la boucle de session doit reprendre.
///
/// Le décodage a lieu dans le processeur du canal, enfoui dans la pile IronRDP ;
/// la peinture et l'interface appartiennent à la boucle. Cette structure est le
/// seul lien entre les deux.
#[derive(Debug, Default)]
pub struct Sortie {
    pub trames: Vec<Trame>,
    /// Nouvelle taille du bureau, quand le serveur a réinitialisé la scène.
    pub taille: Option<(u16, u16)>,
}

pub type FilePartagee = std::sync::Arc<std::sync::Mutex<Sortie>>;

#[derive(Default)]
pub struct Egfx {
    canal: CanalPartage,
    file: FilePartagee,
    surfaces: std::collections::BTreeMap<u16, progressif::Surface>,
    /// Origine à l'écran de chaque surface (MapSurfaceToOutput).
    origines: std::collections::BTreeMap<u16, (u16, u16)>,
    decodeur: progressif::Decodeur,
    trames_decodees: u32,
    zgfx: ironrdp::graphics::zgfx::Decompressor,
    /// Compte des PDU reçus, par identifiant.
    pub vus: std::collections::BTreeMap<u16, usize>,
}

ironrdp::core::impl_as_any!(Egfx);

impl Egfx {
    /// Crée le processeur et rend la case partagée qui portera l'identifiant.
    pub fn nouveau() -> (Self, CanalPartage, FilePartagee) {
        let canal = CanalPartage::default();
        let file = FilePartagee::default();
        (
            Self {
                canal: canal.clone(),
                file: file.clone(),
                ..Default::default()
            },
            canal,
            file,
        )
    }

    /// Traite un PDU du serveur. Rend un accusé de trame le cas échéant.
    fn traiter(&mut self, p: &Pdu) -> Option<Vec<u8>> {
        let c = &p.charge;
        match p.id {
            CMD_CREATE_SURFACE if c.len() >= 7 => {
                let id = u16::from_le_bytes([c[0], c[1]]);
                let (l, h) = (
                    u16::from_le_bytes([c[2], c[3]]),
                    u16::from_le_bytes([c[4], c[5]]),
                );
                self.surfaces
                    .insert(id, progressif::Surface::nouvelle(l, h));
            }
            CMD_DELETE_SURFACE if c.len() >= 2 => {
                let id = u16::from_le_bytes([c[0], c[1]]);
                self.surfaces.remove(&id);
                self.origines.remove(&id);
            }
            CMD_MAP_SURFACE_TO_OUTPUT if c.len() >= 12 => {
                let id = u16::from_le_bytes([c[0], c[1]]);
                let x = u32::from_le_bytes([c[4], c[5], c[6], c[7]]);
                let y = u32::from_le_bytes([c[8], c[9], c[10], c[11]]);
                // Au-delà de 65535, l'origine ne désigne aucun écran réel : on
                // préfère ignorer la correspondance que peindre n'importe où.
                if let (Ok(x), Ok(y)) = (u16::try_from(x), u16::try_from(y)) {
                    self.origines.insert(id, (x, y));
                }
            }
            CMD_RESET_GRAPHICS if c.len() >= 8 => {
                // Le serveur redessine la scène à une nouvelle taille : c'est sa
                // réponse à un redimensionnement demandé par Display Control.
                // Sans la suivre, l'image reste à l'ancienne taille et le bureau
                // s'affiche tronqué dans un cadre qui ne lui correspond plus.
                let l = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                let h = u32::from_le_bytes([c[4], c[5], c[6], c[7]]);
                if let (Ok(l), Ok(h)) = (u16::try_from(l), u16::try_from(h)) {
                    if l > 0 && h > 0 {
                        self.file.lock().unwrap().taille = Some((l, h));
                    }
                }
            }
            CMD_WIRE_TO_SURFACE_2 if c.len() > 13 => self.decoder_surface(c),
            CMD_END_FRAME if c.len() >= 4 => {
                let trame = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                self.trames_decodees = self.trames_decodees.wrapping_add(1);
                return Some(frame_acknowledge(trame, self.trames_decodees));
            }
            _ => {}
        }
        None
    }

    fn decoder_surface(&mut self, c: &[u8]) {
        let id = u16::from_le_bytes([c[0], c[1]]);
        let codec = u16::from_le_bytes([c[2], c[3]]);
        if codec != CODEC_CAPROGRESSIVE {
            eprintln!("egfx : codec {codec} non pris en charge, image ignorée");
            return;
        }
        let Some(surface) = self.surfaces.get_mut(&id) else {
            eprintln!("egfx : image pour une surface inconnue ({id})");
            return;
        };
        let zones = match self.decodeur.decoder(&c[13..], surface) {
            Ok(z) => z,
            Err(e) => {
                eprintln!("egfx : image illisible ({e:#})");
                return;
            }
        };
        let (ox, oy) = self.origines.get(&id).copied().unwrap_or((0, 0));
        let mut sortie = self.file.lock().unwrap();
        for z in zones {
            let largeur = usize::from(z.largeur);
            let mut pixels = Vec::with_capacity(largeur * usize::from(z.hauteur) * 4);
            let stride = usize::from(surface.largeur) * 4;
            for ligne in 0..usize::from(z.hauteur) {
                let d = (usize::from(z.y) + ligne) * stride + usize::from(z.x) * 4;
                pixels.extend_from_slice(&surface.pixels[d..d + largeur * 4]);
            }
            sortie.trames.push(Trame {
                x: ox.saturating_add(z.x),
                y: oy.saturating_add(z.y),
                largeur: z.largeur,
                hauteur: z.hauteur,
                pixels,
            });
        }
    }
}

/// Emballe un PDU du canal graphique en messages prêts pour la couche statique.
pub fn lot_dvc(
    id: u32,
    pdu: Vec<u8>,
) -> ironrdp::core::EncodeResult<ironrdp::svc::SvcProcessorMessages<ironrdp::dvc::DrdynvcClient>> {
    Ok(ironrdp::svc::SvcProcessorMessages::new(
        ironrdp::dvc::encode_dvc_messages(
            id,
            vec![Box::new(Brut(pdu)) as ironrdp::dvc::DvcMessage],
            ironrdp::svc::ChannelFlags::empty(),
        )?,
    ))
}

/// L'identifiant du canal, sans le consommer.
#[must_use]
pub fn canal_ouvert(canal: &CanalPartage) -> Option<u32> {
    *canal.lock().unwrap()
}

/// Le PDU d'annonce à émettre, une fois le canal ouvert — et une seule.
pub fn annonce_a_emettre(canal: &CanalPartage) -> Option<(u32, Vec<u8>)> {
    let id = canal.lock().unwrap().take()?;
    Some((id, caps_advertise()))
}

impl DvcProcessor for Egfx {
    fn channel_name(&self) -> &str {
        CHANNEL_NAME
    }

    /// Ouverture du canal — et, délibérément, aucun message en retour.
    ///
    /// Ce que `start` renvoie part dans le *même* envoi que la réponse de
    /// création du canal. Or GNOME Remote Desktop, via FreeRDP, ne lit ce canal
    /// qu'une fois celui-ci déclaré prêt : les octets arrivés dans cette
    /// écriture-là restent en file, et comme nous n'envoyions plus rien ensuite,
    /// aucun événement ne venait les y chercher. Le serveur attendait dix
    /// secondes une annonce pourtant déjà émise, puis fermait la session sur un
    /// `BadCapabilities` trompeur. FreeRDP annonce une trentaine de
    /// millisecondes plus tard, dans une écriture séparée : c'est ce que fait
    /// `annonce_a_emettre`, appelé depuis la boucle de session.
    fn start(&mut self, channel_id: u32) -> PduResult<Vec<DvcMessage>> {
        eprintln!("egfx : canal ouvert (id {channel_id})");
        *self.canal.lock().unwrap() = Some(channel_id);
        Ok(Vec::new())
    }

    fn process(&mut self, _channel_id: u32, charge: &[u8]) -> PduResult<Vec<DvcMessage>> {
        // Même précaution que dans le décodeur d'images : la décompression ZGFX
        // lit des longueurs et des index de fenêtre fournis par le serveur, et
        // le fuzzing par mutation y a trouvé une panique. Un serveur, même
        // simplement défaillant, ne doit pas pouvoir arrêter le processus — donc
        // toutes les sessions ouvertes, pas seulement la sienne.
        let mut clair = Vec::new();
        let zgfx = &mut self.zgfx;
        let lu = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            zgfx.decompress(charge, &mut clair)
        }));
        if !matches!(lu, Ok(Ok(_))) {
            eprintln!("egfx : segment illisible ({} o)", charge.len());
            return Ok(Vec::new());
        }
        let mut reponses: Vec<DvcMessage> = Vec::new();
        for p in decouper(&clair) {
            *self.vus.entry(p.id).or_insert(0) += 1;
            if std::env::var_os("AVASH_EGFX_TRACE").is_some() {
                let tete: String = p
                    .charge
                    .iter()
                    .take(20)
                    .map(|o| format!("{o:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                eprintln!(
                    "egfx : {} [{:#06x}] ({} o) {tete}",
                    nom_pdu(p.id),
                    p.id,
                    p.charge.len()
                );
                if p.id == CMD_WIRE_TO_SURFACE_2 && p.charge.len() > 13 {
                    inspecter_progressif(&p.charge[13..]);
                }
            }
            if let Some(accuse) = self.traiter(&p) {
                reponses.push(Box::new(Brut(accuse)));
            }
        }
        Ok(reponses)
    }
}

/// Message déjà encodé, à émettre tel quel.
#[derive(Debug)]
struct Brut(Vec<u8>);

impl ironrdp_core::Encode for Brut {
    fn encode(&self, dst: &mut ironrdp_core::WriteCursor<'_>) -> ironrdp_core::EncodeResult<()> {
        ironrdp_core::ensure_size!(in: dst, size: self.0.len());
        dst.write_slice(&self.0);
        Ok(())
    }
    fn name(&self) -> &'static str {
        "egfx"
    }
    fn size(&self) -> usize {
        self.0.len()
    }
}

impl ironrdp::dvc::DvcEncode for Brut {}

impl std::fmt::Debug for Egfx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Egfx").field("vus", &self.vus).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::{annonce_a_emettre, caps_advertise, decouper, CAPVERSION_8, CMD_CAPS_ADVERTISE};

    #[test]
    fn plusieurs_pdu_dans_un_segment_sont_tous_rendus() {
        // S'arrêter au premier ferait perdre des images sans le moindre signe.
        let mut o = Vec::new();
        for (id, n) in [(0x000Bu16, 12usize), (0x0001, 20), (0x000C, 12)] {
            o.extend_from_slice(&id.to_le_bytes());
            o.extend_from_slice(&0u16.to_le_bytes());
            o.extend_from_slice(&u32::try_from(n).unwrap().to_le_bytes());
            o.extend(std::iter::repeat_n(0u8, n - 8));
        }
        let v = decouper(&o);
        assert_eq!(v.len(), 3);
        assert_eq!(v[1].charge.len(), 12);
    }

    #[test]
    fn une_longueur_mensongere_arrete_le_decoupage() {
        let mut o = 0x0001u16.to_le_bytes().to_vec();
        o.extend_from_slice(&0u16.to_le_bytes());
        o.extend_from_slice(&u32::MAX.to_le_bytes());
        o.extend_from_slice(&[0; 4]);
        assert!(decouper(&o).is_empty());
    }

    #[test]
    fn une_longueur_nulle_ne_fait_pas_boucler() {
        let mut o = 0x0001u16.to_le_bytes().to_vec();
        o.extend_from_slice(&0u16.to_le_bytes());
        o.extend_from_slice(&0u32.to_le_bytes());
        assert!(decouper(&o).is_empty());
    }

    #[test]
    fn l_annonce_de_capacites_est_bien_formee() {
        let m = caps_advertise();
        assert_eq!(u16::from_le_bytes([m[0], m[1]]), CMD_CAPS_ADVERTISE);
        let longueur = u32::from_le_bytes([m[4], m[5], m[6], m[7]]) as usize;
        assert_eq!(
            longueur,
            m.len(),
            "la longueur annoncée doit couvrir le PDU entier"
        );
        assert_eq!(
            u16::from_le_bytes([m[8], m[9]]),
            1,
            "un seul jeu de capacités"
        );
        assert_eq!(
            u32::from_le_bytes([m[10], m[11], m[12], m[13]]),
            CAPVERSION_8,
            "la version 8 est la seule dont GNOME Remote Desktop ne relit pas les drapeaux"
        );
    }

    #[test]
    fn l_annonce_n_est_emise_qu_une_fois() {
        // Réannoncer ses capacités en cours de session est une violation du
        // protocole que GNOME Remote Desktop sanctionne par une fermeture.
        let canal = super::CanalPartage::default();
        assert!(
            annonce_a_emettre(&canal).is_none(),
            "aucun canal, rien à dire"
        );
        *canal.lock().unwrap() = Some(7);
        assert_eq!(annonce_a_emettre(&canal).map(|(id, _)| id), Some(7));
        assert!(annonce_a_emettre(&canal).is_none(), "la case est vidée");
    }

    #[test]
    fn les_identifiants_suivent_la_specification() {
        // Ancrés sur MS-RDPEGFX 2.2.2 et vérifiés contre une capture de FreeRDP :
        // l'annonce y porte l'identifiant 0x0012. Avoir écrit 0x0011 — celui de
        // CacheImportReply — produisait un PDU parfaitement formé que GNOME
        // Remote Desktop recevait, ignorait, puis sanctionnait dix secondes plus
        // tard par un « BadCapabilities » qui désignait la mauvaise cause.
        assert_eq!(super::CMD_CAPS_ADVERTISE, 0x0012);
        assert_eq!(super::CMD_CAPS_CONFIRM, 0x0013);
        assert_eq!(super::nom_pdu(0x0012), "CapsAdvertise");
        assert_eq!(super::nom_pdu(0x000B), "StartFrame");
        assert_eq!(super::nom_pdu(0x000E), "ResetGraphics");
        assert_eq!(caps_advertise()[0], 0x12, "premier octet du PDU émis");
    }

    #[test]
    fn le_canal_graphique_ne_s_accorde_qu_apres_l_avoir_appris() {
        use super::{memoriser, Politique};
        let dossier = std::env::temp_dir().join(format!("avash-egfx-{}", std::process::id()));
        let memoire = dossier.join("rdp_canal_graphique");
        let _ = std::fs::remove_dir_all(&dossier);

        // Par défaut on refuse : accepter le canal suffit à faire taire un
        // serveur Windows, qui dessinerait pourtant très bien sans lui.
        assert_eq!(
            Politique::pour("hote:3389", Some(&memoire)),
            Politique::Observer
        );
        assert_eq!(Politique::pour("hote:3389", None), Politique::Observer);

        memoriser("hote:3389", &memoire);
        assert_eq!(
            Politique::pour("hote:3389", Some(&memoire)),
            Politique::Accepter
        );

        // Un autre serveur n'hérite de rien, pas même d'un préfixe commun.
        assert_eq!(
            Politique::pour("hote:33890", Some(&memoire)),
            Politique::Observer
        );
        assert_eq!(
            Politique::pour("autre:3389", Some(&memoire)),
            Politique::Observer
        );

        // Retenir deux fois n'écrit qu'une ligne : le fichier ne gonfle pas à
        // chaque connexion.
        memoriser("hote:3389", &memoire);
        memoriser("autre:3389", &memoire);
        let lignes = std::fs::read_to_string(&memoire).unwrap();
        assert_eq!(lignes.lines().filter(|l| *l == "hote:3389").count(), 1);
        assert_eq!(
            Politique::pour("autre:3389", Some(&memoire)),
            Politique::Accepter
        );

        let _ = std::fs::remove_dir_all(&dossier);
    }
}
