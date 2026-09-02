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
use crate::surface::{Cache, Surface, Zone};

/// Trace du canal graphique : évaluée une seule fois, pas à chaque PDU. `var_os`
/// prend le verrou global de l'environnement et parcourt `environ` ; l'appeler
/// des centaines de fois par trame (un PDU par tuile) pour un drapeau qui ne
/// change jamais était un coût fixe inutile sur le chemin chaud.
static TRACE: std::sync::LazyLock<bool> =
    std::sync::LazyLock::new(|| std::env::var_os("AVASH_EGFX_TRACE").is_some());

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
/// ClearCodec : celui que Windows emploie pour l'essentiel de son dessin sur le
/// canal graphique. Il s'appuie sur des caches — glyphes et barres verticales —
/// qui vivent d'une image à l'autre : le décodeur doit durer toute la session,
/// sinon une image sur deux devient illisible.
const CODEC_CLEARCODEC: u16 = 0x0008;
/// Codec planaire de RDP 6, employé pour de petites zones.
const CODEC_PLANAIRE: u16 = 0x000A;
const CODEC_NON_COMPRESSE: u16 = 0x0000;
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

/// Convertit une image BGRA (ordre du protocole RDP) en RGBA opaque, en une
/// allocation dimensionnée d'avance. `flat_map(...).collect()` repartait d'un
/// tampon vide et doublait sa capacité pour une sortie pourtant connue à l'octet
/// près — plusieurs mégaoctets recopiés en trop sur une image plein écran.
/// Employé quand la source est empruntée (non compressé).
fn bgra_vers_rgba(bgra: &[u8]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(bgra.len());
    for p in bgra.as_chunks::<4>().0 {
        rgba.extend_from_slice(&[p[2], p[1], p[0], 0xFF]);
    }
    rgba
}

/// Même conversion, mais SUR PLACE, quand on possède déjà le tampon (sortie
/// ClearCodec) : on échange rouge et bleu et on force l'opacité, sans allouer un
/// second tampon plein écran par image.
fn bgra_vers_rgba_sur_place(mut bgra: Vec<u8>, taille: usize) -> Vec<u8> {
    bgra.truncate(taille);
    for p in bgra.as_chunks_mut::<4>().0 {
        p.swap(0, 2); // B <-> R
        p[3] = 0xFF; // opaque
    }
    bgra
}

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

/// Un PDU EGFX, en-tête retiré.
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
/// Versions de capacités graphiques (MS-RDPEGFX 2.2.3), de la plus récente à la
/// plus ancienne — l'ordre dans lequel les serveurs les examinent.
///
/// Les drapeaux disent ce que nous savons faire, et surtout ce que nous ne
/// savons pas : `AVC_DISABLED` sur toutes les versions 10 et suivantes, faute
/// de décodeur H.264. Sans lui, un serveur qui accepte l'une de ces versions
/// enverrait de la vidéo que nous ne saurions pas lire. La 8.1 n'a rien à
/// désactiver : on se contente de ne pas y annoncer `AVC420_ENABLED`.
///
/// La 10.1 est la seule dont le champ de drapeaux n'en est pas un — la
/// spécification y place un champ réservé, que les serveurs ignorent.
const CAPS_FLAG_SMALL_CACHE: u32 = 0x0000_0002;
const CAPS_FLAG_AVC_DISABLED: u32 = 0x0000_0020;
const CAPVERSION_8: u32 = 0x0008_0004;
const VERSIONS: &[(u32, u32)] = &[
    (0x000A_0701, CAPS_FLAG_AVC_DISABLED | CAPS_FLAG_SMALL_CACHE), // 10.7
    (0x000A_0600, CAPS_FLAG_AVC_DISABLED | CAPS_FLAG_SMALL_CACHE), // 10.6
    (0x000A_0502, CAPS_FLAG_AVC_DISABLED | CAPS_FLAG_SMALL_CACHE), // 10.5
    (0x000A_0400, CAPS_FLAG_AVC_DISABLED | CAPS_FLAG_SMALL_CACHE), // 10.4
    (0x000A_0301, CAPS_FLAG_AVC_DISABLED),                         // 10.3
    (0x000A_0200, CAPS_FLAG_AVC_DISABLED | CAPS_FLAG_SMALL_CACHE), // 10.2
    (0x000A_0100, 0),                                              // 10.1
    (0x000A_0002, CAPS_FLAG_AVC_DISABLED | CAPS_FLAG_SMALL_CACHE), // 10.0
    (0x0008_0105, CAPS_FLAG_SMALL_CACHE),                          // 8.1
    (CAPVERSION_8, CAPS_FLAG_SMALL_CACHE),                         // 8.0
];

/// Annonce de capacités du canal graphique (MS-RDPEGFX 2.2.2.1).
///
/// Rien n'est compressé ici : contrairement au sens serveur → client, le serveur
/// lit ce canal sans passer par ZGFX (`rdpgfx_server_receive_pdu` attaque
/// directement l'en-tête). Un segment ZGFX en tête ferait lire n'importe quoi.
pub fn caps_advertise() -> Vec<u8> {
    let mut corps = Vec::with_capacity(2 + VERSIONS.len() * 12);
    corps.extend_from_slice(&u16::try_from(VERSIONS.len()).unwrap_or(0).to_le_bytes());
    for (version, drapeaux) in VERSIONS {
        corps.extend_from_slice(&version.to_le_bytes());
        corps.extend_from_slice(&4u32.to_le_bytes()); // longueur des données
        corps.extend_from_slice(&drapeaux.to_le_bytes());
    }
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
    let ancien = std::fs::read_to_string(chemin).unwrap_or_default();
    // Atomique, comme le fichier d'empreintes : ce fichier vit au même endroit
    // et une coupure pendant `fs::write` l'aurait laissé vide — chaque serveur
    // à canal graphique aurait de nouveau coûté une reconnexion.
    let _ = crate::atomique::ecrire(chemin, format!("{ancien}{cle}\n").as_bytes());
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
    surfaces: std::collections::BTreeMap<u16, Surface>,
    cache: Cache,
    planaire: ironrdp::graphics::rdp6::BitmapStreamDecoder,
    clair: ironrdp::graphics::clearcodec::ClearCodecDecoder,
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
                self.surfaces.insert(id, Surface::nouvelle(l, h));
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
            CMD_WIRE_TO_SURFACE_1 if c.len() > 17 => self.wire_to_surface_1(c),
            CMD_WIRE_TO_SURFACE_2 if c.len() > 13 => self.decoder_surface(c),
            CMD_SOLIDFILL if c.len() >= 8 => self.solid_fill(c),
            CMD_SURFACE_TO_SURFACE if c.len() >= 14 => self.surface_vers_surface(c),
            CMD_SURFACE_TO_CACHE if c.len() >= 20 => self.surface_vers_cache(c),
            CMD_CACHE_TO_SURFACE if c.len() >= 6 => self.cache_vers_surface(c),
            CMD_DELETE_ENCODING_CONTEXT => self.decodeur.oublier_tuiles(),
            CMD_EVICT_CACHE_ENTRY if c.len() >= 2 => {
                self.cache.oublier(u16::from_le_bytes([c[0], c[1]]));
            }
            // Une fenêtre distante n'a pas de sens ici : avash affiche la
            // sortie, pas les fenêtres individuelles du bureau. On ignore
            // sciemment, plutôt que de peindre à une origine inventée.
            CMD_MAP_SURFACE_TO_WINDOW
            | CMD_MAP_SURFACE_TO_SCALED_WINDOW
            | CMD_MAP_SURFACE_TO_SCALED_OUTPUT => {}
            CMD_END_FRAME if c.len() >= 4 => {
                let trame = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                self.trames_decodees = self.trames_decodees.wrapping_add(1);
                return Some(frame_acknowledge(trame, self.trames_decodees));
            }
            _ => {}
        }
        None
    }

    /// Publie une zone modifiée d'une surface vers la boucle d'affichage.
    fn publier(&mut self, id: u16, zones: &[Zone]) {
        let Some(surface) = self.surfaces.get(&id) else {
            return;
        };
        let (ox, oy) = self.origines.get(&id).copied().unwrap_or((0, 0));
        let mut sortie = self.file.lock().unwrap();
        for z in zones {
            let Some((z, pixels)) = surface.extraire(*z) else {
                continue;
            };
            sortie.trames.push(Trame {
                x: ox.saturating_add(z.x),
                y: oy.saturating_add(z.y),
                largeur: z.largeur,
                hauteur: z.hauteur,
                pixels,
            });
        }
    }

    /// Image posée directement sur une surface (MS-RDPEGFX 2.2.2.1).
    ///
    /// C'est par là que Windows envoie l'essentiel : `codecId` vaut 0x0008, le
    /// codec planaire de RDP 6, qu'IronRDP sait décoder. Le non compressé est
    /// accepté aussi — un serveur y recourt pour de très petites zones.
    fn wire_to_surface_1(&mut self, c: &[u8]) {
        let id = u16::from_le_bytes([c[0], c[1]]);
        let codec = u16::from_le_bytes([c[2], c[3]]);
        let Some(zone) = Zone::depuis_bords(&c[5..13]) else {
            return;
        };
        let n = u32::from_le_bytes([c[13], c[14], c[15], c[16]]) as usize;
        let Some(donnees) = c.get(17..17 + n) else {
            return;
        };
        let Some(surface) = self.surfaces.get_mut(&id) else {
            return;
        };
        let (l, h) = (usize::from(zone.largeur), usize::from(zone.hauteur));
        let rgba = match codec {
            CODEC_CLEARCODEC => {
                let clair = &mut self.clair;
                let issue = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    clair.decode(donnees, zone.largeur, zone.hauteur)
                }));
                match issue {
                    Ok(Ok(v)) if v.len() >= l * h * 4 => bgra_vers_rgba_sur_place(v, l * h * 4),
                    Ok(Err(e)) => {
                        eprintln!("egfx : ClearCodec refusé : {e}");
                        return;
                    }
                    _ => {
                        eprintln!("egfx : ClearCodec illisible ({n} o)");
                        return;
                    }
                }
            }
            CODEC_PLANAIRE => {
                let mut rgb = Vec::new();
                // Le décodeur planaire indexe ses plans à partir de longueurs
                // portées par le flux : on l'isole, comme les autres.
                let planaire = &mut self.planaire;
                let issue = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    planaire.decode_bitmap_stream_to_rgb24(donnees, &mut rgb, l, h)
                }));
                match &issue {
                    Ok(Err(e)) => eprintln!("egfx : planaire refusé : {e}"),
                    Err(_) => eprintln!("egfx : planaire a paniqué"),
                    Ok(Ok(())) if rgb.len() < l * h * 3 => {
                        eprintln!("egfx : planaire court : {} octets pour {l}x{h}", rgb.len())
                    }
                    Ok(Ok(())) => {}
                }
                if !matches!(issue, Ok(Ok(()))) || rgb.len() < l * h * 3 {
                    return;
                }
                let mut v = Vec::with_capacity(l * h * 4);
                for p in rgb.as_chunks::<3>().0.iter().take(l * h) {
                    v.extend_from_slice(&[p[0], p[1], p[2], 0xFF]);
                }
                v
            }
            // Non compressé : les octets arrivent dans l'ordre du protocole,
            // B, G, R puis un octet de remplissage. Les recopier tels quels
            // donnerait une image aux rouges et bleus inversés.
            CODEC_NON_COMPRESSE if donnees.len() >= l * h * 4 => {
                bgra_vers_rgba(&donnees[..l * h * 4])
            }
            _ => {
                eprintln!("egfx : codec {codec:#06x} non pris en charge, image ignorée");
                return;
            }
        };
        let zone = surface.ecrire(zone, &rgba, l * 4);
        if let Some(z) = zone {
            self.publier(id, &[z]);
        }
    }

    /// Rectangles d'une seule couleur (MS-RDPEGFX 2.2.2.4).
    fn solid_fill(&mut self, c: &[u8]) {
        let id = u16::from_le_bytes([c[0], c[1]]);
        // RDPGFX_COLOR32 se lit B, G, R, puis un octet ignoré ; nos surfaces
        // sont en RGBA, et opaques.
        let couleur = [c[4], c[3], c[2], 0xFF];
        let n = usize::from(u16::from_le_bytes([c[6], c[7]]));
        let Some(surface) = self.surfaces.get_mut(&id) else {
            return;
        };
        let mut zones = Vec::new();
        for i in 0..n {
            let d = 8 + i * 8;
            let Some(bords) = c.get(d..d + 8) else { break };
            if let Some(z) = Zone::depuis_bords(bords).and_then(|z| surface.remplir(z, couleur)) {
                zones.push(z);
            }
        }
        self.publier(id, &zones);
    }

    /// Recopie d'une surface vers une autre (MS-RDPEGFX 2.2.2.5).
    fn surface_vers_surface(&mut self, c: &[u8]) {
        let src = u16::from_le_bytes([c[0], c[1]]);
        let dst = u16::from_le_bytes([c[2], c[3]]);
        let Some(zone) = Zone::depuis_bords(&c[4..12]) else {
            return;
        };
        let Some((zone, pixels)) = self.surfaces.get(&src).and_then(|s| s.extraire(zone)) else {
            return;
        };
        let n = usize::from(u16::from_le_bytes([c[12], c[13]]));
        let Some(surface) = self.surfaces.get_mut(&dst) else {
            return;
        };
        let mut zones = Vec::new();
        for i in 0..n {
            let d = 14 + i * 4;
            let Some(p) = c.get(d..d + 4) else { break };
            let cible = Zone {
                x: u16::from_le_bytes([p[0], p[1]]),
                y: u16::from_le_bytes([p[2], p[3]]),
                largeur: zone.largeur,
                hauteur: zone.hauteur,
            };
            if let Some(z) = surface.ecrire(cible, &pixels, usize::from(zone.largeur) * 4) {
                zones.push(z);
            }
        }
        self.publier(dst, &zones);
    }

    /// Dépôt d'un morceau de surface dans le cache (MS-RDPEGFX 2.2.2.6).
    fn surface_vers_cache(&mut self, c: &[u8]) {
        let id = u16::from_le_bytes([c[0], c[1]]);
        let emplacement = u16::from_le_bytes([c[10], c[11]]);
        let Some(zone) = Zone::depuis_bords(&c[12..20]) else {
            return;
        };
        if let Some((z, pixels)) = self.surfaces.get(&id).and_then(|s| s.extraire(zone)) {
            self.cache
                .deposer(emplacement, z.largeur, z.hauteur, pixels);
        }
    }

    /// Reprise d'un morceau depuis le cache (MS-RDPEGFX 2.2.2.7).
    fn cache_vers_surface(&mut self, c: &[u8]) {
        let emplacement = u16::from_le_bytes([c[0], c[1]]);
        let id = u16::from_le_bytes([c[2], c[3]]);
        let n = usize::from(u16::from_le_bytes([c[4], c[5]]));
        let Some((largeur, hauteur, pixels)) = self.cache.lire(emplacement).cloned() else {
            return;
        };
        let Some(surface) = self.surfaces.get_mut(&id) else {
            return;
        };
        let mut zones = Vec::new();
        for i in 0..n {
            let d = 6 + i * 4;
            let Some(p) = c.get(d..d + 4) else { break };
            let cible = Zone {
                x: u16::from_le_bytes([p[0], p[1]]),
                y: u16::from_le_bytes([p[2], p[3]]),
                largeur,
                hauteur,
            };
            if let Some(z) = surface.ecrire(cible, &pixels, usize::from(largeur) * 4) {
                zones.push(z);
            }
        }
        self.publier(id, &zones);
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
            if *TRACE {
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
        let jeux = usize::from(u16::from_le_bytes([m[8], m[9]]));
        assert_eq!(longueur, 10 + jeux * 12, "un jeu fait douze octets");
        // Aucune version 10 ou plus ne doit être annoncée sans avoir désactivé
        // H.264 : un serveur qui la retiendrait enverrait de la vidéo que nous
        // ne savons pas décoder, et l'écran resterait vide.
        for i in 0..jeux {
            let d = 10 + i * 12;
            let version = u32::from_le_bytes([m[d], m[d + 1], m[d + 2], m[d + 3]]);
            let drapeaux = u32::from_le_bytes([m[d + 8], m[d + 9], m[d + 10], m[d + 11]]);
            if version >= 0x000A_0002 && version != 0x000A_0100 {
                assert!(
                    drapeaux & 0x0000_0020 != 0,
                    "la version {version:#010x} annoncée sans AVC_DISABLED"
                );
            }
        }
        assert!(
            (0..jeux).any(|i| {
                let d = 10 + i * 12;
                u32::from_le_bytes([m[d], m[d + 1], m[d + 2], m[d + 3]]) == CAPVERSION_8
            }),
            "la version 8 reste annoncée : c'est celle des serveurs les plus anciens"
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

    /// Fabrique un PDU du canal graphique à partir de sa charge utile.
    fn pdu(id: u16, charge: &[u8]) -> super::Pdu {
        super::Pdu {
            id,
            charge: charge.to_vec(),
        }
    }

    #[test]
    fn le_cache_de_surfaces_depose_puis_repose_a_plusieurs_endroits() {
        // Le compteur de points de destination est un entier SEIZE bits. Lu sur
        // huit, il valait presque toujours zéro : le serveur déposait dans le
        // cache et redemandait six cents fois sans que rien ne soit peint, et
        // le bureau Windows s'affichait troué.
        let (mut e, _canal, file) = super::Egfx::nouveau();
        // Surface 0, 8x4.
        e.traiter(&pdu(0x0009, &[0, 0, 8, 0, 4, 0, 0x20]));
        // Un rectangle rouge en 0,0 → 2,2.
        e.traiter(&pdu(
            0x0004,
            &[
                0, 0, /* B G R X */ 0, 0, 255, 0, /* rects */ 1, 0, 0, 0, 0, 0, 2, 0, 2,
                0,
            ],
        ));
        // Ce carré part au cache, emplacement 7.
        let mut vers_cache = vec![0, 0]; // surfaceId
        vers_cache.extend_from_slice(&[0; 8]); // cacheKey
        vers_cache.extend_from_slice(&[7, 0]); // emplacement
        vers_cache.extend_from_slice(&[0, 0, 0, 0, 2, 0, 2, 0]); // rect
        e.traiter(&pdu(0x0006, &vers_cache));
        file.lock().unwrap().trames.clear();

        // …puis revient à DEUX endroits. Le compteur vaut 2 sur seize bits.
        let depuis_cache = [7, 0, 0, 0, 2, 0, 4, 0, 0, 0, 6, 0, 2, 0];
        e.traiter(&pdu(0x0007, &depuis_cache));
        let trames = std::mem::take(&mut file.lock().unwrap().trames);
        assert_eq!(trames.len(), 2, "deux points de destination, deux zones");
        assert_eq!((trames[0].x, trames[0].y), (4, 0));
        assert_eq!((trames[1].x, trames[1].y), (6, 2));
        assert_eq!(
            trames[0].pixels[..4],
            [255, 0, 0, 255],
            "le rouge a survécu"
        );
    }

    #[test]
    fn un_remplissage_uni_prend_la_couleur_dans_le_bon_ordre() {
        // RDPGFX_COLOR32 se lit B, G, R : recopier tel quel donnerait un bleu
        // là où le serveur demande un rouge.
        let (mut e, _canal, file) = super::Egfx::nouveau();
        e.traiter(&pdu(0x0009, &[0, 0, 4, 0, 4, 0, 0x20]));
        e.traiter(&pdu(
            0x0004,
            &[0, 0, 0x10, 0x20, 0x30, 0xFF, 1, 0, 0, 0, 0, 0, 4, 0, 4, 0],
        ));
        let trames = std::mem::take(&mut file.lock().unwrap().trames);
        assert_eq!(trames.len(), 1);
        assert_eq!(trames[0].pixels[..4], [0x30, 0x20, 0x10, 0xFF]);
    }

    #[test]
    fn reset_graphics_suit_la_nouvelle_taille_et_ecarte_l_absurde() {
        // Le magnétoscope rejoue des sessions à taille fixe : ce chemin — la
        // réponse du serveur à un redimensionnement — n'y est jamais exercé.
        let (mut e, _canal, file) = super::Egfx::nouveau();
        // 1920 x 1200 (u32 little-endian).
        e.traiter(&pdu(
            super::CMD_RESET_GRAPHICS,
            &[128, 7, 0, 0, 176, 4, 0, 0],
        ));
        assert_eq!(file.lock().unwrap().taille, Some((1920, 1200)));
        // Une dimension nulle est ignorée : l'ancienne taille demeure.
        e.traiter(&pdu(super::CMD_RESET_GRAPHICS, &[0, 0, 0, 0, 176, 4, 0, 0]));
        assert_eq!(file.lock().unwrap().taille, Some((1920, 1200)));
        // Au-delà de 65535, try_from échoue : pas de panique, taille inchangée.
        e.traiter(&pdu(super::CMD_RESET_GRAPHICS, &[0, 0, 1, 0, 0, 0, 1, 0]));
        assert_eq!(file.lock().unwrap().taille, Some((1920, 1200)));
    }

    #[test]
    fn end_frame_accuse_la_trame_et_incremente_le_compteur() {
        // L'accusé de fin de trame n'était couvert par rien : un format erroné
        // fait cesser le serveur d'envoyer des images (session figée après N
        // trames), sans erreur visible.
        let (mut e, _canal, _file) = super::Egfx::nouveau();
        let a = e
            .traiter(&pdu(super::CMD_END_FRAME, &[5, 0, 0, 0]))
            .expect("EndFrame doit produire un accusé");
        assert_eq!(a.len(), 20);
        assert_eq!(
            u16::from_le_bytes([a[0], a[1]]),
            super::CMD_FRAME_ACKNOWLEDGE
        );
        assert_eq!(&a[12..16], &[5, 0, 0, 0], "identifiant de trame renvoyé");
        assert_eq!(&a[16..20], &[1, 0, 0, 0], "première trame décodée");
        let b = e
            .traiter(&pdu(super::CMD_END_FRAME, &[9, 0, 0, 0]))
            .expect("EndFrame doit produire un accusé");
        assert_eq!(&b[12..16], &[9, 0, 0, 0]);
        assert_eq!(&b[16..20], &[2, 0, 0, 0], "compteur incrémenté");
    }

    #[test]
    fn surface_vers_surface_recopie_vers_plusieurs_points() {
        // SurfaceToSurface (défilement matériel, très employé par Windows) n'avait
        // aucun test, alors qu'il a la même forme de bug que le cache : n rectangles
        // lus par décalage, zone source extraite une fois.
        let (mut e, _canal, file) = super::Egfx::nouveau();
        // Deux surfaces 8x4.
        e.traiter(&pdu(0x0009, &[0, 0, 8, 0, 4, 0, 0x20])); // surface 0 (source)
        e.traiter(&pdu(0x0009, &[1, 0, 8, 0, 4, 0, 0x20])); // surface 1 (cible)
                                                            // Un carré rouge 2x2 en 0,0 sur la surface 0.
        e.traiter(&pdu(
            0x0004,
            &[
                0, 0, /* B G R X */ 0, 0, 255, 0, 1, 0, 0, 0, 0, 0, 2, 0, 2, 0,
            ],
        ));
        file.lock().unwrap().trames.clear();
        // src=0, dst=1, bords [0,0,2,2], 2 destinations : (4,0) et (6,2).
        let mut c = vec![0, 0, 1, 0]; // src, dst
        c.extend_from_slice(&[0, 0, 0, 0, 2, 0, 2, 0]); // bords
        c.extend_from_slice(&[2, 0]); // n = 2
        c.extend_from_slice(&[4, 0, 0, 0]); // (4,0)
        c.extend_from_slice(&[6, 0, 2, 0]); // (6,2)
        e.traiter(&pdu(super::CMD_SURFACE_TO_SURFACE, &c));
        let trames = std::mem::take(&mut file.lock().unwrap().trames);
        assert_eq!(trames.len(), 2, "deux destinations, deux zones peintes");
        assert_eq!((trames[0].x, trames[0].y), (4, 0));
        assert_eq!((trames[1].x, trames[1].y), (6, 2));
        assert_eq!(
            trames[0].pixels[..4],
            [255, 0, 0, 255],
            "le rouge a survécu"
        );
    }

    #[test]
    fn une_commande_pour_une_surface_inconnue_ne_fait_rien() {
        // Tout vient du réseau : un identifiant de surface inventé ne doit ni
        // paniquer, ni produire d'image.
        let (mut e, _canal, file) = super::Egfx::nouveau();
        e.traiter(&pdu(
            0x0004,
            &[9, 9, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 4, 0, 4, 0],
        ));
        e.traiter(&pdu(0x0007, &[3, 0, 9, 9, 1, 0, 0, 0, 0, 0]));
        e.traiter(&pdu(0x000A, &[9, 9]));
        assert!(file.lock().unwrap().trames.is_empty());
    }
}
