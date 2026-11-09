//! Isolated read-only VobSub extraction probe; never called by the daemon.
use anyhow::{Context, Result, ensure};
use matroska_demuxer::{ContentEncodingValue, Frame, MatroskaFile};
use serde_json::json;
use std::{
    fs,
    io::{BufReader, Read},
    path::Path,
    time::Instant,
};
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    ensure!(args.len() == 4, "input.mkv track output-directory");
    let input = Path::new(&args[1]);
    let track: u64 = args[2].parse()?;
    let out = Path::new(&args[3]);
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
    ensure!(t.codec_id() == "S_VOBSUB", "Expected VobSub");
    let private = std::str::from_utf8(t.codec_private().context("Missing palette")?)?;
    let idx = oxideav_sub_image::vobsub::parse_idx(private)?;
    ensure!(idx.has_palette, "Missing palette");
    let enc = t.content_encodings().unwrap_or(&[]).to_vec();
    let mut cues = Vec::new();
    let mut failures = Vec::new();
    let mut frame = Frame::default();
    while mkv.next_frame(&mut frame)? {
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
        match oxideav_sub_image::vobsub::parse_and_decode_spu(&data) {
            Ok((spu, pixels, (w, h))) => {
                ensure!(
                    !spu.saw_chg_colcon,
                    "Dynamic palette not handled in this probe"
                );
                let mut gray = Vec::with_capacity(pixels.len());
                for pixel in pixels {
                    let p = pixel as usize;
                    let rgb = idx.palette_rgb[spu.palette_sel[p] as usize];
                    let lum =
                        (rgb[0] as u32 * 299 + rgb[1] as u32 * 587 + rgb[2] as u32 * 114) / 1000;
                    gray.push((255 - lum * spu.alpha[p] as u32 / 15) as u8);
                }
                let name = format!("{:05}.gray", cues.len());
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
    let after = fs::metadata(input)?;
    ensure!(
        before.len() == after.len() && before.modified()? == after.modified()?,
        "Source changed"
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
