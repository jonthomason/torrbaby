//! Adapt Matroska PGS segments to the bitmap decoder; retain container timing.
use anyhow::{Context, Result, ensure};
use oxideav_core::{CodecId, CodecParameters, Decoder, Frame, Packet, TimeBase};
use serde_json::{Value, json};
use std::{fs, path::Path};

pub struct Pgs {
    decoder: Box<dyn Decoder>,
    set: Vec<u8>,
    start: Option<u64>,
    end: Option<u64>,
    forced: bool,
    active: Option<(u64, Option<u64>, usize, usize, Vec<u8>, bool)>,
    cues: Vec<Value>,
    bytes: usize,
    objects: std::collections::HashMap<u16, (usize, usize, usize, bool)>,
    palettes: std::collections::HashSet<u8>,
    references: Vec<u16>,
    palette: u8,
}
impl Pgs {
    pub fn new() -> Result<Self> {
        Ok(Self {
            decoder: oxideav_sub_image::pgs::make_decoder(&CodecParameters::video(CodecId::new(
                "pgs",
            )))?,
            set: vec![],
            start: None,
            end: None,
            forced: false,
            active: None,
            cues: vec![],
            bytes: 0,
            objects: Default::default(),
            palettes: Default::default(),
            references: vec![],
            palette: 0,
        })
    }
    pub fn push(
        &mut self,
        data: &[u8],
        time: u64,
        duration: Option<u64>,
        out: &Path,
    ) -> Result<()> {
        let mut pos = 0;
        while pos < data.len() {
            ensure!(data.len() - pos >= 3, "Truncated PGS segment header");
            let kind = data[pos];
            let len = u16::from_be_bytes([data[pos + 1], data[pos + 2]]) as usize;
            ensure!(len <= data.len() - pos - 3, "Truncated PGS segment");
            let body = &data[pos + 3..pos + 3 + len];
            if kind == 0x16 {
                ensure!(self.set.is_empty(), "Unclosed PGS display set");
                ensure!(body.len() >= 11, "Truncated PGS composition");
                let w = u16::from_be_bytes([body[0], body[1]]) as usize;
                let h = u16::from_be_bytes([body[2], body[3]]) as usize;
                ensure!(
                    w > 0 && h > 0 && w <= 4096 && h <= 2160,
                    "PGS canvas too large"
                );
                ensure!(body[10] <= 16, "Too many PGS objects");
                self.start = Some(time);
                self.end = duration
                    .filter(|d| *d > 0)
                    .and_then(|d| time.checked_add(d));
                if body[7] == 0x80 {
                    self.objects.clear();
                    self.palettes.clear();
                }
                self.references.clear();
                self.palette = body[9];
                let mut i = 11;
                self.forced = false;
                for _ in 0..body[10] {
                    ensure!(i + 8 <= body.len(), "Truncated PGS object reference");
                    self.references
                        .push(u16::from_be_bytes([body[i], body[i + 1]]));
                    self.forced |= body[i + 3] & 0x40 != 0;
                    i += if body[i + 3] & 0x80 != 0 { 16 } else { 8 };
                    ensure!(i <= body.len(), "Truncated PGS crop");
                }
            }
            ensure!(self.start.is_some(), "PGS segment without composition");
            if kind == 0x14 {
                ensure!(body.len() >= 2, "Truncated PGS palette");
                self.palettes.insert(body[0]);
            }
            if kind == 0x15 {
                ensure!(body.len() >= 4, "Truncated PGS object");
                let id = u16::from_be_bytes([body[0], body[1]]);
                if body[3] & 0x80 != 0 {
                    ensure!(body.len() >= 11, "Truncated PGS object dimensions");
                    let w = u16::from_be_bytes([body[7], body[8]]) as usize;
                    let h = u16::from_be_bytes([body[9], body[10]]) as usize;
                    ensure!(
                        w > 0 && h > 0 && w <= 4096 && h <= 2160,
                        "PGS object too large"
                    );
                    let declared =
                        ((body[4] as usize) << 16) | ((body[5] as usize) << 8) | body[6] as usize;
                    ensure!(
                        declared >= 4 && declared <= 16 * 1024 * 1024,
                        "Invalid PGS object length"
                    );
                    self.objects.insert(id, (w * h, declared + 3, 0, false));
                }
                let object = self
                    .objects
                    .get_mut(&id)
                    .context("PGS fragment without initial fragment")?;
                ensure!(!object.3, "Unexpected PGS continuation");
                object.2 += body.len() - 4;
                ensure!(object.2 <= object.1, "PGS object exceeds declared length");
                if body[3] & 0x40 != 0 {
                    ensure!(object.2 == object.1, "Incomplete PGS object");
                    object.3 = true;
                }
                ensure!(
                    self.objects.len() <= 256
                        && self.objects.values().map(|v| v.0 + v.1).sum::<usize>()
                            <= 64 * 1024 * 1024,
                    "PGS epoch storage limit"
                );
            }
            ensure!(
                self.set.len() + 13 + len <= 16 * 1024 * 1024,
                "PGS display set too large"
            );
            // The decoder expects SUP headers. Timing is supplied separately from MKV.
            self.set.extend_from_slice(b"PG\0\0\0\0\0\0\0\0");
            self.set.extend_from_slice(&data[pos..pos + 3 + len]);
            pos += 3 + len;
            if kind == 0x80 {
                ensure!(
                    self.references.is_empty()
                        || (self.palettes.contains(&self.palette)
                            && self
                                .references
                                .iter()
                                .all(|id| self.objects.get(id).is_some_and(|o| o.3))),
                    "Incomplete PGS display objects or palette"
                );
                let start = self.start.take().context("Missing PGS timestamp")?;
                self.decoder.send_packet(
                    &Packet::new(0, TimeBase::new(1, 1000), std::mem::take(&mut self.set))
                        .with_pts(start as i64),
                )?;
                let Frame::Video(frame) = self.decoder.receive_frame()? else {
                    anyhow::bail!("Expected PGS image")
                };
                let p = frame.planes.first().context("Missing PGS image")?;
                ensure!(
                    p.stride > 0 && p.stride % 4 == 0 && p.data.len() % p.stride == 0,
                    "Invalid PGS image"
                );
                let w = p.stride / 4;
                let h = p.data.len() / p.stride;
                // Crop transparent margins before OCR, keeping multi-object placement.
                let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
                for y in 0..h {
                    for x in 0..w {
                        if p.data[y * p.stride + x * 4 + 3] > 0 {
                            x0 = x0.min(x);
                            y0 = y0.min(y);
                            x1 = x1.max(x + 1);
                            y1 = y1.max(y + 1);
                        }
                    }
                }
                let image = if x1 > x0 && y1 > y0 {
                    let mut gray = Vec::with_capacity((x1 - x0) * (y1 - y0));
                    for y in y0..y1 {
                        for x in x0..x1 {
                            let px = &p.data[y * p.stride + x * 4..][..4];
                            let lum =
                                (px[0] as u32 * 299 + px[1] as u32 * 587 + px[2] as u32 * 114)
                                    / 1000;
                            gray.push((255 - lum * px[3] as u32 / 255) as u8);
                        }
                    }
                    Some((x1 - x0, y1 - y0, gray))
                } else {
                    None
                };
                let end = self.end.take();
                self.display(start, end, image, self.forced, out)?;
            }
        }
        Ok(())
    }
    fn display(
        &mut self,
        start: u64,
        end: Option<u64>,
        image: Option<(usize, usize, Vec<u8>)>,
        forced: bool,
        out: &Path,
    ) -> Result<()> {
        if let Some((previous, limit, w, h, gray, f)) = &mut self.active {
            ensure!(start >= *previous, "Out-of-order PGS display");
            if image
                .as_ref()
                .is_some_and(|(nw, nh, ng)| nw == w && nh == h && ng == gray)
                && *f == forced
                && limit.is_none_or(|e| e >= start)
            {
                *limit = end;
                return Ok(());
            }
        }
        self.close(Some(start), out)?;
        self.active = image.map(|(w, h, g)| (start, end, w, h, g, forced));
        Ok(())
    }
    fn close(&mut self, next: Option<u64>, out: &Path) -> Result<()> {
        if let Some((start, limit, w, h, gray, forced)) = self.active.take() {
            let end = match (limit, next) {
                (Some(a), Some(b)) => a.min(b),
                (Some(a), None) => a,
                (None, Some(b)) => b,
                _ => anyhow::bail!("Final PGS caption has no clear event or duration"),
            };
            if end == start {
                return Ok(());
            }
            ensure!(end > start, "Invalid PGS caption timing");
            self.bytes += gray.len();
            ensure!(
                self.bytes <= 256 * 1024 * 1024 && self.cues.len() < 10000,
                "PGS caption storage limit"
            );
            let name = format!("{:05}.gray", self.cues.len());
            fs::write(out.join(&name), gray)?;
            self.cues.push(json!({"file":name,"width":w,"height":h,"start_ms":start,"end_ms":end,"forced":forced}));
        }
        Ok(())
    }
    pub fn finish(mut self, out: &Path) -> Result<Vec<Value>> {
        ensure!(
            self.set.is_empty() && self.start.is_none(),
            "Incomplete PGS display set"
        );
        self.close(None, out)?;
        // An empty result (no visible display) is reported by the caller.
        Ok(self.cues)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn mkv(sup: &[u8]) -> Vec<Vec<u8>> {
        let mut out = vec![];
        let mut pos = 0;
        while pos < sup.len() {
            let (_, end) = oxideav_sub_image::pgs::read_segment(sup, pos).unwrap();
            out.push(sup[pos + 10..end].to_vec());
            pos = end;
        }
        out
    }
    #[test]
    fn fragmented_display_clear_and_container_duration() -> Result<()> {
        let out = std::env::temp_dir().join(format!("torrbaby-pgs-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&out)?;
        let sup = oxideav_sub_image::pgs::build_demo_display_set(
            (16, 16),
            (2, 1),
            (4, 5),
            &[(1, [255, 255, 255, 255])],
            &[1, 1],
        );
        let segments = mkv(&sup);
        let mut decoder = Pgs::new()?;
        for segment in &segments {
            decoder.push(segment, 1000, Some(1200), &out)?;
        }
        // Explicit clear at 3s must not extend a container duration ending at 2.2s.
        let mut clear = segments[0][..14].to_vec();
        clear[1..3].copy_from_slice(&11u16.to_be_bytes());
        clear[13] = 0;
        decoder.push(&clear, 3000, None, &out)?;
        decoder.push(&[0x80, 0, 0], 3000, None, &out)?;
        let cues = decoder.finish(&out)?;
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0]["start_ms"], 1000);
        assert_eq!(cues[0]["end_ms"], 2200);
        assert_eq!(cues[0]["width"], 2);
        assert_eq!(cues[0]["height"], 1);
        let mut decoder = Pgs::new()?;
        for segment in &segments {
            decoder.push(segment, 4000, None, &out)?;
        }
        assert!(decoder.finish(&out).is_err());
        fs::remove_dir_all(out)?;
        Ok(())
    }
}
