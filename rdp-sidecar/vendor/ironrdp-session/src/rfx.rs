use core::cmp::min;

use ironrdp_graphics::color_conversion::{self, YCbCrBuffer};
use ironrdp_graphics::image_processing::PixelFormat;
use ironrdp_graphics::rectangle_processing::Region;
use ironrdp_graphics::{dwt, quantization, rlgr, subband_reconstruction};
use ironrdp_pdu::codecs::rfx::{self, EntropyAlgorithm, Quant, RfxRectangle, Tile};
use ironrdp_pdu::geometry::{InclusiveRectangle, Rectangle as _};
use ironrdp_pdu::{decode_cursor, Decode as _, ReadCursor};
use tracing::{instrument, trace};

use crate::image::DecodedImage;
use crate::{custom_err, general_err, reason_err, SessionResult};

const TILE_SIZE: u16 = 64;

pub type FrameId = u32;

pub struct DecodingContext {
    context: rfx::ContextPdu,
    channels: rfx::ChannelsPdu,
    decoding_tiles: DecodingTileContext,
}

impl Default for DecodingContext {
    fn default() -> Self {
        Self {
            context: rfx::ContextPdu {
                flags: rfx::OperatingMode::empty(),
                entropy_algorithm: EntropyAlgorithm::Rlgr1,
            },
            channels: rfx::ChannelsPdu(Vec::new()),
            decoding_tiles: DecodingTileContext::new(),
        }
    }
}

impl DecodingContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn decode(
        &mut self,
        image: &mut DecodedImage,
        destination: &InclusiveRectangle,
        input: &mut ReadCursor<'_>,
    ) -> SessionResult<(FrameId, InclusiveRectangle)> {
        loop {
            let block = rfx::Block::decode(input).map_err(|e| custom_err!("decode block", e))?;

            match block {
                rfx::Block::Sync(_) => {
                    self.process_sync(input)?;
                }
                rfx::Block::CodecChannel(rfx::CodecChannel::FrameBegin(f)) => {
                    return self.process_frame(f, input, image, destination);
                }
                _ => {
                    return Err(reason_err!(
                        "rfx::DecodingContext",
                        "unexpected RFX block type: {:?}",
                        block.block_type()
                    ));
                }
            }
        }
    }

    fn process_sync(&mut self, input: &mut ReadCursor<'_>) -> SessionResult<()> {
        self.process_headers(input)
    }

    fn process_headers(&mut self, input: &mut ReadCursor<'_>) -> SessionResult<()> {
        let mut context = None;
        let mut channels = None;

        // headers can appear in any order: CodecVersions, Channels, Context
        for _ in 0..3 {
            match decode_cursor(input).map_err(|e| custom_err!("decode headers", e))? {
                rfx::Block::CodecChannel(rfx::CodecChannel::Context(c)) => context = Some(c),
                rfx::Block::Channels(c) => channels = Some(c),
                rfx::Block::CodecVersions(_) => (),
                _ => {
                    return Err(general_err!("unexpected RFX block type"));
                }
            }
        }

        let context = context.ok_or_else(|| general_err!("context header is missing"))?;
        let channels = channels.ok_or_else(|| general_err!("channels header is missing"))?;

        if channels.0.is_empty() {
            return Err(general_err!("no RFX channel announced"));
        }

        self.context = context;
        self.channels = channels;

        Ok(())
    }

    #[instrument(skip_all)]
    fn process_frame(
        &mut self,
        frame_begin: rfx::FrameBeginPdu,
        input: &mut ReadCursor<'_>,
        image: &mut DecodedImage,
        destination: &InclusiveRectangle,
    ) -> SessionResult<(FrameId, InclusiveRectangle)> {
        let channel = self
            .channels
            .0
            .first()
            .ok_or_else(|| {
                // Une trame RemoteFX sans qu'aucun en-tête (Sync, Context,
                // Channels) n'ait été reçu avant : le serveur a perdu ses
                // en-têtes, ce que faisait ironrdp-server quand son premier
                // essai d'encodage manquait de tampon (10 septembre 2026).
                general_err!("RemoteFX frame received before any Sync/Channels header (the server dropped its RemoteFX headers)")
            })?;
        let width = channel
            .width
            .try_into()
            .map_err(|_| general_err!("invalid width"))?;
        let height = channel
            .height
            .try_into()
            .map_err(|_| general_err!("invalid height"))?;
        let entropy_algorithm = self.context.entropy_algorithm;

        let region: rfx::Block<'_> =
            decode_cursor(input).map_err(|e| custom_err!("decode region", e))?;
        let mut region = match region {
            rfx::Block::CodecChannel(rfx::CodecChannel::Region(region)) => region,
            _ => return Err(general_err!("unexpected block type")),
        };
        let tile_set: rfx::Block<'_> =
            decode_cursor(input).map_err(|e| custom_err!("decode tile_set", e))?;
        let tile_set = match tile_set {
            rfx::Block::CodecChannel(rfx::CodecChannel::TileSet(t)) => t,
            _ => return Err(general_err!("unexpected block type")),
        };
        let frame_end: rfx::Block<'_> =
            decode_cursor(input).map_err(|e| custom_err!("decode frame_end", e))?;
        if !matches!(
            frame_end,
            rfx::Block::CodecChannel(rfx::CodecChannel::FrameEnd(_))
        ) {
            return Err(general_err!("unexpected block type"));
        }

        if region.rectangles.is_empty() {
            region.rectangles = vec![RfxRectangle {
                x: 0,
                y: 0,
                width,
                height,
            }];
        }
        let region = region;

        trace!(frame_index = frame_begin.index);
        trace!(destination_rectangle = ?destination);
        trace!(context = ?self.context);
        trace!(channels = ?self.channels);
        trace!(?region);

        let clipping_rectangles =
            clipping_rectangles(region.rectangles.as_slice(), destination, width, height);
        trace!("Clipping rectangles: {:?}", clipping_rectangles);

        let mut final_update_rectangle = clipping_rectangles.extents.clone();

        // Portage avash (26 novembre 2026) : les tuiles se décodent en
        // parallèle, par lots, puis se recopient dans l'image dans l'ordre.
        // Une tuile de 64×64 ne dépend d'aucune autre ; c'était pourtant tout
        // le décodage, sur un seul cœur, qui plafonnait un bureau RemoteFX
        // plein écran à 13 images par seconde (mesuré par la mire du banc
        // d'affichage, e2e/mesures/rdp.spec.js). La recopie s'arrête à la
        // première tuile en erreur, comme la boucle d'origine : l'image
        // garde les tuiles d'avant, et l'erreur rendue est la même.
        let rectangles: Vec<InclusiveRectangle> =
            tiles_to_rectangles(tile_set.tiles.as_slice(), destination).collect();
        let donnees = map_tiles_data(tile_set.tiles.as_slice(), tile_set.quants.as_slice())?;
        let fils = fils_de_decodage();
        for (lot_rectangles, lot_donnees) in rectangles
            .chunks(LOT_TUILES)
            .zip(donnees.chunks(LOT_TUILES))
        {
            self.decoding_tiles
                .lot
                .resize(lot_donnees.len() * TUILE_RGBA, 0);
            let resultats = decoder_lot(
                lot_donnees,
                entropy_algorithm,
                &mut self.decoding_tiles,
                fils,
            );
            for (i, (update_rectangle, resultat)) in
                lot_rectangles.iter().zip(resultats).enumerate()
            {
                resultat?;
                let current_update_rectangle = image.apply_tile(
                    &self.decoding_tiles.lot[i * TUILE_RGBA..(i + 1) * TUILE_RGBA],
                    PixelFormat::RgbA32,
                    &clipping_rectangles,
                    update_rectangle,
                )?;

                final_update_rectangle = final_update_rectangle.union(&current_update_rectangle);
            }
        }

        Ok((frame_begin.index, final_update_rectangle))
    }
}

/// Octets d'une tuile décodée (64×64, RGBA).
const TUILE_RGBA: usize = 64 * 64 * 4;

/// Tuiles décodées avant recopie. Borne la mémoire quoi qu'annonce le
/// serveur : un TileSet porte jusqu'à 65 535 tuiles, qu'un serveur hostile
/// ferait sinon tenir ensemble (1 Gio). 256 tuiles, 4 Mio ; une image 1080p
/// en compte 510, soit deux lots.
const LOT_TUILES: usize = 256;

/// Au-delà, plus de fils ne fait plus gagner : la recopie et l'envoi à
/// l'interface restent sur le fil de la session.
const FILS_MAX: usize = 8;

/// Sous ce nombre de tuiles, lancer des fils coûte plus qu'il ne rapporte.
const TUILES_PAR_FIL_MIN: usize = 8;

fn fils_de_decodage() -> usize {
    std::thread::available_parallelism()
        .map_or(1, core::num::NonZeroUsize::get)
        .clamp(1, FILS_MAX)
}

#[derive(Debug, Clone)]
struct DecodingTileContext {
    /// Les tuiles décodées du lot en cours, à la suite.
    lot: Vec<u8>,
    /// Les tampons de travail du fil de la session.
    travail: Travail,
}

/// Les tampons de travail d'un fil de décodage : un par composante YCbCr,
/// et celui de la transformée en ondelettes.
#[derive(Debug, Clone)]
struct Travail {
    ycbcr_buffer: Vec<Vec<i16>>,
    ycbcr_temp_buffer: Vec<i16>,
}

impl Travail {
    fn new() -> Self {
        let tile_size = usize::from(TILE_SIZE);
        Self {
            ycbcr_buffer: vec![vec![0; tile_size * tile_size]; 3],
            ycbcr_temp_buffer: vec![0; tile_size * tile_size],
        }
    }

    /// Décode des tuiles à la suite, chacune dans sa part de `sortie`.
    fn decoder(
        &mut self,
        tuiles: &[TileData<'_>],
        entropy_algorithm: EntropyAlgorithm,
        sortie: &mut [u8],
    ) -> Vec<SessionResult<()>> {
        tuiles
            .iter()
            .zip(sortie.chunks_exact_mut(TUILE_RGBA))
            .map(|(tuile, sortie)| {
                decode_tile(
                    tuile,
                    entropy_algorithm,
                    sortie,
                    self.ycbcr_buffer.as_mut(),
                    self.ycbcr_temp_buffer.as_mut(),
                )
            })
            .collect()
    }
}

impl DecodingTileContext {
    fn new() -> Self {
        Self {
            lot: Vec::new(),
            travail: Travail::new(),
        }
    }
}

/// Décode un lot de tuiles dans `ctx.lot`, réparties entre `fils` fils, et
/// rend le verdict de chaque tuile, dans l'ordre du lot.
fn decoder_lot(
    tuiles: &[TileData<'_>],
    entropy_algorithm: EntropyAlgorithm,
    ctx: &mut DecodingTileContext,
    fils: usize,
) -> Vec<SessionResult<()>> {
    let fils = fils.min(tuiles.len() / TUILES_PAR_FIL_MIN).max(1);
    if fils == 1 {
        return ctx.travail.decoder(tuiles, entropy_algorithm, &mut ctx.lot);
    }
    let par_fil = tuiles.len().div_ceil(fils);
    std::thread::scope(|portee| {
        let mut fils_lances = Vec::with_capacity(fils);
        for (part, sortie) in tuiles
            .chunks(par_fil)
            .zip(ctx.lot.chunks_mut(par_fil * TUILE_RGBA))
        {
            fils_lances.push(
                portee.spawn(move || Travail::new().decoder(part, entropy_algorithm, sortie)),
            );
        }
        // Une panique dans un fil remonte ici, sur le fil de la session, comme
        // si le décodage y avait eu lieu. Rien ne l'y attrape : ce chemin
        // (RemoteFX par le chemin rapide) n'est pas sous le `catch_unwind` du
        // canal graphique, et elle arrête le processus RDP de cette session.
        // D'où le refus, au décodage, de ce qu'un serveur hostile pouvait y
        // faire paniquer (audit du 27 novembre 2026, SE-4).
        fils_lances
            .into_iter()
            .flat_map(|f| {
                f.join()
                    .unwrap_or_else(|panique| std::panic::resume_unwind(panique))
            })
            .collect()
    })
}

fn decode_tile(
    tile: &TileData<'_>,
    entropy_algorithm: EntropyAlgorithm,
    output: &mut [u8],
    ycbcr_temp: &mut [Vec<i16>],
    temp: &mut [i16],
) -> SessionResult<()> {
    for ((quant, data), ycbcr_buffer) in tile
        .quants
        .iter()
        .zip(tile.data.iter())
        .zip(ycbcr_temp.iter_mut())
    {
        decode_component(
            quant,
            entropy_algorithm,
            data,
            ycbcr_buffer.as_mut_slice(),
            temp,
        )?;
    }

    let ycbcr_buffer = YCbCrBuffer {
        y: ycbcr_temp[0].as_slice(),
        cb: ycbcr_temp[1].as_slice(),
        cr: ycbcr_temp[2].as_slice(),
    };

    color_conversion::ycbcr_to_rgba(ycbcr_buffer, output)
        .map_err(|e| custom_err!("decode_tile", e))?;

    Ok(())
}

fn decode_component(
    quant: &Quant,
    entropy_algorithm: EntropyAlgorithm,
    data: &[u8],
    output: &mut [i16],
    temp: &mut [i16],
) -> SessionResult<()> {
    rlgr::decode(entropy_algorithm, data, output)
        .map_err(|e| custom_err!("decode_component", e))?;
    subband_reconstruction::decode(&mut output[4032..]);
    quantization::decode(output, quant);
    dwt::decode(output, temp);

    Ok(())
}

fn clipping_rectangles(
    rectangles: &[RfxRectangle],
    destination: &InclusiveRectangle,
    width: u16,
    height: u16,
) -> Region {
    let mut clipping_rectangles = Region::new();

    rectangles
        .iter()
        .map(|r| InclusiveRectangle {
            left: min(destination.left + r.x, width - 1),
            top: min(destination.top + r.y, height - 1),
            right: min(destination.left + r.x + r.width - 1, width - 1),
            bottom: min(destination.top + r.y + r.height - 1, height - 1),
        })
        .for_each(|r| clipping_rectangles.union_rectangle(r));

    clipping_rectangles
}

fn tiles_to_rectangles<'a>(
    tiles: &'a [Tile<'_>],
    destination: &'a InclusiveRectangle,
) -> impl Iterator<Item = InclusiveRectangle> + 'a {
    tiles.iter().map(|t| InclusiveRectangle {
        left: destination.left + t.x * TILE_SIZE,
        top: destination.top + t.y * TILE_SIZE,
        right: destination.left + t.x * TILE_SIZE + TILE_SIZE - 1,
        bottom: destination.top + t.y * TILE_SIZE + TILE_SIZE - 1,
    })
}

/// Les tables de quantification de chaque tuile, par leur index.
///
/// Audit du 27 novembre 2026 (SE-4) : l'indexation directe paniquait sur un
/// index hors de la liste (TileSet à `numQuant = 0`), et ce chemin n'est sous
/// aucun `catch_unwind`. Le décodage du TileSet refuse désormais un tel index
/// ; ce refus-ci couvre un `TileSet` construit autrement que par lui.
fn map_tiles_data<'a>(tiles: &[Tile<'a>], quants: &[Quant]) -> SessionResult<Vec<TileData<'a>>> {
    let table = |i: u8| {
        quants
            .get(usize::from(i))
            .cloned()
            .ok_or_else(|| general_err!("index de quantification RemoteFX hors de la liste"))
    };
    tiles
        .iter()
        .map(|t| {
            Ok(TileData {
                quants: [
                    table(t.y_quant_index)?,
                    table(t.cb_quant_index)?,
                    table(t.cr_quant_index)?,
                ],
                data: [t.y_data, t.cb_data, t.cr_data],
            })
        })
        .collect()
}

struct TileData<'a> {
    quants: [Quant; 3],
    data: [&'a [u8]; 3],
}

#[cfg(test)]
mod tests_decodage_parallele {
    //! Le décodage réparti entre fils rend, tuile par tuile, exactement ce
    //! que rend le décodage sur le seul fil de la session : mêmes pixels,
    //! mêmes erreurs, dans le même ordre (portage avash, 26 novembre 2026).
    #![expect(
        clippy::as_conversions,
        clippy::cast_possible_truncation,
        reason = "données de test"
    )]

    use super::*;

    fn hasard(etat: &mut u64) -> u64 {
        *etat ^= *etat >> 12;
        *etat ^= *etat << 25;
        *etat ^= *etat >> 27;
        etat.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn quant(etat: &mut u64) -> Quant {
        // Quantification modérée : les coefficients restent dans la plage
        // que la conversion YCbCr attend.
        let mut q = || 6 + (hasard(etat) % 3) as u8;
        Quant {
            ll3: q(),
            lh3: q(),
            hl3: q(),
            hh3: q(),
            lh2: q(),
            hl2: q(),
            hh2: q(),
            lh1: q(),
            hl1: q(),
            hh1: q(),
        }
    }

    #[test]
    fn le_decodage_parallele_rend_les_memes_tuiles_et_les_memes_erreurs() {
        let mut etat = 0x5eed_0f_a5a5_u64;
        // RLGR3 seulement : ce paquet, testé seul, prend ironrdp-graphics sur
        // crates.io, dont l'encodeur RLGR1 se désynchronise du décodeur (UP_GR
        // pour UQ_GR, corrigé dans notre copie) et fabriquerait des tuiles
        // fausses. Le partage entre fils ne dépend pas du mode d'entropie.
        for mode in [EntropyAlgorithm::Rlgr3] {
            // De vraies tuiles : une image 64×64 (un dégradé propre à chaque
            // tuile) passée par la chaîne d'encodage RemoteFX, l'inverse du
            // décodage. Une tuile sur sept a une composante vide : des
            // erreurs au milieu du lot, que la recopie doit rencontrer au même
            // endroit quel que soit le nombre de fils. Pas d'octets au
            // hasard : leurs coefficients hors plage font déborder la
            // conversion YCbCr de la caisse `yuv` en debug (voir
            // rdp-sidecar/vendor/README.md), ce que ce test ne vise pas.
            let quants: Vec<Quant> = (0..100).map(|_| quant(&mut etat)).collect();
            let octets: Vec<[Vec<u8>; 3]> = (0..100)
                .map(|t| {
                    let pixels: Vec<u8> = (0..64 * 64)
                        .flat_map(|i| {
                            let (x, y) = (i % 64, i / 64);
                            let bruit = (hasard(&mut etat) % 9) as usize;
                            [(x * 3 + t) as u8, (y * 2 + t * 5) as u8, ((x + y) / 2 + bruit) as u8, 255]
                        })
                        .collect();
                    let (mut y, mut cb, mut cr) = ([0i16; 4096], [0i16; 4096], [0i16; 4096]);
                    color_conversion::to_64x64_ycbcr_tile(&pixels, 64, 64, 256, PixelFormat::RgbA32, &mut y, &mut cb, &mut cr)
                        .expect("YCbCr");
                    let mut temp = [0i16; 4096];
                    [y, cb, cr].map(|mut plan| {
                        if t % 7 == 3 {
                            return Vec::new();
                        }
                        dwt::encode(&mut plan, &mut temp);
                        quantization::encode(&mut plan, &quants[t]);
                        subband_reconstruction::encode(&mut plan[4032..]);
                        let mut tampon = vec![0u8; 4096 * 64];
                        let n = rlgr::encode(mode, &plan, &mut tampon).expect("encodage");
                        tampon.truncate(n);
                        tampon
                    })
                })
                .collect();
            let tuiles: Vec<TileData<'_>> = octets
                .iter()
                .zip(&quants)
                .map(|(c, q)| TileData {
                    quants: [q.clone(), q.clone(), q.clone()],
                    data: [&c[0], &c[1], &c[2]],
                })
                .collect();
            let mut seul = DecodingTileContext::new();
            seul.lot = vec![0xa5; tuiles.len() * TUILE_RGBA];
            let attendu: Vec<Result<(), String>> = decoder_lot(&tuiles, mode, &mut seul, 1)
                .into_iter()
                .map(|r| r.map_err(|e| e.to_string()))
                .collect();
            assert!(attendu.iter().any(Result::is_ok) && attendu.iter().any(Result::is_err));
            for fils in [2, 3, 4, 8] {
                let mut plusieurs = DecodingTileContext::new();
                plusieurs.lot = vec![0xa5; tuiles.len() * TUILE_RGBA];
                let obtenu: Vec<Result<(), String>> =
                    decoder_lot(&tuiles, mode, &mut plusieurs, fils)
                        .into_iter()
                        .map(|r| r.map_err(|e| e.to_string()))
                        .collect();
                assert_eq!(obtenu, attendu, "verdicts ({mode:?}, {fils} fils)");
                assert!(
                    plusieurs.lot == seul.lot,
                    "pixels différents ({mode:?}, {fils} fils)"
                );
            }
        }
    }

    /// Audit du 27 novembre 2026, SE-4 : l'index d'une table de
    /// quantification hors de la liste était une panique, sur un chemin que
    /// rien n'isole. C'est une erreur.
    #[test]
    fn un_index_de_quantification_hors_liste_est_refuse() {
        let tuile = |i: u8| Tile {
            y_quant_index: 0,
            cb_quant_index: 0,
            cr_quant_index: i,
            x: 0,
            y: 0,
            y_data: &[],
            cb_data: &[],
            cr_data: &[],
        };
        let mut etat = 7;
        let q = quant(&mut etat);
        assert!(map_tiles_data(&[tuile(0)], &[]).is_err());
        assert!(map_tiles_data(&[tuile(1)], core::slice::from_ref(&q)).is_err());
        assert_eq!(map_tiles_data(&[tuile(0)], &[q]).unwrap().len(), 1);
    }

    #[test]
    fn un_petit_lot_reste_sur_le_fil_de_la_session() {
        // Sous TUILES_PAR_FIL_MIN tuiles par fil, pas de fil lancé : le
        // résultat est celui du fil unique, sans coût de lancement.
        let octets = [vec![1u8, 2, 3], vec![4, 5], vec![6]];
        let tuiles = [TileData {
            quants: [Quant::default(), Quant::default(), Quant::default()],
            data: [&octets[0], &octets[1], &octets[2]],
        }];
        let mut ctx = DecodingTileContext::new();
        ctx.lot = vec![0; TUILE_RGBA];
        assert_eq!(
            decoder_lot(&tuiles, EntropyAlgorithm::Rlgr1, &mut ctx, 8).len(),
            1
        );
    }
}
