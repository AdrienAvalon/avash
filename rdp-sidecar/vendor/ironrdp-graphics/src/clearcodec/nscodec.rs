//! NSCodec (MS-RDPNSC) as ClearCodec subcodec 1 (MS-RDPEGFX 2.2.4.6.2.1).
//!
//! Portage avash. Upstream 0.9.0 leaves this subcodec as a silent no-op, so a
//! region encoded with it stays black in the composite. Windows Server uses it
//! for exactly the colourful bits of the desktop: the taskbar icons (68×24,
//! 46×14, …), which then get cached and stamped around as black boxes. Seen on
//! 2026-09-03 in an avash displaying a Windows desktop, reproduced offline by
//! replaying its recording (`windows-clearcodec-nscodec`).
//!
//! Port of FreeRDP's `nsc_rle_decode`, `nsc_rle_decompress_data` and
//! `nsc_decode` (libfreerdp/codec/nsc.c), the reference implementation.
//!
//! Stream layout (MS-RDPNSC 2.2.1): four `u32` plane byte counts (luma,
//! orange chroma, green chroma, alpha), `ColorLossLevel` (1..=7),
//! `ChromaSubsamplingLevel`, two reserved bytes, then the planes. A plane
//! shorter than its expected size is RLE-compressed; an empty one is all
//! 0xFF; one at least as long is stored raw.

use ironrdp_core::{invalid_field_err, DecodeResult};

/// Decode an NSCodec bitmap of `width`×`height` into `output` (BGRA, row
/// stride `surface_width`) at (`x_start`, `y_start`).
///
/// The caller has already checked that the region fits the surface.
pub(crate) fn decode(
    data: &[u8],
    width: u16,
    height: u16,
    output: &mut [u8],
    x_start: u16,
    y_start: u16,
    surface_width: u16,
) -> DecodeResult<()> {
    if data.len() < 20 {
        return Err(invalid_field_err!(
            "nscodec",
            "stream shorter than its header"
        ));
    }
    let mut tailles = [0usize; 4];
    for (i, t) in tailles.iter_mut().enumerate() {
        let d = i * 4;
        *t = u32::from_le_bytes([data[d], data[d + 1], data[d + 2], data[d + 3]]) as usize;
    }
    let perte = data[16];
    if !(1..=7).contains(&perte) {
        return Err(invalid_field_err!("nscodec", "ColorLossLevel out of 1..=7"));
    }
    let sous_echantillonne = data[17] != 0;
    let total = tailles
        .iter()
        .try_fold(0usize, |a, &t| a.checked_add(t))
        .ok_or_else(|| invalid_field_err!("nscodec", "plane sizes overflow"))?;
    let planes_data = data
        .get(20..)
        .filter(|p| p.len() >= total)
        .ok_or_else(|| invalid_field_err!("nscodec", "planes shorter than announced"))?;

    let w = usize::from(width);
    let h = usize::from(height);
    // FreeRDP rounds the working width up to 8 and the height up to 2: the
    // luma plane is stored at the rounded width when chroma is subsampled,
    // and the chroma planes at half of both.
    let lw = w.div_ceil(8) * 8;
    let lh = h.div_ceil(2) * 2;
    let attendues = if sous_echantillonne {
        [lw * h, (lw / 2) * (lh / 2), (lw / 2) * (lh / 2), w * h]
    } else {
        [w * h; 4]
    };

    let mut plans: [Vec<u8>; 4] = Default::default();
    let mut debut = 0usize;
    for i in 0..4 {
        let brut = &planes_data[debut..debut + tailles[i]];
        debut += tailles[i];
        plans[i] = if tailles[i] == 0 {
            vec![0xFF; attendues[i]]
        } else if tailles[i] < attendues[i] {
            rle(brut, attendues[i])
                .ok_or_else(|| invalid_field_err!("nscodec", "corrupt RLE plane"))?
        } else {
            brut[..attendues[i]].to_vec()
        };
    }

    let decalage = u32::from(perte - 1);
    let sw = usize::from(surface_width);
    for y in 0..h {
        let (ly, cy, cstride) = if sous_echantillonne {
            (y * lw, (y >> 1) * (lw / 2), true)
        } else {
            (y * w, y * w, false)
        };
        for x in 0..w {
            let cx = if cstride { x >> 1 } else { x };
            let lum = i16::from(plans[0][ly + x]);
            // Colour loss recovery: the chroma byte is shifted left then read
            // back as a signed byte, exactly like FreeRDP.
            let chroma = |v: u8| i16::from((u16::from(v) << decalage) as u8 as i8);
            let co = chroma(plans[1][cy + cx]);
            let cg = chroma(plans[2][cy + cx]);
            let r = (lum + co - cg).clamp(0, 255) as u8;
            let g = (lum + cg).clamp(0, 255) as u8;
            let b = (lum - co - cg).clamp(0, 255) as u8;
            let dst = ((usize::from(y_start) + y) * sw + usize::from(x_start) + x) * 4;
            let Some(px) = output.get_mut(dst..dst + 4) else {
                return Err(invalid_field_err!("nscodec", "region exceeds output"));
            };
            px.copy_from_slice(&[b, g, r, 0xFF]);
        }
    }
    Ok(())
}

/// NSCodec run-length decoding of one plane (MS-RDPNSC 2.2.2, FreeRDP
/// `nsc_rle_decode`): a byte followed by the same byte announces a run whose
/// length is the next byte plus two, or, when that byte is 0xFF, a `u32`;
/// the last four bytes of the plane are always stored raw.
fn rle(entree: &[u8], taille: usize) -> Option<Vec<u8>> {
    let mut sortie = Vec::with_capacity(taille);
    let mut i = 0usize;
    let mut restant = taille;
    while restant > 4 {
        let valeur = *entree.get(i)?;
        i += 1;
        if restant == 5 {
            sortie.push(valeur);
            restant -= 1;
            continue;
        }
        if *entree.get(i)? == valeur {
            i += 1;
            let n = *entree.get(i)?;
            i += 1;
            let longueur = if n < 0xFF {
                usize::from(n) + 2
            } else {
                let b = entree.get(i..i + 4)?;
                i += 4;
                u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize
            };
            if longueur > restant {
                return None;
            }
            sortie.resize(sortie.len() + longueur, valeur);
            restant -= longueur;
        } else {
            sortie.push(valeur);
            restant -= 1;
        }
    }
    if restant != 4 {
        return None;
    }
    sortie.extend_from_slice(entree.get(i..i + 4)?);
    Some(sortie)
}

#[cfg(test)]
mod tests {
    use super::{decode, rle};

    #[test]
    fn rle_run_then_raw_tail() {
        // 50 50 <2> : run of four 50s ; then the last four bytes raw.
        let plane = rle(&[50, 50, 2, 1, 2, 3, 4], 8).unwrap();
        assert_eq!(plane, [50, 50, 50, 50, 1, 2, 3, 4]);
        // A run that overshoots, or a missing tail, is refused.
        assert!(rle(&[50, 50, 9, 1, 2, 3, 4], 8).is_none());
        assert!(rle(&[50, 50, 2, 1, 2], 8).is_none());
    }

    fn stream(tailles: [u32; 4], perte: u8, sous: u8, plans: &[&[u8]]) -> Vec<u8> {
        let mut s = Vec::new();
        for t in tailles {
            s.extend_from_slice(&t.to_le_bytes());
        }
        s.extend_from_slice(&[perte, sous, 0, 0]);
        for p in plans {
            s.extend_from_slice(p);
        }
        s
    }

    #[test]
    fn a_flat_grey_bitmap_without_subsampling() {
        // 2×2, raw planes, no chroma, no alpha plane (→ 0xFF).
        let s = stream([4, 4, 4, 0], 1, 0, &[&[100; 4], &[0; 4], &[0; 4]]);
        let mut out = vec![0u8; 3 * 3 * 4];
        decode(&s, 2, 2, &mut out, 1, 1, 3).unwrap();
        let px = |x: usize, y: usize| &out[(y * 3 + x) * 4..(y * 3 + x) * 4 + 4];
        assert_eq!(px(1, 1), [100, 100, 100, 0xFF]);
        assert_eq!(px(2, 2), [100, 100, 100, 0xFF]);
        // Outside the region, untouched.
        assert_eq!(px(0, 0), [0, 0, 0, 0]);
    }

    #[test]
    fn chroma_is_shifted_by_the_colour_loss_and_subsampled() {
        // 3×1 with subsampling: luma stored at width 8, chroma at 4×1.
        // ColorLossLevel 2 → shift 1 : Co 4 becomes 8.
        let luma = [10u8, 20, 30, 0, 0, 0, 0, 0];
        let s = stream([8, 4, 4, 0], 2, 1, &[&luma, &[4; 4], &[0; 4]]);
        let mut out = vec![0u8; 3 * 4];
        decode(&s, 3, 1, &mut out, 0, 0, 3).unwrap();
        // b = y − co, g = y, r = y + co.
        assert_eq!(&out[0..4], [2, 10, 18, 0xFF]);
        assert_eq!(&out[4..8], [12, 20, 28, 0xFF]);
        assert_eq!(&out[8..12], [22, 30, 38, 0xFF]);
    }

    #[test]
    fn a_short_or_lying_stream_is_refused() {
        assert!(decode(&[0; 10], 2, 2, &mut vec![0; 16], 0, 0, 2).is_err());
        // Announces 4 bytes of luma but carries none.
        let s = stream([4, 0, 0, 0], 1, 0, &[]);
        assert!(decode(&s, 2, 2, &mut vec![0; 16], 0, 0, 2).is_err());
        // ColorLossLevel 0 is out of range.
        let s = stream([4, 4, 4, 0], 0, 0, &[&[1; 4], &[0; 4], &[0; 4]]);
        assert!(decode(&s, 2, 2, &mut vec![0; 16], 0, 0, 2).is_err());
    }
}
