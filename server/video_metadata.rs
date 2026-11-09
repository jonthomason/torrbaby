//! Preserve Matroska display/color metadata in MP4 visual sample entry boxes.
use anyhow::{Context, Result, ensure};
use matroska_demuxer::{MatrixCoefficients, Primaries, Range, TransferCharacteristics, Video};

pub fn boxes(v: &Video) -> Result<Vec<u8>> {
    ensure!(
        [
            v.pixel_crop_bottom(),
            v.pixel_crop_top(),
            v.pixel_crop_left(),
            v.pixel_crop_right()
        ]
        .iter()
        .all(|n| n.unwrap_or(0) == 0),
        "Cropped video needs clean-aperture support"
    );
    let mut out = Vec::new();
    if v.display_width().is_some() || v.display_height().is_some() {
        let w = v
            .display_width()
            .map(|n| n.get())
            .unwrap_or(v.pixel_width().get());
        let h = v
            .display_height()
            .map(|n| n.get())
            .unwrap_or(v.pixel_height().get());
        let mut numerator = w
            .checked_mul(v.pixel_height().get())
            .context("Aspect ratio overflow")?;
        let mut denominator = h
            .checked_mul(v.pixel_width().get())
            .context("Aspect ratio overflow")?;
        let (mut a, mut b) = (numerator, denominator);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        numerator /= a;
        denominator /= a;
        let mut body = Vec::new();
        body.extend(u32::try_from(numerator)?.to_be_bytes());
        body.extend(u32::try_from(denominator)?.to_be_bytes());
        out.extend(atom(b"pasp", &body)?);
    }
    if let Some(c) = v.colour() {
        let matrix = c
            .matrix_coefficients()
            .filter(|x| *x != MatrixCoefficients::Unknown);
        let transfer = c
            .transfer_characteristics()
            .filter(|x| *x != TransferCharacteristics::Unknown);
        let primaries = c.primaries().filter(|x| *x != Primaries::Unknown);
        if matrix.is_some()
            || transfer.is_some()
            || primaries.is_some()
            || matches!(c.range(), Some(Range::Broadcast | Range::Full))
        {
            let full = match c.range() {
                Some(Range::Full) => Some(true),
                Some(Range::Broadcast) => Some(false),
                _ => None,
            };
            let mut body = if full.is_some() {
                b"nclx".to_vec()
            } else {
                b"nclc".to_vec()
            };
            let p = primaries
                .map(|v| code(v, Primaries::from))
                .transpose()?
                .unwrap_or(2);
            let t = transfer
                .map(|v| code(v, TransferCharacteristics::from))
                .transpose()?
                .unwrap_or(2);
            let m = matrix
                .map(|v| code(v, MatrixCoefficients::from))
                .transpose()?
                .unwrap_or(2);
            for n in [p, t, m] {
                body.extend(n.to_be_bytes());
            }
            if let Some(full) = full {
                body.push(if full { 128 } else { 0 });
            }
            out.extend(atom(b"colr", &body)?);
        }
        if c.max_cll().is_some() || c.max_fall().is_some() {
            let mut body = Vec::new();
            for n in [c.max_cll().unwrap_or(0), c.max_fall().unwrap_or(0)] {
                body.extend(u16::try_from(n)?.to_be_bytes());
            }
            out.extend(atom(b"clli", &body)?);
        }
        if let Some(m) = c.mastering_metadata() {
            let mut body = Vec::new();
            // ISO mdcv uses G,B,R order and 1/50000 chromaticity units.
            for value in [
                m.primary_g_chromaticity_x(),
                m.primary_g_chromaticity_y(),
                m.primary_b_chromaticity_x(),
                m.primary_b_chromaticity_y(),
                m.primary_r_chromaticity_x(),
                m.primary_r_chromaticity_y(),
                m.white_point_chromaticity_x(),
                m.white_point_chromaticity_y(),
            ] {
                let n = value.context("Incomplete HDR mastering primaries")?;
                ensure!(
                    n.is_finite() && (0.0..=1.0).contains(&n),
                    "Invalid HDR chromaticity"
                );
                body.extend(((n * 50000.0).round() as u16).to_be_bytes());
            }
            for value in [m.luminance_max(), m.luminance_min()] {
                let n = value.context("Incomplete HDR mastering luminance")?;
                ensure!(
                    n.is_finite() && n >= 0.0 && n * 10000.0 <= u32::MAX as f64,
                    "Invalid HDR luminance"
                );
                body.extend(((n * 10000.0).round() as u32).to_be_bytes());
            }
            out.extend(atom(b"mdcv", &body)?);
        }
    }
    Ok(out)
}
fn code<T: PartialEq>(value: T, decode: impl Fn(u64) -> T) -> Result<u16> {
    (0..=255)
        .find(|n| decode(*n) == value)
        .map(|n| n as u16)
        .context("Unknown color code point")
}
pub fn atom(kind: &[u8; 4], body: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend(u32::try_from(body.len() + 8)?.to_be_bytes());
    out.extend(kind);
    out.extend(body);
    Ok(out)
}
