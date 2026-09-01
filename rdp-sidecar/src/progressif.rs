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

/// L'état d'une tuile entre deux paliers de qualité.
///
/// Les coefficients sont conservés dans le domaine des fréquences : la
/// transformée en ondelettes inverse les détruirait, or le palier suivant
/// travaille dessus. On la fait donc sur une copie.
struct EtatTuile {
    coefficients: [Vec<i16>; 3],
    signes: [Vec<i8>; 3],
    /// Quantificateur progressif du palier précédent, par composante.
    quant: [ComponentCodecQuant; 3],
}

impl Default for EtatTuile {
    fn default() -> Self {
        Self {
            coefficients: [vec![0; COEFFS], vec![0; COEFFS], vec![0; COEFFS]],
            signes: [vec![0; COEFFS], vec![0; COEFFS], vec![0; COEFFS]],
            quant: [ComponentCodecQuant::LOSSLESS; 3],
        }
    }
}

/// Nombre de tuiles dont on garde l'état. Un écran 4K en compte un peu plus de
/// deux mille ; au-delà, c'est un serveur qui invente des coordonnées.
const TUILES_MAX: usize = 4096;

/// Tampons de travail, réutilisés d'une tuile à l'autre.
///
/// Un décodage de tuile alloue sinon 3 × 4096 i16 plus un tampon de sortie à
/// chaque fois — soixante fois par trame en 1024×768, plusieurs fois par
/// seconde.
pub struct Decodeur {
    composantes: [Vec<i16>; 3],
    signes: Vec<i8>,
    temp: Vec<i16>,
    tuile: Vec<u8>,
    /// État par tuile, indexé par sa position dans la grille.
    etats: std::collections::BTreeMap<(u16, u16), EtatTuile>,
}

impl Default for Decodeur {
    fn default() -> Self {
        Self {
            composantes: [vec![0; COEFFS], vec![0; COEFFS], vec![0; COEFFS]],
            signes: vec![0; COEFFS],
            temp: vec![0; COEFFS],
            tuile: vec![0; COTE * COTE * 4],
            etats: std::collections::BTreeMap::new(),
        }
    }
}

impl std::fmt::Debug for Decodeur {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Decodeur(progressif)")
    }
}

impl Decodeur {
    /// Referme le contexte d'affinage : les tuiles en cours n'ont plus de suite.
    ///
    /// Les garder ferait appliquer un palier d'amélioration à des coefficients
    /// qui ne sont plus ceux de l'image à l'écran.
    pub fn oublier_tuiles(&mut self) {
        self.etats.clear();
    }

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
                ProgressiveTile::Simple(t) => {
                    let bases = [
                        quant(t.quant_idx_y)?,
                        quant(t.quant_idx_cb)?,
                        quant(t.quant_idx_cr)?,
                    ];
                    let flux = [t.y_data, t.cb_data, t.cr_data];
                    for n in 0..3 {
                        progressive::decode_first_pass(
                            flux[n],
                            &bases[n],
                            &ComponentCodecQuant::LOSSLESS,
                            extrapoler,
                            &mut self.composantes[n],
                            &mut self.signes,
                        )
                        .map_err(|e| anyhow::anyhow!("{e}"))
                        .context("premier passage d'une composante")?;
                    }
                }
                ProgressiveTile::First(t) => {
                    // Premier palier : on décode ET on garde l'état, car les
                    // paliers suivants viendront s'y ajouter.
                    let bases = [
                        quant(t.quant_idx_y)?,
                        quant(t.quant_idx_cb)?,
                        quant(t.quant_idx_cr)?,
                    ];
                    let prog = palier(t.quality);
                    let flux = [t.y_data, t.cb_data, t.cr_data];
                    if self.etats.len() >= TUILES_MAX && !self.etats.contains_key(&(x_idx, y_idx)) {
                        anyhow::bail!(
                            "Trop de tuiles en cours d'affinage : {} .",
                            self.etats.len()
                        );
                    }
                    let etat = self.etats.entry((x_idx, y_idx)).or_default();
                    for n in 0..3 {
                        progressive::decode_first_pass(
                            flux[n],
                            &bases[n],
                            &prog[n],
                            extrapoler,
                            &mut etat.coefficients[n],
                            &mut etat.signes[n],
                        )
                        .map_err(|e| anyhow::anyhow!("{e}"))
                        .context("premier palier d'une composante")?;
                        etat.quant[n] = prog[n];
                        self.composantes[n].copy_from_slice(&etat.coefficients[n]);
                    }
                }
                ProgressiveTile::Upgrade(t) => {
                    // Palier suivant : sans le premier, il n'y a rien à affiner.
                    let Some(etat) = self.etats.get_mut(&(x_idx, y_idx)) else {
                        continue;
                    };
                    let prog = palier(t.quality);
                    let srl = [t.y_srl_data, t.cb_srl_data, t.cr_srl_data];
                    let brut = [t.y_raw_data, t.cb_raw_data, t.cr_raw_data];
                    for n in 0..3 {
                        progressive::decode_upgrade_pass(
                            srl[n],
                            brut[n],
                            &etat.quant[n],
                            &prog[n],
                            extrapoler,
                            &mut etat.coefficients[n],
                            &mut etat.signes[n],
                        );
                        etat.quant[n] = prog[n];
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
    use super::{reporter, Surface, Zone, COTE};

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
