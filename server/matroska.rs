//! Read-only Matroska subtitle inspection/extraction. No codecs or OCR required.
use crate::subtitles::TimedCue;
use anyhow::{Context, Result, ensure};
use matroska_demuxer::{ContentCompAlgo, ContentEncoding, ContentEncodingValue, Frame, MatroskaFile, TrackType};
use serde_json::{Value, json};
use std::{
    fs::File,
    io::BufReader,
    path::Path,
    time::{Duration, Instant},
};

/// ContentEncodingScope bits: frame contents, and the track's CodecPrivate.
const SCOPE_FRAMES: u64 = 1;
const SCOPE_PRIVATE: u64 = 2;

/// Whether every ContentEncoding of a track can be undone here: zlib or header
/// stripping applied to frames and/or CodecPrivate. Encryption, bzlib, LZO and
/// encodings of other encodings (scope 4) are not supported.
pub(crate) fn encodings_supported(encodings: Option<&[ContentEncoding]>) -> bool {
    encodings.unwrap_or(&[]).iter().all(|e| {
        e.scope() != 0
            && e.scope() & !(SCOPE_FRAMES | SCOPE_PRIVATE) == 0
            && matches!(e.encoding(), ContentEncodingValue::Compression(c)
                if matches!(c.algo(), ContentCompAlgo::Zlib | ContentCompAlgo::Stripping))
    })
}

/// Undo a track's ContentEncodings for one frame (`SCOPE_FRAMES`) or for its
/// CodecPrivate (`SCOPE_PRIVATE`). Muxers apply encodings in ascending
/// ContentEncodingOrder, so they are reversed from the highest order down.
/// Decompressed output is bounded by `limit` bytes.
fn decode_content(encodings: Option<&[ContentEncoding]>, scope: u64, data: &[u8], limit: usize) -> Result<Vec<u8>> {
    use std::io::Read;
    ensure!(encodings_supported(encodings), "Unsupported Matroska content encoding");
    let mut ordered: Vec<_> = encodings.unwrap_or(&[]).iter().filter(|e| e.scope() & scope != 0).collect();
    ordered.sort_by_key(|e| std::cmp::Reverse(e.order()));
    let mut data = data.to_vec();
    for encoding in ordered {
        let ContentEncodingValue::Compression(compression) = encoding.encoding() else {
            anyhow::bail!("Unsupported Matroska content encoding");
        };
        data = match compression.algo() {
            ContentCompAlgo::Zlib => {
                let mut inflated = Vec::new();
                flate2::read::ZlibDecoder::new(data.as_slice())
                    .take(limit as u64 + 1)
                    .read_to_end(&mut inflated)
                    .context("Invalid zlib-compressed Matroska block")?;
                inflated
            }
            // Header stripping removed these leading bytes from every frame.
            ContentCompAlgo::Stripping => [compression.settings().unwrap_or(&[]), &data].concat(),
            _ => anyhow::bail!("Unsupported Matroska content encoding"),
        };
        ensure!(data.len() <= limit, "Decoded Matroska block too large");
    }
    Ok(data)
}

/// Frame payload with the track's content encodings undone.
pub(crate) fn decode_frame(encodings: Option<&[ContentEncoding]>, data: &[u8], limit: usize) -> Result<Vec<u8>> {
    decode_content(encodings, SCOPE_FRAMES, data, limit)
}

/// CodecPrivate with any encodings whose scope covers it undone.
pub(crate) fn decode_private(encodings: Option<&[ContentEncoding]>, data: &[u8], limit: usize) -> Result<Vec<u8>> {
    decode_content(encodings, SCOPE_PRIVATE, data, limit)
}

/// A Matroska file, read only as far as its segment ends. Files passed around on
/// cloud drives often have a few kilobytes of random bytes appended after the video,
/// to make each copy look unique; read as Matroska, those bytes are now and then a
/// malformed element and the whole file was refused. A player stops at the segment's
/// end, and so does this.
pub struct Bounded { file: File, end: u64 }
impl Bounded {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let mut file = File::open(path)?;
        let length = file.metadata()?.len();
        let end = Self::segment_end(&mut file).filter(|end| *end <= length).unwrap_or(length);
        std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(0))?;
        Ok(Self { file, end })
    }
    /// Where the file's one segment ends: after the EBML header, the segment's own stated size.
    fn segment_end(file: &mut File) -> Option<u64> {
        use std::io::{Read, Seek, SeekFrom};
        // An EBML number: its length is the position of the first set bit.
        fn number(file: &mut File, keep_marker: bool) -> Option<(u64, bool)> {
            let mut first = [0u8; 1];
            file.read_exact(&mut first).ok()?;
            let length = first[0].leading_zeros() as usize + 1;
            if length > 8 { return None; }
            let mut value = if keep_marker { first[0] as u64 } else { (first[0] as u64) & ((1u64 << (8 - length)) - 1) };
            let mut ones = !keep_marker && value == (1u64 << (8 - length)) - 1;
            for _ in 1..length {
                let mut next = [0u8; 1];
                file.read_exact(&mut next).ok()?;
                value = (value << 8) | next[0] as u64;
                ones &= next[0] == 0xFF;
            }
            Some((value, ones))
        }
        file.seek(SeekFrom::Start(0)).ok()?;
        let (header, _) = number(file, true)?;
        if header != 0x1A45_DFA3 { return None; }
        let (size, _) = number(file, false)?;
        file.seek(SeekFrom::Current(i64::try_from(size).ok()?)).ok()?;
        let (segment, _) = number(file, true)?;
        if segment != 0x1853_8067 { return None; }
        // A size of all ones means "until the end of the file".
        let (size, unknown) = number(file, false)?;
        if unknown { return None; }
        file.stream_position().ok()?.checked_add(size)
    }
}
impl std::io::Read for Bounded {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let at = std::io::Seek::stream_position(&mut self.file)?;
        let room = self.end.saturating_sub(at).min(buffer.len() as u64) as usize;
        if room == 0 { return Ok(0); }
        self.file.read(&mut buffer[..room])
    }
}
impl std::io::Seek for Bounded {
    fn seek(&mut self, to: std::io::SeekFrom) -> std::io::Result<u64> {
        match to {
            // The end is the segment's, not the file's.
            std::io::SeekFrom::End(offset) => {
                let target = (self.end as i64).checked_add(offset).filter(|t| *t >= 0).ok_or_else(|| std::io::Error::other("Seek before the start"))?;
                self.file.seek(std::io::SeekFrom::Start(target as u64))
            }
            other => self.file.seek(other),
        }
    }
}
fn open(path: &Path) -> Result<MatroskaFile<BufReader<Bounded>>> {
    ensure!(path.metadata()?.is_file(), "Expected a regular media file");
    MatroskaFile::open(BufReader::new(Bounded::open(path)?)).context("Read Matroska container")
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
        nul_terminated_text_frames: usize, interior_nul_text_frames: usize,
    }
    let mut timing: BTreeMap<u64,Timing> = BTreeMap::new();
    let mut presentation_times: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    let mut dolby_configuration: BTreeMap<u64,Value> = BTreeMap::new();
    let mut frame = Frame::default();
    let start = Instant::now();
    let mut sampled = 0;
    while sampled < frame_limit && file.next_frame(&mut frame)? {
        ensure!(start.elapsed() < Duration::from_secs(30), "Timing sample timed out");
        ensure!(frame.data.len() <= 64 * 1024 * 1024, "Media frame too large");
        sampled += 1;
        if !dolby_configuration.contains_key(&frame.track) {
            if let Some(t)=file.tracks().iter().find(|t| t.track_number().get()==frame.track && ["A_AC3","A_EAC3"].contains(&t.codec_id())) {
                let codec=if t.codec_id()=="A_AC3" {"ac3"} else {"eac3"};
                let info=match crate::dolby::configuration(codec,&frame.data) {
                    Ok((config,rate,channels))=>json!({"sample_rate":rate,"channels":channels,"configuration":config}),
                    Err(error)=>json!({"error":error.to_string()}),
                };
                dolby_configuration.insert(frame.track,info);
            }
        }
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
        if file.tracks().iter().any(|t|t.track_number().get()==frame.track && t.codec_id().starts_with("S_TEXT/")) {
            let end=frame.data.iter().rposition(|b|*b!=0).map_or(0,|i|i+1);
            stats.nul_terminated_text_frames+=usize::from(end<frame.data.len());
            stats.interior_nul_text_frames+=usize::from(frame.data[..end].contains(&0));
        }
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
        "sampled_frames":sampled,"timing":timing,"cadence":cadence,"dolby_configuration":dolby_configuration}))
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
            // Text extraction and bitmap OCR both undo zlib and header stripping.
            "extraction_supported":(["S_TEXT/UTF8","S_TEXT/ASS","S_TEXT/SSA"].contains(&t.codec_id())) && encodings_supported(t.content_encodings()),
            "ocr_supported":(["S_HDMV/PGS","S_VOBSUB"].contains(&t.codec_id())) && encodings_supported(t.content_encodings()),
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
        encodings_supported(metadata.content_encodings()),
        "This subtitle track's encryption or compression is not supported"
    );
    let encodings = metadata.content_encodings().map(<[_]>::to_vec);
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
        let data = decode_frame(encodings.as_deref(), &frame.data, 16_384)?;
        // Some muxers retain C-string terminators in UTF-8/ASS packets.
        // Strip only trailing padding; an interior NUL remains invalid.
        let text = std::str::from_utf8(&data).context("Invalid UTF-8 subtitle text")?
            .trim_end_matches('\0');
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
        LazyLock::new(|| regex::Regex::new(r"\{[^}]*\}").expect("valid regex"));
    static DRAWING: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\\p(\d+)").expect("valid regex"));
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
    // Matches the processing queue's maximum; one serialized extractor stalled it.
    static WORKERS: LazyLock<tokio::sync::Semaphore> =
        LazyLock::new(|| tokio::sync::Semaphore::new(8));
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
        LazyLock::new(|| tokio::sync::Semaphore::new(8));
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

/// Minimal Matroska writer for subtitle tracks with content encodings (tests only).
#[cfg(test)]
mod bounded_tests {
    use super::*;
    use std::io::{Read, Seek, SeekFrom};
    #[test]
    fn bytes_after_the_segment_are_never_read() -> Result<()> {
        // An EBML header (4 bytes of body), a segment of 6 bytes, then junk a sharer appended.
        let mut bytes = vec![0x1A, 0x45, 0xDF, 0xA3, 0x84, 1, 2, 3, 4, 0x18, 0x53, 0x80, 0x67, 0x86, 9, 9, 9, 9, 9, 9];
        let end = bytes.len() as u64;
        bytes.extend([0xEE; 40]);
        let path = std::env::temp_dir().join(format!("torrbaby-bounded-{}.mkv", uuid::Uuid::new_v4()));
        std::fs::write(&path, &bytes)?;
        let mut reader = Bounded::open(&path)?;
        let mut all = Vec::new();
        reader.read_to_end(&mut all)?;
        assert_eq!(all.len() as u64, end);
        assert_eq!(reader.seek(SeekFrom::End(0))?, end);
        assert_eq!(reader.seek(SeekFrom::End(-2))?, end - 2);
        let mut rest = Vec::new();
        reader.read_to_end(&mut rest)?;
        assert_eq!(rest, [9, 9]);
        // A segment of unknown size runs to the end of the file, as before.
        let open_ended = [0x1A, 0x45, 0xDF, 0xA3, 0x80, 0x18, 0x53, 0x80, 0x67, 0xFF, 7, 7, 7];
        std::fs::write(&path, open_ended)?;
        let mut whole = Vec::new();
        Bounded::open(&path)?.read_to_end(&mut whole)?;
        assert_eq!(whole.len(), open_ended.len());
        std::fs::remove_file(&path)?;
        Ok(())
    }
}
#[cfg(test)]
pub(crate) mod fixture {
    pub fn element(id: u32, body: &[u8]) -> Vec<u8> {
        let mut out: Vec<u8> = id.to_be_bytes().into_iter().skip_while(|b| *b == 0).collect();
        out.push(0x01);
        out.extend_from_slice(&(body.len() as u64).to_be_bytes()[1..]);
        out.extend_from_slice(body);
        out
    }
    pub fn uint(id: u32, value: u64) -> Vec<u8> {
        element(id, &value.to_be_bytes())
    }
    pub fn zlib(data: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }
    /// ContentEncodings with one compression: algo 0 zlib, 3 header stripping.
    pub fn compression(algo: u64, settings: Option<&[u8]>, scope: u64) -> Vec<u8> {
        let compression = [uint(0x4254, algo), settings.map(|s| element(0x4255, s)).unwrap_or_default()].concat();
        element(0x6D80, &element(0x6240, &[uint(0x5031, 0), uint(0x5032, scope), uint(0x5033, 0), element(0x5034, &compression)].concat()))
    }
    pub struct Track<'a> {
        pub number: u64,
        pub codec: &'a str,
        pub language: &'a str,
        pub encodings: Vec<u8>,
        pub private: Option<Vec<u8>>,
        /// (start ms, duration ms, stored payload) in timestamp order.
        pub blocks: Vec<(u64, Option<u64>, Vec<u8>)>,
    }
    pub fn subtitles(tracks: &[Track]) -> Vec<u8> {
        let header = [
            uint(0x4286, 1), uint(0x42F7, 1), uint(0x42F2, 4), uint(0x42F3, 8),
            element(0x4282, b"matroska"), uint(0x4287, 4), uint(0x4285, 2),
        ].concat();
        let info = element(0x1549A966, &[uint(0x2AD7B1, 1_000_000), element(0x4D80, b"t"), element(0x5741, b"t")].concat());
        let entries: Vec<u8> = tracks.iter().flat_map(|t| element(0xAE, &[
            uint(0xD7, t.number), uint(0x73C5, t.number), uint(0x83, 0x11), element(0x86, t.codec.as_bytes()),
            element(0x22B59C, t.language.as_bytes()), t.encodings.clone(),
            t.private.as_ref().map(|p| element(0x63A2, p)).unwrap_or_default(),
        ].concat())).collect();
        let mut events: Vec<(u64, Vec<u8>)> = tracks.iter().flat_map(|t| t.blocks.iter().map(|(ms, duration, payload)| {
            let block = element(0xA1, &[&[0x80 | t.number as u8][..], &(*ms as i16).to_be_bytes(), &[0], payload].concat());
            (*ms, element(0xA0, &[block, duration.map(|d| uint(0x9B, d)).unwrap_or_default()].concat()))
        })).collect();
        events.sort_by_key(|(ms, _)| *ms);
        let mut cluster = uint(0xE7, 0);
        for (_, group) in events { cluster.extend(group); }
        let segment = [info, element(0x1654AE6B, &entries), element(0x1F43B675, &cluster)].concat();
        [element(0x1A45DFA3, &header), element(0x18538067, &segment)].concat()
    }
}

#[cfg(test)]
mod encoding_tests {
    use super::fixture::*;
    use super::*;

    #[test]
    fn compressed_and_header_stripped_text_tracks_extract() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("torrbaby-mkv-encoding-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir)?;
        let input = dir.join("subtitles.mkv");
        let text = |number, encodings, blocks| Track { number, codec: "S_TEXT/UTF8", language: "eng", encodings, private: None, blocks };
        std::fs::write(&input, subtitles(&[
            text(1, compression(0, None, 1), vec![(1000, Some(1500), zlib(b"Hello from zlib")), (3000, Some(500), zlib("你好".as_bytes()))]),
            // Header stripping: "Hello " was removed from every frame and stored once.
            text(2, compression(3, Some(b"Hello "), 1), vec![(1000, Some(1000), b"stripped world".to_vec())]),
            // bzlib is not supported and must say so rather than produce garbage.
            text(3, compression(1, None, 1), vec![(1000, Some(1000), b"BZh".to_vec())]),
        ]))?;
        let inventory = tracks(&input)?;
        let supported: Vec<_> = inventory["tracks"].as_array().unwrap().iter()
            .map(|t| (t["encoded"].as_bool().unwrap(), t["extraction_supported"].as_bool().unwrap())).collect();
        assert_eq!(supported, [(true, true), (true, true), (true, false)]);
        let cues = extract(&input, 1)?;
        assert_eq!(cues.iter().map(|c| (c.start_ms, c.end_ms, c.text.as_str())).collect::<Vec<_>>(),
            [(1000, 2500, "Hello from zlib"), (3000, 3500, "你好")]);
        assert_eq!(extract(&input, 2)?[0].text, "Hello stripped world");
        assert!(extract(&input, 3).is_err());
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn decoded_blocks_are_bounded_and_corrupt_data_fails() {
        // Corrupt zlib data fails instead of passing compressed bytes through.
        let dir = std::env::temp_dir().join(format!("torrbaby-mkv-bound-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let input = dir.join("subtitles.mkv");
        std::fs::write(&input, subtitles(&[
            Track { number: 1, codec: "S_TEXT/UTF8", language: "eng", encodings: compression(0, None, 1), private: None,
                blocks: vec![(0, Some(100), b"not zlib".to_vec())] },
        ])).unwrap();
        let file = open(&input).unwrap();
        let encodings = file.tracks()[0].content_encodings().map(<[_]>::to_vec);
        assert!(decode_frame(encodings.as_deref(), b"not zlib", 1024).is_err());
        assert!(decode_frame(encodings.as_deref(), &zlib(&[0; 2048]), 1024).is_err());
        assert_eq!(decode_frame(encodings.as_deref(), &zlib(b"abc"), 1024).unwrap(), b"abc");
        // CodecPrivate is untouched when the encoding scope covers frames only.
        assert_eq!(decode_private(encodings.as_deref(), b"raw", 1024).unwrap(), b"raw");
        assert!(extract(&input, 1).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
