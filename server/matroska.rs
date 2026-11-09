//! Read-only Matroska subtitle inspection/extraction. No codecs or OCR required.
use crate::subtitles::TimedCue;
use anyhow::{Context, Result, ensure};
use matroska_demuxer::{Frame, MatroskaFile, TrackType};
use serde_json::{Value, json};
use std::{
    fs::File,
    io::BufReader,
    path::Path,
    time::{Duration, Instant},
};

fn open(path: &Path) -> Result<MatroskaFile<BufReader<File>>> {
    let file = File::open(path)?;
    ensure!(file.metadata()?.is_file(), "Expected a regular media file");
    MatroskaFile::open(BufReader::new(file)).context("Read Matroska container")
}

/// Codec inventory and timing statistics only; never emit compressed payloads or dialogue.
pub fn inspect(path: &Path, frame_limit: usize) -> Result<Value> {
    use std::collections::BTreeMap;
    ensure!(frame_limit <= 100_000, "Frame sample limit is 100000");
    let mut file = open(path)?;
    ensure!(file.tracks().len() <= 256, "Too many tracks");
    let scale = file.info().timestamp_scale().get();
    let tracks: Vec<_> = file.tracks().iter().map(|t| json!({
        "track": t.track_number().get(), "kind": format!("{:?}",t.track_type()),
        "codec": t.codec_id(), "enabled":t.flag_enabled(), "default":t.flag_default(),
        "language":t.language_bcp47().or(t.language()),
        "default_duration_ns":t.default_duration().map(|n|n.get()),
        "codec_delay_ns":t.codec_delay(), "codec_private_bytes":t.codec_private().map(|d|d.len()).unwrap_or(0),
        "encoded":t.content_encodings().is_some(),
        "video":t.video().map(|v|json!({"width":v.pixel_width().get(),"height":v.pixel_height().get()})),
        "audio":t.audio().map(|a|json!({"sample_rate":a.sampling_frequency(),"channels":a.channels().get(),"bit_depth":a.bit_depth().map(|n|n.get())}))
    })).collect();
    #[derive(Default, serde::Serialize)]
    struct Timing {
        frames: usize, bytes: u64, first_pts_ns: Option<u64>, last_pts_ns: Option<u64>,
        min_pts_ns: Option<u64>, max_pts_ns: Option<u64>, reordered: usize,
        explicit_duration: usize, keyframes: usize,
    }
    let mut timing: BTreeMap<u64,Timing> = BTreeMap::new();
    let mut presentation_times: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    let mut frame = Frame::default();
    let start = Instant::now();
    let mut sampled = 0;
    while sampled < frame_limit && file.next_frame(&mut frame)? {
        ensure!(start.elapsed() < Duration::from_secs(30), "Timing sample timed out");
        ensure!(frame.data.len() <= 64 * 1024 * 1024, "Media frame too large");
        sampled += 1;
        let pts = frame.timestamp.checked_mul(scale).context("Timestamp overflow")?;
        presentation_times.entry(frame.track).or_default().push(pts);
        let stats = timing.entry(frame.track).or_default();
        if stats.last_pts_ns.is_some_and(|previous| previous > pts) { stats.reordered += 1; }
        stats.frames += 1; stats.bytes += frame.data.len() as u64;
        stats.first_pts_ns.get_or_insert(pts); stats.last_pts_ns = Some(pts);
        stats.min_pts_ns = Some(stats.min_pts_ns.map_or(pts, |p|p.min(pts)));
        stats.max_pts_ns = Some(stats.max_pts_ns.map_or(pts, |p|p.max(pts)));
        stats.explicit_duration += usize::from(frame.duration.is_some());
        stats.keyframes += usize::from(frame.is_keyframe == Some(true));
    }
    // Presentation-order cadence is separate from packet/decode order. Reporting
    // adjacent packet PTS differences would mistake B-frame reordering for gaps.
    let cadence: BTreeMap<_,_> = presentation_times.into_iter().map(|(track, mut times)| {
        times.sort_unstable();
        let mut deltas: BTreeMap<u64,usize> = BTreeMap::new();
        for pair in times.windows(2) { *deltas.entry(pair[1]-pair[0]).or_default() += 1; }
        let duplicate_pts = deltas.remove(&0).unwrap_or(0);
        let default_duration = file.tracks().iter().find(|t|t.track_number().get()==track)
            .and_then(|t|t.default_duration()).map(|n|n.get());
        // Matroska timestamp quantization permits one tick of rounding error.
        let outside_default_cadence = default_duration.map(|duration| deltas.iter()
            .filter(|(delta,_)| delta.abs_diff(duration)>scale).map(|(_,count)|*count).sum::<usize>());
        (track, json!({"duplicate_pts":duplicate_pts,"presentation_delta_ns":deltas,
            "outside_default_cadence":outside_default_cadence}))
    }).collect();
    Ok(json!({"container":"matroska","timestamp_scale_ns":scale,"tracks":tracks,
        "sampled_frames":sampled,"timing":timing,"cadence":cadence}))
}

pub fn tracks(path: &Path) -> Result<Value> {
    let file = open(path)?;
    let tracks: Vec<_> = file
        .tracks()
        .iter()
        .filter(|t| t.track_type() == TrackType::Subtitle)
        .map(|t| {
            json!({"track":t.track_number().get(),"codec":t.codec_id(),
            "name":t.name(),"language":t.language_bcp47().or(t.language()),
            "forced":t.flag_forced(),"hearing_impaired":t.flag_hearing_impaired(),
            "text_extractable":(["S_TEXT/UTF8","S_TEXT/ASS","S_TEXT/SSA","S_TEXT/WEBVTT"].contains(&t.codec_id())),
            "extraction_supported":(["S_TEXT/UTF8","S_TEXT/ASS","S_TEXT/SSA"].contains(&t.codec_id())) && t.content_encodings().is_none(),
            "encoded":t.content_encodings().is_some()})
        })
        .collect();
    let audio: Vec<_> = file.tracks().iter().filter(|t| t.track_type() == TrackType::Audio)
        .map(|t| json!({"track":t.track_number().get(),"codec":t.codec_id(),"language":t.language_bcp47().or(t.language())})).collect();
    Ok(json!({"container":"matroska","tracks":tracks,"audio_tracks":audio}))
}

pub fn extract(path: &Path, track: u64) -> Result<Vec<TimedCue>> {
    let started = Instant::now();
    let mut file = open(path)?;
    let metadata = file
        .tracks()
        .iter()
        .find(|t| t.track_number().get() == track)
        .context("Subtitle track not found")?;
    ensure!(
        metadata.track_type() == TrackType::Subtitle
            && ["S_TEXT/UTF8", "S_TEXT/ASS", "S_TEXT/SSA"].contains(&metadata.codec_id()),
        "This extractor supports Matroska UTF-8, ASS and SSA text subtitles"
    );
    ensure!(
        metadata.content_encodings().is_none(),
        "Encoded subtitle tracks are not supported"
    );
    let styled = metadata.codec_id() != "S_TEXT/UTF8";
    let delay = metadata.codec_delay().unwrap_or(0);
    let scale = file.info().timestamp_scale().get();
    let mut frame = Frame::default();
    let mut cues = Vec::new();
    let mut frames = 0usize;
    let mut text_bytes = 0usize;
    while file.next_frame(&mut frame)? {
        frames += 1;
        ensure!(
            frames <= 5_000_000 && started.elapsed() < Duration::from_secs(180),
            "Subtitle extraction scan limit exceeded"
        );
        if frame.track != track {
            continue;
        }
        ensure!(frame.data.len() <= 16_384, "Subtitle cue too large");
        let text = std::str::from_utf8(&frame.data).context("Invalid UTF-8 subtitle text")?;
        if text.trim().is_empty() {
            continue;
        }
        ensure!(!text.contains('\0'), "Invalid subtitle text");
        let text = if styled {
            ass_text(text)?
        } else {
            crate::subtitles::plain_text(text)
        };
        if text.trim().is_empty() {
            continue;
        }
        let duration = frame
            .duration
            .context("Subtitle cue has no explicit duration")?;
        let start_ns = frame
            .timestamp
            .checked_mul(scale)
            .context("Subtitle timestamp overflow")?;
        let end_ns = frame
            .timestamp
            .checked_add(duration)
            .and_then(|t| t.checked_mul(scale))
            .context("Subtitle timestamp overflow")?;
        let start_ms = start_ns.saturating_sub(delay) / 1_000_000;
        let end_ms = end_ns.saturating_sub(delay) / 1_000_000;
        ensure!(end_ms > start_ms, "Invalid subtitle interval");
        text_bytes += text.len();
        ensure!(
            cues.len() < 100_000 && text_bytes <= 8_388_608,
            "Subtitle track too large"
        );
        cues.push(TimedCue {
            start_ms,
            end_ms,
            text,
        });
    }
    ensure!(!cues.is_empty(), "Subtitle track has no text cues");
    cues.sort_by_key(|cue| cue.start_ms);
    Ok(cues)
}

// Matroska stores ASS/SSA event fields as ReadOrder, Layer, Style, Name,
// MarginL, MarginR, MarginV, Effect, Text. Keep dialogue, not vector drawings.
fn ass_text(packet: &str) -> Result<String> {
    use std::sync::LazyLock;
    static OVERRIDE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\{[^}]*\}").unwrap());
    static DRAWING: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\\p(\d+)").unwrap());
    let text = packet
        .splitn(9, ',')
        .nth(8)
        .context("Invalid ASS subtitle event")?;
    let mut result = String::new();
    let mut drawing = false;
    let mut offset = 0;
    for tag in OVERRIDE.find_iter(text) {
        if !drawing {
            result.push_str(&text[offset..tag.start()]);
        }
        for mode in DRAWING.captures_iter(tag.as_str()) {
            drawing = &mode[1] != "0";
        }
        offset = tag.end();
    }
    if !drawing {
        result.push_str(&text[offset..]);
    }
    Ok(crate::subtitles::plain_text(
        &result
            .replace("\\N", "\n")
            .replace("\\n", "\n")
            .replace("\\h", " "),
    ))
}

/// Isolate parser panics and CPU-heavy scans from the serving process. This is
/// not a memory sandbox: macOS rejects the tested address/data rlimits.
pub async fn extract_worker(path: &Path, track: u64) -> Result<Vec<TimedCue>> {
    use std::{os::unix::fs::MetadataExt, process::Stdio, sync::LazyLock};
    use tokio::io::AsyncReadExt;
    static WORKERS: LazyLock<tokio::sync::Semaphore> =
        LazyLock::new(|| tokio::sync::Semaphore::new(1));
    let _permit = WORKERS.acquire().await?;
    fn revision(path: &Path) -> Result<(u64, u64, u64, i64, i64, i64, i64)> {
        let m = path.metadata()?;
        Ok((
            m.dev(),
            m.ino(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
        ))
    }
    let before = revision(path)?;
    let mut child = tokio::process::Command::new("/bin/sh")
        .args([
            "-c",
            "ulimit -c 0; ulimit -t 180 || exit 1; exec \"$@\"",
            "torrbaby-subtitle-worker",
        ])
        .arg(std::env::current_exe()?)
        .args(["matroska-subtitles", "--input"])
        .arg(path)
        .arg("--track")
        .arg(track.to_string())
        .arg("--json-cues")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("Start subtitle extraction worker")?;
    let stdout = child
        .stdout
        .take()
        .context("Subtitle worker stdout unavailable")?;
    let outcome = tokio::time::timeout(Duration::from_secs(190), async {
        let mut bytes = Vec::new();
        stdout.take(16_777_217).read_to_end(&mut bytes).await?;
        ensure!(
            bytes.len() <= 16_777_216,
            "Subtitle worker response too large"
        );
        ensure!(
            child.wait().await?.success(),
            "Subtitle extraction worker failed"
        );
        let cues: Vec<TimedCue> =
            serde_json::from_slice(&bytes).context("Invalid subtitle worker response")?;
        ensure!(
            !cues.is_empty()
                && cues.len() <= 100_000
                && cues.iter().all(|cue| cue.end_ms > cue.start_ms
                    && !cue.text.trim().is_empty()
                    && cue.text.len() <= 16_384),
            "Invalid extracted cues"
        );
        ensure!(
            cues.windows(2)
                .all(|pair| pair[0].start_ms <= pair[1].start_ms),
            "Extracted cues out of order"
        );
        ensure!(
            before == revision(path)?,
            "Media changed during subtitle extraction"
        );
        Ok(cues)
    })
    .await;
    match outcome {
        Ok(Ok(cues)) => Ok(cues),
        failed => {
            let _ = child.kill().await;
            match failed {
                Ok(Err(error)) => Err(error),
                _ => anyhow::bail!("Subtitle extraction timed out"),
            }
        }
    }
}

/// Inspect embedded tracks outside the HTTP process, with bounded concurrency,
/// CPU time, wall time and output. Bitmap tracks are reported, never OCRed.
pub async fn tracks_worker(path: &Path) -> Result<Value> {
    use std::{process::Stdio, sync::LazyLock};
    use tokio::io::AsyncReadExt;
    static WORKERS: LazyLock<tokio::sync::Semaphore> =
        LazyLock::new(|| tokio::sync::Semaphore::new(2));
    let _permit = WORKERS
        .try_acquire()
        .context("Subtitle inspection is busy; try again shortly")?;
    let mut child = tokio::process::Command::new("/bin/sh")
        .args([
            "-c",
            "ulimit -c 0; ulimit -t 10 || exit 1; exec \"$@\"",
            "torrbaby-subtitle-inspector",
        ])
        .arg(std::env::current_exe()?)
        .args(["matroska-subtitles", "--input"])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("Start subtitle inspection worker")?;
    let stdout = child
        .stdout
        .take()
        .context("Subtitle inspector stdout unavailable")?;
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        let mut bytes = Vec::new();
        stdout.take(1_048_577).read_to_end(&mut bytes).await?;
        ensure!(bytes.len() <= 1_048_576, "Subtitle inventory too large");
        ensure!(child.wait().await?.success(), "Subtitle inspection failed");
        let result: Value = serde_json::from_slice(&bytes)?;
        let tracks = result["tracks"]
            .as_array()
            .context("Invalid subtitle inventory")?;
        ensure!(
            tracks.len() <= 256
                && tracks
                    .iter()
                    .all(|t| t["track"].as_u64().is_some_and(|n| n > 0)),
            "Invalid subtitle inventory"
        );
        Ok(result)
    })
    .await;
    match result {
        Ok(Ok(value)) => Ok(value),
        failed => {
            let _ = child.kill().await;
            match failed {
                Ok(Err(error)) => Err(error),
                _ => anyhow::bail!("Subtitle inspection timed out"),
            }
        }
    }
}
