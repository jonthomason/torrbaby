//! Isolated bitmap decoding and bounded, cached Apple Vision fallback.
use anyhow::{Context, Result, ensure};
use matroska_demuxer::{ContentEncodingValue, Frame, MatroskaFile};
use serde_json::json;
use std::{
    fs,
    io::{BufReader, Read},
    path::Path,
    time::Instant,
};
/// Render 2-bit VobSub pixels as dark-on-white grayscale for OCR.
///
/// SET_COLOR/SET_CONTR store nibbles as `emphasis2 emphasis1 pattern background`
/// (the order libavcodec's dvdsubdec uses). oxideav-sub-image 0.0.9 stores those
/// nibbles in wire order but labels index 0 as background, so a raw pixel value
/// `p` must be looked up at `3 - p`. Without this, the common white-fill/black-
/// outline layout rendered only scattered outline pixels and OCR found no text.
fn vobsub_gray(
    pixels: &[u8],
    palette_sel: &[u8; 4],
    alpha: &[u8; 4],
    palette: &[[u8; 3]; 16],
) -> Vec<u8> {
    pixels
        .iter()
        .map(|&pixel| {
            let slot = 3 - (pixel as usize & 3);
            let rgb = palette[palette_sel[slot] as usize & 15];
            let lum = (rgb[0] as u32 * 299 + rgb[1] as u32 * 587 + rgb[2] as u32 * 114) / 1000;
            (255 - lum * (alpha[slot] as u32 & 15) / 15) as u8
        })
        .collect()
}

pub fn decode(input: &Path, track: u64, out: &Path) -> Result<()> {
    fs::create_dir(out)?;
    let started = Instant::now();
    let before = fs::metadata(input)?;
    let mut mkv = MatroskaFile::open(BufReader::new(fs::File::open(input)?))?;
    let scale = mkv.info().timestamp_scale().get();
    let t = mkv
        .tracks()
        .iter()
        .find(|t| t.track_number().get() == track)
        .context("Track missing")?;
    let is_pgs = t.codec_id() == "S_HDMV/PGS";
    ensure!(
        is_pgs || t.codec_id() == "S_VOBSUB",
        "Unsupported bitmap subtitle codec"
    );
    let idx = if is_pgs {
        None
    } else {
        let private = std::str::from_utf8(t.codec_private().context("Missing palette")?)?;
        let idx = oxideav_sub_image::vobsub::parse_idx(private)?;
        ensure!(idx.has_palette, "Missing palette");
        Some(idx)
    };
    let mut pgs = if is_pgs {
        Some(crate::pgs_subtitles::Pgs::new()?)
    } else {
        None
    };
    let enc = t.content_encodings().unwrap_or(&[]).to_vec();
    let mut cues = Vec::new();
    let mut failures = Vec::new();
    let mut frame = Frame::default();
    let mut bytes_written = 0u64;
    while mkv.next_frame(&mut frame)? {
        ensure!(started.elapsed().as_secs() < 180, "Bitmap scan timed out");
        ensure!(
            frame.data.len() <= 64 * 1024 * 1024,
            "Media packet too large"
        );
        if frame.track != track {
            continue;
        }
        ensure!(cues.len() < 10000, "Cue limit");
        let mut data = frame.data.clone();
        for e in enc.iter().rev() {
            ensure!(e.scope() == 1, "Unsupported encoding scope");
            match e.encoding() {
                ContentEncodingValue::Compression(c) => match c.algo() {
                    matroska_demuxer::ContentCompAlgo::Zlib => {
                        let mut v = Vec::new();
                        flate2::read::ZlibDecoder::new(data.as_slice())
                            .take(1_048_577)
                            .read_to_end(&mut v)?;
                        ensure!(v.len() <= 1_048_576, "Large SPU");
                        data = v;
                    }
                    _ => anyhow::bail!("Unsupported compression {:?}", c.algo()),
                },
                _ => anyhow::bail!("Encrypted subtitle"),
            }
        }
        if let Some(pgs) = &mut pgs {
            pgs.push(
                &data,
                (frame.timestamp as u128 * scale as u128 / 1_000_000) as u64,
                frame
                    .duration
                    .map(|d| (d as u128 * scale as u128 / 1_000_000) as u64),
                out,
            )?;
            continue;
        }
        let idx = idx.as_ref().context("Missing VobSub palette")?;
        match oxideav_sub_image::vobsub::parse_and_decode_spu(&data) {
            Ok((spu, pixels, (w, h))) => {
                ensure!(
                    w > 0 && h > 0 && w <= 4096 && h <= 2160,
                    "Subtitle image too large"
                );
                ensure!(
                    !spu.saw_chg_colcon,
                    "Dynamic subtitle palettes are not supported"
                );
                let gray = vobsub_gray(&pixels, &spu.palette_sel, &spu.alpha, &idx.palette_rgb);
                let name = format!("{:05}.gray", cues.len());
                bytes_written += gray.len() as u64;
                ensure!(
                    bytes_written <= 256 * 1024 * 1024,
                    "Subtitle image storage limit"
                );
                fs::write(out.join(&name), gray)?;
                let base = frame.timestamp as u128 * scale as u128 / 1_000_000;
                let start = base + spu.start_delay_raw as u128 * 1024 * 1000 / 90000;
                let end = if spu.stop_delay_raw > spu.start_delay_raw {
                    Some(base + spu.stop_delay_raw as u128 * 1024 * 1000 / 90000)
                } else {
                    frame
                        .duration
                        .map(|d| base + d as u128 * scale as u128 / 1_000_000)
                };
                cues.push(json!({"file":name,"width":w,"height":h,"start_ms":start as u64,"end_ms":end.map(|e|e as u64),"forced":spu.forced_display}));
            }
            Err(e) => failures.push(json!({"timestamp":frame.timestamp,"error":e.to_string()})),
        }
    }
    if let Some(pgs) = pgs {
        cues = pgs.finish(out)?;
    }
    let after = fs::metadata(input)?;
    ensure!(
        before.len() == after.len() && before.modified()? == after.modified()?,
        "Source changed"
    );
    ensure!(
        failures.is_empty(),
        "Some subtitle images could not be decoded"
    );
    let manifest = json!({"cues":cues,"failures":failures,"extraction_seconds":started.elapsed().as_secs_f64()});
    fs::write(
        out.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    println!(
        "{}",
        json!({"cues":manifest["cues"].as_array().unwrap().len(),"failures":manifest["failures"].as_array().unwrap().len(),"seconds":manifest["extraction_seconds"]})
    );
    Ok(())
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Output {
    pub language: String,
    pub method: String,
    #[serde(rename = "unreadableCues")]
    pub unreadable_cues: usize,
    pub cues: Vec<crate::subtitles::TimedCue>,
}
fn checked(path: &Path) -> Result<Output> {
    ensure!(
        fs::metadata(path)?.len() <= 16_777_216,
        "OCR result too large"
    );
    let result: Output = serde_json::from_slice(&fs::read(path)?)?;
    ensure!(
        ["en", "zh-Hans"].contains(&result.language.as_str()) && result.method == "ocr",
        "Invalid OCR language"
    );
    ensure!(
        !result.cues.is_empty()
            && result.cues.len() <= 10000
            && result.cues.iter().all(|c| c.end_ms > c.start_ms
                && !c.text.trim().is_empty()
                && c.text.len() <= 16384
                && !c.text.contains('\0')),
        "Invalid OCR captions"
    );
    ensure!(
        result
            .cues
            .windows(2)
            .all(|p| p[0].start_ms <= p[1].start_ms),
        "Unordered OCR captions"
    );
    Ok(result)
}
pub fn cleanup(root: &Path) -> Result<()> {
    let cache = root.join("ocr-v1");
    fs::create_dir_all(&cache)?;
    for entry in fs::read_dir(cache)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with("building-")
            && entry.file_type()?.is_dir()
        {
            fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}
async fn run(program: &Path, args: &[&std::ffi::OsStr], seconds: u64) -> Result<()> {
    use std::process::Stdio;
    let mut child = tokio::process::Command::new("/bin/sh")
        .args([
            "-c",
            "ulimit -c 0; ulimit -t 600 || exit 1; exec \"$@\"",
            "torrbaby-ocr-worker",
        ])
        .arg(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("Start OCR worker")?;
    let status = tokio::time::timeout(std::time::Duration::from_secs(seconds), child.wait())
        .await
        .context("OCR worker timed out")??;
    ensure!(
        status.success(),
        "OCR could not read these subtitles; source preserved"
    );
    Ok(())
}
pub async fn extract(
    input: &Path,
    track: u64,
    hint: &str,
    root: &Path,
    progress: &Path,
) -> Result<Output> {
    use sha2::{Digest, Sha256};
    use std::ffi::OsStr;
    let revision = crate::subtitle_jobs::revision(input)?;
    let key = Sha256::digest(serde_json::to_vec(&(1, input, &revision, track, hint))?)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let cache = root.join("ocr-v1");
    fs::create_dir_all(&cache)?;
    let final_path = cache.join(format!("{key}.json"));
    if final_path.exists() {
        return checked(&final_path);
    }
    let work = cache.join(format!("building-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&work)?;
    let _ = fs::remove_file(progress);
    let result = async {
        let exe = std::env::current_exe()?;
        let images = work.join("images");
        let number = track.to_string();
        run(
            &exe,
            &[
                OsStr::new("ocr-subtitles"),
                OsStr::new("--input"),
                input.as_os_str(),
                OsStr::new("--track"),
                OsStr::new(&number),
                OsStr::new("--output"),
                images.as_os_str(),
            ],
            200,
        )
        .await?;
        let output = work.join("result.json");
        run(
            &exe.with_file_name("torrbaby-subtitle-ocr"),
            &[
                images.as_os_str(),
                output.as_os_str(),
                OsStr::new(hint),
                progress.as_os_str(),
            ],
            600,
        )
        .await?;
        let parsed = checked(&output)?;
        ensure!(
            crate::subtitle_jobs::revision(input)? == revision,
            "Media changed during OCR"
        );
        let mut entries = Vec::new();
        let mut bytes = 0u64;
        for entry in fs::read_dir(&cache)? {
            let e = entry?;
            if e.file_type()?.is_file() && e.path().extension().is_some_and(|x| x == "json") {
                let m = e.metadata()?;
                bytes += m.len();
                entries.push((m.modified()?, e.path(), m.len()));
            }
        }
        entries.sort();
        let incoming = fs::metadata(&output)?.len();
        for (_, path, size) in entries {
            if bytes + incoming <= 128 * 1024 * 1024 {
                break;
            }
            fs::remove_file(path)?;
            bytes = bytes.saturating_sub(size);
        }
        ensure!(bytes + incoming <= 128 * 1024 * 1024, "OCR cache is full");
        fs::rename(output, &final_path)?;
        Ok(parsed)
    }
    .await;
    let _ = fs::remove_dir_all(work);
    let _ = fs::remove_file(progress);
    result
}

pub async fn detect_language(cues: &[crate::subtitles::TimedCue]) -> Result<String> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;
    let text = cues
        .iter()
        .map(|c| c.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let sample = text.chars().take(8000).collect::<String>();
    let mut child = tokio::process::Command::new(
        std::env::current_exe()?.with_file_name("torrbaby-subtitle-extractor"),
    )
    .arg("--detect-language")
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .kill_on_drop(true)
    .spawn()?;
    child
        .stdin
        .take()
        .context("Missing language input")?
        .write_all(sample.as_bytes())
        .await?;
    let output = tokio::time::timeout(std::time::Duration::from_secs(15), child.wait_with_output())
        .await
        .context("Language detection timed out")??;
    ensure!(
        output.status.success() && output.stdout.len() < 1024,
        "Language detection failed"
    );
    let tag: Option<String> = serde_json::from_slice(&output.stdout)?;
    match tag.as_deref() {
        Some("en") => Ok("en".into()),
        Some(t) if t.starts_with("zh") => Ok("zh-Hans".into()),
        _ => anyhow::bail!("No supported subtitle language could be identified"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One 2x1 SPU: a pattern pixel then a background pixel, with
    /// SET_COLOR/SET_CONTR in DVD wire order (e2 e1 / p b).
    fn spu() -> Vec<u8> {
        let mut s = vec![0, 0, 0, 5, 0x54];
        s.extend([0, 0, 0, 5, 0x01]);
        s.extend([0x03, 0x00, 0x10]); // pattern=palette 1, others palette 0
        s.extend([0x04, 0xFF, 0xF0]); // background transparent, others opaque
        s.extend([0x05, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00]);
        s.extend([0x06, 0x00, 0x04, 0x00, 0x00, 0xFF]);
        let len = s.len() as u16;
        s[..2].copy_from_slice(&len.to_be_bytes());
        s
    }

    #[test]
    fn vobsub_pattern_pixels_use_pattern_colour() {
        let (spu, pixels, size) = oxideav_sub_image::vobsub::parse_and_decode_spu(&spu()).unwrap();
        assert_eq!(size, (2, 1));
        assert_eq!(pixels, [1, 0]);
        let mut palette = [[0u8; 3]; 16];
        palette[1] = [0xf0, 0xf0, 0xf0];
        // Light pattern text becomes dark ink; the transparent background stays white.
        assert_eq!(
            vobsub_gray(&pixels, &spu.palette_sel, &spu.alpha, &palette),
            [15, 255]
        );
    }
}
