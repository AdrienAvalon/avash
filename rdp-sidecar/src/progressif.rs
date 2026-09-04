//! Décodage RemoteFX Progressive (MS-RDPEGFX 2.2.4.2).
//!
//! C'est le codec que GNOME Remote Desktop emploie dès lors que le client
//! n'annonce pas H.264 — donc le nôtre. IronRDP fournit les briques : l'analyse
//! du flux en blocs, le premier passage d'une composante (RLGR1, delta LL3,
//! déquantification) et la conversion YCbCr. Manquait l'assemblage : parcourir
//! les tuiles, faire la transformée en ondelettes inverse, et reporter le tout
//! dans une surface.
//!
//! Les trois formes de tuile sont traitées. GNOME Remote Desktop n'envoie que
//! des tuiles `simple` ; Windows, lui, affine ses images par paliers de qualité
//! — une tuile `first` grossière, puis des `upgrade` qui ajoutent des bits de
//! précision aux coefficients déjà reçus. Sans elles, le bureau s'affiche avec
//! des trous exactement là où le serveur comptait revenir.

use anyhow::{Context, Result};

use crate::surface::{Surface, Zone};
use ironrdp::graphics::color_conversion::{ycbcr_to_rgba, YCbCrBuffer};
use ironrdp::graphics::{dwt, dwt_extrapolate, progressive};
use ironrdp::pdu::codecs::rfx::progressive::{
    decode_progressive_stream, ComponentCodecQuant, ProgressiveBlock, ProgressiveRegion,
    ProgressiveTile,
};

/// Côté d'une tuile, en pixels. Fixé par le codec.
const COTE: usize = 64;
const COEFFS: usize = progressive::COEFFICIENTS_PER_COMPONENT;
/// Sous-bandes d'une composante, dans l'ordre du flux : HL1, LH1, HH1, HL2,
/// LH2, HH2, HL3, LH3, HH3, puis LL3.
const NB_BANDES: usize = progressive::NUM_BANDS;
const LL3: usize = NB_BANDES - 1;
/// Bit 0 des drapeaux d'une tuile simple ou `first` : ses coefficients sont
/// un écart à ajouter à ceux déjà gardés (RFX_TILE_DIFFERENCE).
const DRAPEAU_DIFFERENCE: u8 = 0x01;

/// L'état d'une tuile entre deux paliers de qualité.
///
/// Les coefficients sont conservés dans le domaine des fréquences : la
/// transformée en ondelettes inverse les détruirait, or le palier suivant
/// travaille dessus. On la fait donc sur une copie.
pub(crate) struct EtatTuile {
    coefficients: [Vec<i16>; 3],
    signes: [Vec<i8>; 3],
    /// Position de bit atteinte, par composante puis par bande : quantificateur
    /// de base plus quantificateur progressif du dernier palier (le `yBitPos`
    /// de FreeRDP). Le palier suivant apporte exactement la différence.
    positions: [[u8; NB_BANDES]; 3],
}

impl Default for EtatTuile {
    fn default() -> Self {
        Self {
            coefficients: [vec![0; COEFFS], vec![0; COEFFS], vec![0; COEFFS]],
            signes: [vec![0; COEFFS], vec![0; COEFFS], vec![0; COEFFS]],
            positions: [[0; NB_BANDES]; 3],
        }
    }
}

/// Position de bit de chaque bande : base et progressif s'additionnent.
fn positions(base: &ComponentCodecQuant, prog: &ComponentCodecQuant) -> [u8; NB_BANDES] {
    std::array::from_fn(|b| base.for_band(b).saturating_add(prog.for_band(b)))
}

/// Début et nombre de coefficients de chaque bande dans le tampon d'une
/// composante.
fn bandes(extrapoler: bool) -> [(usize, usize); NB_BANDES] {
    if extrapoler {
        dwt_extrapolate::band_layout().map(|b| (b.offset, b.count()))
    } else {
        // Sans extrapolation, les bandes sont carrées : 32², 16² puis 8².
        let mut debut = 0;
        std::array::from_fn(|b| {
            let nombre = match b {
                0..=2 => 1024,
                3..=5 => 256,
                _ => 64,
            };
            debut += nombre;
            (debut - nombre, nombre)
        })
    }
}

/// Bits lus de gauche à droite ; au-delà de la fin, des zéros, comme le
/// `wBitStream` de FreeRDP.
struct LecteurBits<'a> {
    donnees: &'a [u8],
    position: usize,
}

impl<'a> LecteurBits<'a> {
    fn nouveau(donnees: &'a [u8]) -> Self {
        Self {
            donnees,
            position: 0,
        }
    }

    fn bit(&mut self) -> bool {
        let bit = self
            .donnees
            .get(self.position / 8)
            .is_some_and(|o| (o >> (7 - self.position % 8)) & 1 == 1);
        self.position += 1;
        bit
    }

    fn bits(&mut self, nombre: u32) -> u32 {
        (0..nombre).fold(0, |v, _| (v << 1) | u32::from(self.bit()))
    }
}

/// Le décodeur SRL (« simplified run-length ») d'un palier d'affinage :
/// les coefficients encore nuls y reçoivent leur première valeur.
///
/// Port fidèle de `progressive_rfx_srl_read` de FreeRDP, avec son paramètre
/// adaptatif `kp` (8 au départ), ses séries de zéros et son alternance entre
/// encodage des zéros et encodage unaire. Cet état traverse TOUTES les bandes
/// d'une composante : le flux est un seul ruban, pas dix.
struct LecteurSrl<'a> {
    bits: LecteurBits<'a>,
    kp: u32,
    zeros: u32,
    unaire: bool,
}

impl<'a> LecteurSrl<'a> {
    fn nouveau(donnees: &'a [u8]) -> Self {
        Self {
            bits: LecteurBits::nouveau(donnees),
            kp: 8,
            zeros: 0,
            unaire: false,
        }
    }

    /// Valeur du prochain coefficient nul : zéro, ou une grandeur signée sur
    /// `nb_bits` bits.
    fn lire(&mut self, nb_bits: u32) -> i16 {
        if self.zeros > 0 {
            self.zeros -= 1;
            return 0;
        }
        let k = self.kp / 8;
        if !self.unaire {
            if !self.bits.bit() {
                // Une série d'au moins 2^k zéros : celui-ci, puis les suivants.
                self.zeros = (1u32 << k) - 1;
                self.kp = (self.kp + 4).min(80);
                return 0;
            }
            // Moins de 2^k zéros, comptés sur k bits, puis une valeur.
            self.unaire = true;
            self.zeros = self.bits.bits(k);
            if self.zeros > 0 {
                self.zeros -= 1;
                return 0;
            }
        }
        self.unaire = false;
        let negatif = self.bits.bit();
        self.kp = self.kp.saturating_sub(6);
        let grandeur = if nb_bits == 1 {
            1
        } else {
            // Unaire : autant de zéros que la grandeur dépasse 1, borné par ce
            // que `nb_bits` peut représenter.
            let max = (1u32 << nb_bits) - 1;
            let mut g = 1;
            while g < max && !self.bits.bit() {
                g += 1;
            }
            g
        };
        let grandeur = i16::try_from(grandeur).unwrap_or(i16::MAX);
        if negatif {
            -grandeur
        } else {
            grandeur
        }
    }
}

/// Applique un palier d'affinage à une composante d'une tuile.
///
/// Réécrit d'après `progressive_rfx_upgrade_component` de FreeRDP, la
/// référence : `decode_upgrade_pass` d'IronRDP 0.9 repart du début des flux
/// SRL et brut à chaque bande, repart de `kp = 0`, lit la grandeur SRL comme
/// un code de Golomb-Rice, fait passer LL3 par le SRL et décale la valeur du
/// seul quantificateur progressif. Dès qu'un palier touche plusieurs bandes,
/// les bandes suivantes lisent les bits des précédentes : blocs flous de
/// 64 pixels, carré gris uniforme, bande abîmée dans la dernière colonne. Vu
/// le 2026-09-03 dans un avash Windows affichant un bureau, reproduit par le
/// rejeu de son enregistrement (`windows-surfaces-successives`, trame 20 :
/// une seule passe `upgrade` de 150 tuiles suffit).
///
/// Chaque bande apporte `nb_bits` bits de précision de plus, la différence
/// entre la position de bit du palier précédent et celle-ci ; la valeur lue
/// se place au-dessus du quantificateur de base (`position − 1`). Les
/// coefficients déjà non nuls reçoivent leur grandeur en brut, avec le signe
/// connu ; les nuls passent par le SRL ; LL3 est lue entièrement en brut.
fn affiner(
    srl: &[u8],
    brut: &[u8],
    base: &ComponentCodecQuant,
    prog: &ComponentCodecQuant,
    extrapoler: bool,
    etat: &mut EtatTuile,
    composante: usize,
) -> Result<()> {
    let mut lecteur_srl = LecteurSrl::nouveau(srl);
    let mut lecteur_brut = LecteurBits::nouveau(brut);
    let nouvelles = positions(base, prog);
    let coefficients = &mut etat.coefficients[composante];
    let signes = &mut etat.signes[composante];
    let positions_atteintes = &mut etat.positions[composante];
    for (b, (debut, nombre)) in bandes(extrapoler).into_iter().enumerate() {
        let nb_bits =
            u32::from(positions_atteintes[b].checked_sub(nouvelles[b]).context(
                "palier d'affinage qui recule : moins de précision qu'au palier précédent",
            )?);
        positions_atteintes[b] = nouvelles[b];
        if nb_bits == 0 {
            continue;
        }
        let decalage = u32::from(nouvelles[b].saturating_sub(1));
        let (coefficients, signes) = (
            coefficients
                .get_mut(debut..debut + nombre)
                .context("bande hors du tampon")?,
            signes
                .get_mut(debut..debut + nombre)
                .context("bande hors du tampon")?,
        );
        for (c, s) in coefficients.iter_mut().zip(signes.iter_mut()) {
            let valeur = if b == LL3 || *s > 0 {
                i32::try_from(lecteur_brut.bits(nb_bits)).unwrap_or(i32::MAX)
            } else if *s < 0 {
                -i32::try_from(lecteur_brut.bits(nb_bits)).unwrap_or(i32::MAX)
            } else {
                let v = lecteur_srl.lire(nb_bits);
                *s = i8::try_from(v.signum()).unwrap_or(0);
                i32::from(v)
            };
            *c = i16::try_from(i32::from(*c) + (valeur << decalage)).unwrap_or(if valeur < 0 {
                i16::MIN
            } else {
                i16::MAX
            });
        }
    }
    Ok(())
}

/// Nombre de tuiles dont on garde l'état : celui d'une surface au plafond de
/// résolution (8192 / 64 au carré), l'état des tuiles simples compris. Une
/// tuile hors de la surface n'en reçoit aucun, si bien qu'un serveur ne peut
/// faire enfler la mémoire qu'à hauteur de la surface qu'il a déclarée, elle-
/// même bornée à la création. Le plafond précédent, 4096, refusait un écran
/// 8K (8160 tuiles) depuis que les tuiles simples gardent leur état.
const TUILES_MAX: usize = (8192 / COTE) * (8192 / COTE);

/// L'état progressif d'UNE surface : les coefficients de ses tuiles, indexés
/// par leur position dans la grille.
///
/// Il vit dans la surface, pas dans le décodeur, comme chez FreeRDP : quand
/// Windows remplace la sienne — après l'écran d'avertissement de connexion, à
/// chaque redimensionnement — les tuiles de la nouvelle surface n'ont rien à
/// voir avec celles de l'ancienne à la même position. Et il survit à
/// `DeleteEncodingContext` : les tuiles « en différence » du contexte suivant
/// s'ajoutent à ces coefficients-là.
#[derive(Default)]
pub struct Tuiles {
    pub(crate) etats: std::collections::BTreeMap<(u16, u16), EtatTuile>,
}

impl Tuiles {
    /// La scène repart de zéro (`ResetGraphics`) : plus aucune tuile n'a de
    /// suite, ni palier d'affinage ni différence.
    pub fn oublier(&mut self) {
        self.etats.clear();
    }
}

impl std::fmt::Debug for Tuiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Tuiles({} en cours d'affinage)", self.etats.len())
    }
}

/// Tampons de travail, réutilisés d'une tuile à l'autre.
///
/// Un décodage de tuile alloue sinon 3 × 4096 i16 plus un tampon de sortie à
/// chaque fois — soixante fois par trame en 1024×768, plusieurs fois par
/// seconde.
pub struct Decodeur {
    composantes: [Vec<i16>; 3],
    temp: Vec<i16>,
    tuile: Vec<u8>,
}

impl Default for Decodeur {
    fn default() -> Self {
        Self {
            composantes: [vec![0; COEFFS], vec![0; COEFFS], vec![0; COEFFS]],
            temp: vec![0; COEFFS],
            tuile: vec![0; COTE * COTE * 4],
        }
    }
}

impl std::fmt::Debug for Decodeur {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Decodeur(progressif)")
    }
}

impl Decodeur {
    /// Décode un flux progressif dans `surface` et rend les zones modifiées.
    ///
    /// Le décodage proprement dit est isolé derrière `catch_unwind`. Les
    /// bibliothèques qui font le gros du travail — entropie RLGR, conversion
    /// YCbCr — indexent des tampons à partir de valeurs venues du serveur, et le
    /// fuzzing par mutation a montré qu'un flux corrompu peut y provoquer une
    /// panique. Elle ferait tomber tout le processus, donc la session, donc les
    /// autres onglets : n'importe quel serveur auquel on se connecte pourrait
    /// nous couper. Une image illisible doit rester une image illisible.
    ///
    /// Les tampons manipulés ne sont que des pixels et des coefficients : une
    /// interruption au milieu laisse au pire une tuile à moitié peinte, jamais
    /// un état incohérent.
    pub fn decoder(&mut self, flux: &[u8], surface: &mut Surface) -> Result<Vec<Zone>> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.decoder_interne(flux, surface)
        }))
        .unwrap_or_else(|_| {
            anyhow::bail!("Le décodeur s'est interrompu sur cette image : flux corrompu.")
        })
    }

    fn decoder_interne(&mut self, flux: &[u8], surface: &mut Surface) -> Result<Vec<Zone>> {
        let blocs = decode_progressive_stream(flux)
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("analyse du flux progressif")?;
        let mut zones = Vec::new();
        for bloc in &blocs {
            if let ProgressiveBlock::Region(region) = bloc {
                self.decoder_region(region, surface, &mut zones)?;
            }
        }
        Ok(zones)
    }

    fn decoder_region(
        &mut self,
        region: &ProgressiveRegion<'_>,
        surface: &mut Surface,
        zones: &mut Vec<Zone>,
    ) -> Result<()> {
        let extrapoler = region.uses_reduce_extrapolate();
        let quant = |i: u8| -> Result<ComponentCodecQuant> {
            region
                .quant_vals
                .get(usize::from(i))
                .copied()
                .context("indice de quantification hors table")
        };
        // Le palier de qualité désigne un jeu de quantificateurs progressifs.
        // Son absence n'est pas une erreur : une tuile simple n'en a pas.
        let palier = |q: u8| -> [ComponentCodecQuant; 3] {
            region
                .quant_prog_vals
                .get(usize::from(q))
                .map_or([ComponentCodecQuant::LOSSLESS; 3], |p| {
                    [p.y_quant, p.cb_quant, p.cr_quant]
                })
        };

        for tuile in &region.tiles {
            let (x_idx, y_idx) = match tuile {
                ProgressiveTile::Simple(t) => (t.x_idx, t.y_idx),
                ProgressiveTile::First(t) => (t.x_idx, t.y_idx),
                ProgressiveTile::Upgrade(t) => (t.x_idx, t.y_idx),
            };
            match tuile {
                // Une tuile simple est un premier palier sans suite prévue :
                // même chemin, quantificateur progressif sans perte.
                ProgressiveTile::Simple(t) => {
                    let bases = [
                        quant(t.quant_idx_y)?,
                        quant(t.quant_idx_cb)?,
                        quant(t.quant_idx_cr)?,
                    ];
                    self.premier_palier(
                        surface,
                        (x_idx, y_idx),
                        &bases,
                        &[ComponentCodecQuant::LOSSLESS; 3],
                        [t.y_data, t.cb_data, t.cr_data],
                        t.flags & DRAPEAU_DIFFERENCE != 0,
                        extrapoler,
                    )?;
                }
                ProgressiveTile::First(t) => {
                    let bases = [
                        quant(t.quant_idx_y)?,
                        quant(t.quant_idx_cb)?,
                        quant(t.quant_idx_cr)?,
                    ];
                    self.premier_palier(
                        surface,
                        (x_idx, y_idx),
                        &bases,
                        &palier(t.quality),
                        [t.y_data, t.cb_data, t.cr_data],
                        t.flags & DRAPEAU_DIFFERENCE != 0,
                        extrapoler,
                    )?;
                }
                ProgressiveTile::Upgrade(t) => {
                    // Palier suivant : sans le premier, il n'y a rien à affiner.
                    let Some(etat) = surface.tuiles.etats.get_mut(&(x_idx, y_idx)) else {
                        continue;
                    };
                    // Le palier porte ses propres indices de base : FreeRDP
                    // les relit à chaque fois (et s'étonne s'ils changent).
                    let bases = [
                        quant(t.quant_idx_y)?,
                        quant(t.quant_idx_cb)?,
                        quant(t.quant_idx_cr)?,
                    ];
                    let prog = palier(t.quality);
                    let srl = [t.y_srl_data, t.cb_srl_data, t.cr_srl_data];
                    let brut = [t.y_raw_data, t.cb_raw_data, t.cr_raw_data];
                    for n in 0..3 {
                        affiner(srl[n], brut[n], &bases[n], &prog[n], extrapoler, etat, n)
                            .context("palier d'affinage d'une composante")?;
                        self.composantes[n].copy_from_slice(&etat.coefficients[n]);
                    }
                }
            }
            // La transformée détruit les coefficients : elle ne travaille donc
            // que sur la copie de travail, jamais sur l'état conservé.
            for n in 0..3 {
                if extrapoler {
                    dwt_extrapolate::decode(&mut self.composantes[n], &mut self.temp);
                } else {
                    dwt::decode(&mut self.composantes[n], &mut self.temp);
                }
            }
            let [y, cb, cr] = &self.composantes;
            ycbcr_to_rgba(
                YCbCrBuffer {
                    y: &y[..COEFFS],
                    cb: &cb[..COEFFS],
                    cr: &cr[..COEFFS],
                },
                &mut self.tuile,
            )
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("conversion YCbCr")?;
            if let Some(z) = reporter(&self.tuile, x_idx, y_idx, surface) {
                zones.push(z);
            }
        }
        Ok(())
    }

    /// Premier palier d'une tuile, simple ou `first` : décode les trois
    /// composantes, fixe l'état gardé pour la surface et laisse dans les
    /// tampons de travail les coefficients à transformer.
    ///
    /// Une tuile « en différence » (bit 0 de ses drapeaux, MS-RDPEGFX
    /// 2.2.4.2.1.4.2) ne porte que l'écart avec les coefficients déjà gardés
    /// pour cette position : on l'y ajoute, comme `progressive_rfx_dwt_2d_
    /// decode` de FreeRDP. La prendre pour une image complète rend l'écart lui-
    /// même — un coin de bureau inchangé devient un carré gris uniforme, un
    /// coin retouché une bouillie de blocs. Windows en envoie dès qu'il rouvre
    /// un contexte de codec sur une surface qui a déjà servi : vu le
    /// 2026-09-03 dans un avash Windows affichant un bureau, reproduit par
    /// `windows-surfaces-successives` (trame 19, quinze tuiles en différence).
    /// IronRDP 0.9 lit le drapeau et n'en fait rien.
    ///
    /// Les signes gardés sont ceux de l'écart, pas de la somme : c'est ce que
    /// FreeRDP fait, et le palier d'affinage suivant lit ses flux d'après eux.
    #[allow(clippy::too_many_arguments)]
    fn premier_palier(
        &mut self,
        surface: &mut Surface,
        cle: (u16, u16),
        bases: &[ComponentCodecQuant; 3],
        prog: &[ComponentCodecQuant; 3],
        flux: [&[u8]; 3],
        difference: bool,
        extrapoler: bool,
    ) -> Result<()> {
        // Une tuile hors de la surface ne se peint pas (`reporter` l'ignore)
        // et ne mérite pas d'état : ses coordonnées viennent du réseau, et
        // chaque état pèse trente-six kilooctets.
        if usize::from(cle.0) * COTE >= usize::from(surface.largeur)
            || usize::from(cle.1) * COTE >= usize::from(surface.hauteur)
        {
            return Ok(());
        }
        let etats = &mut surface.tuiles.etats;
        if etats.len() >= TUILES_MAX && !etats.contains_key(&cle) {
            anyhow::bail!("Trop de tuiles en cours d'affinage : {} .", etats.len());
        }
        let etat = etats.entry(cle).or_default();
        for n in 0..3 {
            // Le décodeur entropique ne réécrit pas forcément tout le tampon :
            // ce qui n'est pas dans le flux vaut zéro, pas la tuile d'avant.
            self.composantes[n].fill(0);
            progressive::decode_first_pass(
                flux[n],
                &bases[n],
                &prog[n],
                extrapoler,
                &mut self.composantes[n],
                &mut etat.signes[n],
            )
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("premier palier d'une composante")?;
            if difference {
                for (c, d) in etat.coefficients[n].iter_mut().zip(&self.composantes[n]) {
                    *c = c.saturating_add(*d);
                }
                self.composantes[n].copy_from_slice(&etat.coefficients[n]);
            } else {
                etat.coefficients[n].copy_from_slice(&self.composantes[n]);
            }
            etat.positions[n] = positions(&bases[n], &prog[n]);
        }
        Ok(())
    }
}

/// Recopie une tuile 64×64 dans la surface, en la rognant sur ses bords.
///
/// Une surface dont les côtés ne sont pas multiples de 64 reçoit des tuiles qui
/// débordent : les recopier sans rogner écrirait dans la ligne suivante et
/// produirait l'image en escalier qu'on a déjà vue sur xrdp.
fn reporter(tuile: &[u8], x_idx: u16, y_idx: u16, surface: &mut Surface) -> Option<Zone> {
    let x0 = usize::from(x_idx) * COTE;
    let y0 = usize::from(y_idx) * COTE;
    let sl = usize::from(surface.largeur);
    let sh = usize::from(surface.hauteur);
    if x0 >= sl || y0 >= sh {
        return None;
    }
    let largeur = COTE.min(sl - x0);
    let hauteur = COTE.min(sh - y0);
    for ligne in 0..hauteur {
        let src = (ligne * COTE) * 4;
        let dst = ((y0 + ligne) * sl + x0) * 4;
        surface.pixels[dst..dst + largeur * 4].copy_from_slice(&tuile[src..src + largeur * 4]);
    }
    Some(Zone {
        x: x_idx * COTE as u16,
        y: y_idx * COTE as u16,
        largeur: u16::try_from(largeur).unwrap_or(u16::MAX),
        hauteur: u16::try_from(hauteur).unwrap_or(u16::MAX),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        affiner, bandes, positions, reporter, ComponentCodecQuant, EtatTuile, LecteurSrl, Surface,
        Zone, COEFFS, COTE, LL3, NB_BANDES,
    };

    /// Quantificateur uniforme : la même valeur sur les dix bandes.
    fn uniforme(v: u8) -> ComponentCodecQuant {
        ComponentCodecQuant {
            ll3: v,
            hl3: v,
            lh3: v,
            hh3: v,
            hl2: v,
            lh2: v,
            hh2: v,
            hl1: v,
            lh1: v,
            hh1: v,
        }
    }

    #[test]
    fn le_lecteur_srl_suit_freerdp() {
        // Déroulé à la main d'après `progressive_rfx_srl_read`, kp = 8 (k = 1) :
        //   « 0 »            une série de 2^1 zéros (celui-ci et le suivant), kp → 12
        //   « 1 » puis « 1 »  moins de deux zéros, en l'occurrence un
        //   « 0 » « 0 0 1 »   signe +, grandeur unaire 3 sur 3 bits, kp → 6 (k = 0)
        //   « 1 » « 1 » « 1 »  aucun zéro, signe −, grandeur 1
        // Soit 0110 0011 11 : 0x63 0xC0.
        let mut l = LecteurSrl::nouveau(&[0x63, 0xC0]);
        let lus: Vec<i16> = (0..5).map(|_| l.lire(3)).collect();
        assert_eq!(lus, [0, 0, 0, 3, -1]);
        // Au-delà du flux, des zéros : jamais de lecture hors du tampon.
        assert_eq!(l.lire(3), 0);
    }

    #[test]
    fn les_bandes_d_un_palier_lisent_le_meme_ruban_a_la_suite() {
        // Trouvé en relisant IronRDP 0.9 après `windows-surfaces-successives` :
        // son palier repartait du début du flux brut à chaque bande. Ici HL1
        // (1023 coefficients connus, lus en brut) puis LL3 (81, toujours en
        // brut) reçoivent un bit chacun ; le ruban porte 1023 zéros puis 81 uns.
        // Un décodeur qui repart du début donnerait des zéros à LL3.
        let mut brut = vec![0u8; 127];
        brut.push(0x01);
        brut.extend(std::iter::repeat_n(0xFF, 10));
        let mut etat = EtatTuile::default();
        let (premiere, n_premiere) = bandes(true)[0];
        etat.signes[0][premiere..premiere + n_premiere].fill(1);
        let base = uniforme(6);
        let mut precedent = uniforme(0);
        precedent.hl1 = 1;
        precedent.ll3 = 1;
        etat.positions[0] = positions(&base, &precedent);
        affiner(&[], &brut, &base, &uniforme(0), true, &mut etat, 0).expect("palier lisible");
        let (derniere, n_derniere) = bandes(true)[LL3];
        // Un bit à la position 6 (base + progressif − 1 = 5) vaut 32.
        assert!(etat.coefficients[0][derniere..derniere + n_derniere]
            .iter()
            .all(|&c| c == 32));
        assert!(etat.coefficients[0][..n_premiere].iter().all(|&c| c == 0));
        assert_eq!(etat.positions[0], [6; NB_BANDES]);
    }

    #[test]
    fn un_palier_qui_recule_est_refuse() {
        // Moins de précision qu'au palier précédent n'a pas de sens : FreeRDP
        // refuse la tuile, nous aussi, sans rien lire.
        let base = uniforme(6);
        let mut etat = EtatTuile::default();
        etat.positions[0] = positions(&base, &uniforme(0));
        assert!(affiner(&[], &[], &base, &uniforme(2), true, &mut etat, 0).is_err());
    }

    #[test]
    fn une_tuile_hors_de_la_surface_ne_laisse_aucun_etat() {
        // Les indices de tuile viennent du réseau : sur une surface de 64 × 64,
        // la tuile (5, 0) ne se peint pas et ne doit pas non plus coûter ses
        // trente-six kilooctets d'état, sinon un serveur remplit la mémoire à
        // coups de coordonnées inventées.
        use ironrdp::graphics::progressive::encode_first_pass;
        let base = uniforme(6);
        let mut composante = vec![40i16; COEFFS];
        let mut donnees = vec![0u8; 4 * COEFFS];
        let n = encode_first_pass(&mut composante, &mut donnees, &base, &uniforme(0), true)
            .expect("encodage");
        let flux = &donnees[..n];
        let mut d = super::Decodeur::default();
        let mut s = Surface::nouvelle(64, 64);
        for cle in [(5, 0), (0, 9), (u16::MAX, u16::MAX)] {
            d.premier_palier(
                &mut s,
                cle,
                &[base; 3],
                &[uniforme(0); 3],
                [flux; 3],
                false,
                true,
            )
            .expect("ignorée sans erreur");
        }
        assert!(s.tuiles.etats.is_empty());
        d.premier_palier(
            &mut s,
            (0, 0),
            &[base; 3],
            &[uniforme(0); 3],
            [flux; 3],
            false,
            true,
        )
        .expect("dans la surface");
        assert_eq!(s.tuiles.etats.len(), 1);
    }

    #[test]
    fn une_tuile_en_difference_s_ajoute_aux_coefficients_gardes() {
        // Trame 19 de `windows-surfaces-successives` : quinze tuiles `first` en
        // différence après un `DeleteEncodingContext`. Prises pour des images
        // complètes, elles rendaient l'écart lui-même (carré gris en (0, 0)).
        use ironrdp::graphics::progressive::encode_first_pass;
        let base = uniforme(6);
        let prog = uniforme(0);
        let mut composante = vec![40i16; COEFFS];
        let mut donnees = vec![0u8; 4 * COEFFS];
        let n = encode_first_pass(&mut composante, &mut donnees, &base, &prog, true)
            .expect("encodage d'une composante unie");
        let flux = &donnees[..n];
        let mut d = super::Decodeur::default();
        let mut s = Surface::nouvelle(64, 64);
        let premier = |d: &mut super::Decodeur, s: &mut Surface, difference: bool| {
            d.premier_palier(
                s,
                (0, 0),
                &[base; 3],
                &[prog; 3],
                [flux; 3],
                difference,
                true,
            )
            .expect("premier palier");
            s.tuiles.etats[&(0, 0)].coefficients[0].clone()
        };
        let seul = premier(&mut d, &mut s, false);
        assert!(seul.iter().any(|&c| c != 0), "la composante n'est pas vide");
        // Une image complète remplace ; une différence s'ajoute.
        assert_eq!(premier(&mut d, &mut s, false), seul);
        let double = premier(&mut d, &mut s, true);
        assert!(double
            .iter()
            .zip(&seul)
            .all(|(&deux, &un)| deux == un.saturating_mul(2)));
        // Et les tampons de travail, ceux que la transformée va lire, portent
        // bien la somme.
        assert_eq!(d.composantes[0], double);
    }

    #[test]
    fn une_tuile_de_bord_est_rognee_et_non_debordante() {
        // 100 de large : la deuxième colonne de tuiles n'a que 36 pixels utiles.
        let mut s = Surface::nouvelle(100, 70);
        let tuile = vec![0xABu8; COTE * COTE * 4];
        let z = reporter(&tuile, 1, 0, &mut s).expect("tuile dans la surface");
        assert_eq!(
            z,
            Zone {
                x: 64,
                y: 0,
                largeur: 36,
                hauteur: 64
            }
        );
        // Le dernier pixel de la première ligne est peint…
        assert_eq!(s.pixels[(99) * 4], 0xAB);
        // …et le premier de la deuxième ligne ne l'est pas par débordement.
        assert_eq!(s.pixels[(100) * 4], 0x00);
    }

    #[test]
    fn une_tuile_hors_surface_est_ignoree() {
        // Un serveur qui annonce une tuile au-delà de la surface ne doit pas
        // faire paniquer le client : c'est une entrée venue du réseau.
        let mut s = Surface::nouvelle(64, 64);
        let tuile = vec![0xFFu8; COTE * COTE * 4];
        assert!(reporter(&tuile, 5, 0, &mut s).is_none());
        assert!(reporter(&tuile, 0, 9, &mut s).is_none());
        assert!(s.pixels.iter().all(|&o| o == 0));
    }

    #[test]
    fn la_hauteur_est_rognee_comme_la_largeur() {
        let mut s = Surface::nouvelle(64, 70);
        let tuile = vec![0x11u8; COTE * COTE * 4];
        let z = reporter(&tuile, 0, 1, &mut s).expect("tuile dans la surface");
        assert_eq!(z.hauteur, 6, "70 - 64 lignes utiles");
        assert_eq!(s.pixels.len(), 64 * 70 * 4);
    }

    #[test]
    fn un_flux_illisible_rend_une_erreur_et_ne_fait_pas_tomber_le_processus() {
        // Le décodage repose sur des bibliothèques qui indexent des tampons à
        // partir de valeurs venues du serveur. Le fuzzing par mutation y a
        // trouvé deux paniques ; sans le filet, n'importe quel serveur pouvait
        // arrêter le processus — et donc toutes les sessions ouvertes, pas
        // seulement la sienne.
        let mut d = super::Decodeur::default();
        let mut s = Surface::nouvelle(64, 64);
        for flux in [
            vec![],
            vec![0xFF; 3],
            vec![0xAA; 64],
            (0..512u16).map(|i| (i % 251) as u8).collect(),
        ] {
            // Refusé, ou lu sans rien produire : dans les deux cas l'appel rend
            // la main, et aucun pixel n'est peint à partir de n'importe quoi.
            let zones = d.decoder(&flux, &mut s).unwrap_or_default();
            assert!(zones.is_empty(), "du bruit ne peut pas produire une image");
        }
        assert!(s.pixels.iter().all(|&o| o == 0), "la surface reste intacte");
    }
}
