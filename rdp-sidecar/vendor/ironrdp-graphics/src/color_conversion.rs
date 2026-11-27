use std::io;

use yuv::{
    BufferStoreMut, YuvError, YuvPlanarImage, YuvPlanarImageMut, rdp_abgr_to_yuv444, rdp_argb_to_yuv444,
    rdp_bgra_to_yuv444, rdp_rgba_to_yuv444, rdp_yuv444_to_argb, rdp_yuv444_to_rgba,
};

use crate::image_processing::PixelFormat;

/// Plus grande luminance que `yuv` convertit sans déborder : il calcule
/// `y + 4096` sur un i16. Une valeur au-delà ne vient que d'un flux hostile ou
/// corrompu (un pixel valide reste loin en dessous) ; elle est saturée, comme
/// le serait le pixel. Trouvé par cargo-fuzz le 27 novembre 2026 (Avash, cible
/// egfx_progressif) : panique en debug, couleurs fausses en release.
const LUMINANCE_MAX: i16 = i16::MAX - 4096;

/// La luminance telle quelle, ou une copie saturée si une valeur dépasse
/// `LUMINANCE_MAX` : la copie ne se paie que sur un flux hostile.
fn luminance_bornee(y: &[i16]) -> std::borrow::Cow<'_, [i16]> {
    if y.iter().all(|&v| v <= LUMINANCE_MAX) {
        std::borrow::Cow::Borrowed(y)
    } else {
        std::borrow::Cow::Owned(y.iter().map(|&v| v.min(LUMINANCE_MAX)).collect())
    }
}

// FIXME: used for the test suite, we may want to drop it
pub fn ycbcr_to_argb(input: YCbCrBuffer<'_>, output: &mut [u8]) -> io::Result<()> {
    let y = luminance_bornee(input.y);
    let len = u32::try_from(output.len()).map_err(io::Error::other)?;
    let width = len / 4;
    let planar = YuvPlanarImage {
        y_plane: &y,
        y_stride: width,
        u_plane: input.cb,
        u_stride: width,
        v_plane: input.cr,
        v_stride: width,
        width,
        height: 1,
    };
    rdp_yuv444_to_argb(&planar, output, len).map_err(io::Error::other)
}

pub fn ycbcr_to_rgba(input: YCbCrBuffer<'_>, output: &mut [u8]) -> io::Result<()> {
    let y = luminance_bornee(input.y);
    let len = u32::try_from(output.len()).map_err(io::Error::other)?;
    let width = len / 4;
    let planar = YuvPlanarImage {
        y_plane: &y,
        y_stride: width,
        u_plane: input.cb,
        u_stride: width,
        v_plane: input.cr,
        v_stride: width,
        width,
        height: 1,
    };
    rdp_yuv444_to_rgba(&planar, output, len).map_err(io::Error::other)
}

/// # Panics
///
/// - Panics if `width` > 64.
/// - Panics if `height` > 64.
#[allow(clippy::too_many_arguments)]
pub fn to_64x64_ycbcr_tile(
    input: &[u8],
    width: u32,
    height: u32,
    stride: u32,
    format: PixelFormat,
    y: &mut [i16; 64 * 64],
    cb: &mut [i16; 64 * 64],
    cr: &mut [i16; 64 * 64],
) -> Result<(), YuvError> {
    assert!(width <= 64);
    assert!(height <= 64);

    let y_plane = BufferStoreMut::Borrowed(y);
    let u_plane = BufferStoreMut::Borrowed(cb);
    let v_plane = BufferStoreMut::Borrowed(cr);
    let mut plane = YuvPlanarImageMut {
        y_plane,
        y_stride: 64,
        u_plane,
        u_stride: 64,
        v_plane,
        v_stride: 64,
        width,
        height,
    };

    match format {
        PixelFormat::RgbA32 | PixelFormat::RgbX32 => rdp_rgba_to_yuv444(&mut plane, input, stride),
        PixelFormat::ARgb32 | PixelFormat::XRgb32 => rdp_argb_to_yuv444(&mut plane, input, stride),
        PixelFormat::BgrA32 | PixelFormat::BgrX32 => rdp_bgra_to_yuv444(&mut plane, input, stride),
        PixelFormat::ABgr32 | PixelFormat::XBgr32 => rdp_abgr_to_yuv444(&mut plane, input, stride),
    }
}

/// Convert a 15-bit RDP color (RGB555) to RGB. Input value should be represented in
/// little-endian format.
///
/// Layout: `[0, R4:R0, G4:G0, B4:B0]` -- MSB unused, 5 bits per channel.
pub fn rdp_15bit_to_rgb(color: u16) -> [u8; 3] {
    #[allow(clippy::missing_panics_doc, reason = "unreachable panic (checked integer underflow)")]
    let out = {
        let r = u8::try_from(((((color >> 10) & 0x1f) * 527) + 23) >> 6).expect("max possible value is 255");
        let g = u8::try_from(((((color >> 5) & 0x1f) * 527) + 23) >> 6).expect("max possible value is 255");
        let b = u8::try_from((((color & 0x1f) * 527) + 23) >> 6).expect("max possible value is 255");
        [r, g, b]
    };

    out
}

/// Convert a 16-bit RDP color (RGB565) to RGB. Input value should be represented in
/// little-endian format.
pub fn rdp_16bit_to_rgb(color: u16) -> [u8; 3] {
    #[allow(clippy::missing_panics_doc, reason = "unreachable panic (checked integer underflow)")]
    let out = {
        let r = u8::try_from(((((color >> 11) & 0x1f) * 527) + 23) >> 6).expect("max possible value is 255");
        let g = u8::try_from(((((color >> 5) & 0x3f) * 259) + 33) >> 6).expect("max possible value is 255");
        let b = u8::try_from((((color & 0x1f) * 527) + 23) >> 6).expect("max possible value is 255");
        [r, g, b]
    };

    out
}

#[derive(Debug)]
pub struct YCbCrBuffer<'a> {
    pub y: &'a [i16],
    pub cb: &'a [i16],
    pub cr: &'a [i16],
}

impl Iterator for YCbCrBuffer<'_> {
    type Item = YCbCr;

    fn next(&mut self) -> Option<Self::Item> {
        if !self.y.is_empty() && !self.cb.is_empty() && !self.cr.is_empty() {
            let y = self.y[0];
            let cb = self.cb[0];
            let cr = self.cr[0];

            self.y = &self.y[1..];
            self.cb = &self.cb[1..];
            self.cr = &self.cr[1..];

            Some(YCbCr { y, cb, cr })
        } else {
            None
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct YCbCr {
    pub y: i16,
    pub cb: i16,
    pub cr: i16,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[cfg(test)]
mod tests_luminance_hostile {
    use super::*;

    /// Trouvé par cargo-fuzz le 27 novembre 2026 (cible egfx_progressif, audit
    /// QT-11) : `yuv` calcule `y + 4096` sur un i16, et une luminance hostile
    /// au-delà de 28 671 débordait (panique en debug, couleurs fausses en
    /// release).
    #[test]
    fn une_luminance_hostile_ne_fait_pas_paniquer_la_conversion() {
        let y = [i16::MAX; 4];
        let c = [0i16; 4];
        let mut sortie = [0u8; 16];
        ycbcr_to_rgba(YCbCrBuffer { y: &y, cb: &c, cr: &c }, &mut sortie).expect("conversion");
        assert_eq!(&sortie[..3], &[255, 255, 255], "luminance saturée : blanc");
        let mut argb = [0u8; 16];
        ycbcr_to_argb(YCbCrBuffer { y: &y, cb: &c, cr: &c }, &mut argb).expect("conversion");
    }

    #[test]
    fn une_luminance_valide_n_est_pas_touchee() {
        let y: Vec<i16> = (0..64).map(|i| i * 40 - 1200).collect();
        let c: Vec<i16> = (0..64).map(|i| i * 3 - 90).collect();
        assert!(matches!(luminance_bornee(&y), std::borrow::Cow::Borrowed(_)));
        let mut sortie = vec![0u8; 256];
        ycbcr_to_rgba(YCbCrBuffer { y: &y, cb: &c, cr: &c }, &mut sortie).expect("conversion");
        let planar = YuvPlanarImage {
            y_plane: &y,
            y_stride: 64,
            u_plane: &c,
            u_stride: 64,
            v_plane: &c,
            v_stride: 64,
            width: 64,
            height: 1,
        };
        let mut attendu = vec![0u8; 256];
        rdp_yuv444_to_rgba(&planar, &mut attendu, 256).expect("référence");
        assert_eq!(sortie, attendu);
    }
}
