//! Matroska packet passthrough for persistent virtual MP4 indexes.
use anyhow::{Context, Result, ensure};
use matroska_demuxer::{Frame, MatroskaFile, TrackType};
use oxideav_core::{CodecId, CodecParameters, Packet, StreamInfo, TimeBase};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{BufReader, Read, Seek, SeekFrom, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
    time::{Duration, Instant},
};

struct LimitedFile {
    file: File,
    limit: u64,
    mapping: Option<std::sync::Arc<std::sync::Mutex<crate::virtual_mkv::Recording>>>,
}
impl Write for LimitedFile {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let end = self.file.stream_position()?.checked_add(bytes.len() as u64);
        if end.is_none_or(|end| end > self.limit) {
            return Err(std::io::Error::other("Remux storage limit reached"));
        }
        if let Some(mapping) = &self.mapping {
            let mut record = mapping.lock().map_err(|_| std::io::Error::other("Mapping lock failed"))?;
            if let Some((source, expected)) = record.pending.take() {
                if bytes != expected { return Err(std::io::Error::other("Muxer changed a media packet")); }
                let at = self.file.stream_position()?;
                record.extents.push([at, source, bytes.len() as u64]);
                self.file.seek(SeekFrom::Current(bytes.len() as i64))?;
                return Ok(bytes.len());
            }
        }
        self.file.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}
impl Seek for LimitedFile {
    fn seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
        self.file.seek(p)
    }
}
struct Track {
    stream: StreamInfo,
    duration_ns: u64,
    video: bool,
    count: u64,
    pts: Vec<u64>,
    audio_samples: u64,
    origin_ns: Option<u64>,
    codec_delay_ns: u64,
    visual_boxes: Vec<u8>,
    labels: TrackLabels,
    default_track: bool,
}

pub fn run(input: &Path, output: &Path, max_bytes: u64) -> Result<serde_json::Value> {
    run_internal(input, output, max_bytes, None)
}

pub(crate) fn run_internal(input: &Path, output: &Path, max_bytes: u64,
    mapping: Option<std::sync::Arc<std::sync::Mutex<crate::virtual_mkv::Recording>>>) -> Result<serde_json::Value> {
    ensure!(
        (1_048_576..=34_359_738_368).contains(&max_bytes),
        "Remux budget must be 1 MiB to 32 GiB"
    );
    let before = crate::subtitle_jobs::revision(input)?;
    let source = File::open(input)?;
    ensure!(
        source.metadata()?.is_file(),
        "Expected a regular input file"
    );
    let mut mkv = MatroskaFile::open(BufReader::new(source))?;
    let scale = mkv.info().timestamp_scale().get();
    let mut tracks = BTreeMap::new();
    for t in mkv
        .tracks()
        .iter()
        .filter(|t| matches!(t.track_type(), TrackType::Video | TrackType::Audio))
    {
        ensure!(tracks.len() < 8, "Too many media tracks");
        ensure!(
            t.content_encodings().is_none(),
            "Encoded tracks need additional remux support"
        );
        let video = t.track_type() == TrackType::Video;
        let mut audio_samples = 1024;
        let codec = match t.codec_id() {
            "V_MPEG4/ISO/AVC" => "h264",
            "V_MPEGH/ISO/HEVC" => "h265",
            "A_AAC" => "aac",
            "A_AC3" => "ac3",
            "A_EAC3" => "eac3",
            other => anyhow::bail!("Remux codec not implemented yet: {other}"),
        };
        let mut params = if video {
            CodecParameters::video(CodecId::new(codec))
        } else {
            CodecParameters::audio(CodecId::new(codec))
        };
        params.extradata = if matches!(codec, "ac3" | "eac3") {
            Vec::new()
        } else {
            t.codec_private()
                .context("Missing codec configuration")?
                .to_vec()
        };
        let duration_ns;
        let time_base;
        if video {
            let v = t.video().context("Missing video dimensions")?;
            params.width = Some(v.pixel_width().get().try_into()?);
            params.height = Some(v.pixel_height().get().try_into()?);
            duration_ns = t
                .default_duration()
                .context("Variable frame rate needs additional timing support")?
                .get();
            ensure!(
                (1_000_000..=1_000_000_000).contains(&duration_ns),
                "Invalid video duration"
            );
            time_base = TimeBase::new(1, 1000);
        } else if matches!(codec, "ac3" | "eac3") {
            let a = t.audio().context("Missing audio parameters")?;
            ensure!(
                [32000.0, 44100.0, 48000.0].contains(&a.sampling_frequency()),
                "Unsupported AC-3 sample rate"
            );
            params.sample_rate = Some(a.sampling_frequency() as u32);
            params.channels = Some(a.channels().get().try_into()?);
            duration_ns = 0;
            time_base = TimeBase::new(1, a.sampling_frequency() as i64);
        } else {
            let a = t.audio().context("Missing audio parameters")?;
            let (core_rate, output_rate, channels) = aac_configuration(&params.extradata)?;
            ensure!(a.sampling_frequency() == core_rate as f64
                && a.output_sampling_frequency().is_none_or(|r| r == output_rate as f64)
                && a.channels().get() == channels as u64, "AAC layout mismatch");
            params.sample_rate = Some(output_rate);
            params.channels = Some(channels.into());
            audio_samples = 1024 * output_rate as u64 / core_rate as u64;
            duration_ns = 0;
            time_base = TimeBase::new(1, output_rate as i64);
        }
        let stream = StreamInfo {
            index: tracks.len() as u32,
            time_base,
            duration: None,
            start_time: Some(0),
            params,
        };
        tracks.insert(
            t.track_number().get(),
            Track {
                stream,
                duration_ns,
                video,
                count: 0,
                pts: Vec::new(),
                audio_samples: if matches!(codec, "ac3" | "eac3") {
                    1536
                } else {
                    audio_samples
                },
                origin_ns: None,
                codec_delay_ns: t.codec_delay().unwrap_or(0),
                labels: TrackLabels {
                    language: normalized_language(
                        t.language_bcp47().or(t.language()).unwrap_or("und"),
                    )?,
                    name: t.name().map(str::to_owned),
                    audio: !video,
                    enabled: t.flag_enabled(),
                },
                default_track: t.flag_default(),
                visual_boxes: t
                    .video()
                    .map(crate::video_metadata::boxes)
                    .transpose()?
                    .unwrap_or_default(),
            },
        );
    }
    ensure!(tracks.values().any(|t| t.video), "No supported video track");
    if tracks
        .values()
        .any(|t| matches!(t.stream.params.codec_id.as_str(), "ac3" | "eac3"))
    {
        let started = Instant::now();
        let mut frame = Frame::default();
        let mut scanned = 0;
        while tracks.values().any(|t| {
            matches!(t.stream.params.codec_id.as_str(), "ac3" | "eac3")
                && t.stream.params.extradata.is_empty()
        }) {
            ensure!(
                scanned < 100_000 && started.elapsed() < Duration::from_secs(30),
                "Dolby audio configuration scan limit"
            );
            ensure!(mkv.next_frame(&mut frame)?, "Missing Dolby audio packet");
            scanned += 1;
            ensure!(
                frame.data.len() <= 64 * 1024 * 1024,
                "Media frame too large"
            );
            if let Some(t) = tracks.get_mut(&frame.track) {
                if matches!(t.stream.params.codec_id.as_str(), "ac3" | "eac3")
                    && t.stream.params.extradata.is_empty()
                {
                    let (config, rate, channels) = crate::dolby::configuration(
                        t.stream.params.codec_id.as_str(),
                        &frame.data,
                    )?;
                    ensure!(
                        Some(rate) == t.stream.params.sample_rate
                            && Some(channels) == t.stream.params.channels,
                        "Dolby audio container/header mismatch"
                    );
                    t.stream.params.extradata = config;
                }
            }
        }
        mkv = MatroskaFile::open(BufReader::new(File::open(input)?))?;
    }
    let selected_audio = tracks
        .values()
        .filter(|t| !t.video && t.labels.enabled)
        .min_by_key(|t| (!t.default_track, t.stream.index))
        .map(|t| t.stream.index);
    for t in tracks.values_mut().filter(|t| !t.video) {
        t.labels.enabled = selected_audio == Some(t.stream.index);
    }
    let mut streams: Vec<_> = tracks.values().map(|t| t.stream.clone()).collect();
    streams.sort_by_key(|s| s.index);
    // create_new never replaces source media or an existing destination.
    let out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(output)?;
    let result = (|| -> Result<serde_json::Value> {
        let options = oxideav_mp4::options::Mp4MuxerOptions {
            large_mdat: true,
            write_edit_list: true,
            ..Default::default()
        };
        let mut mux = oxideav_mp4::muxer::open_with_options(
            Box::new(LimitedFile {
                file: out,
                limit: max_bytes,
                mapping: mapping.clone(),
            }),
            &streams,
            options,
        )?;
        mux.write_header()?;
        let started = Instant::now();
        let mut frame = Frame::default();
        while mkv.next_frame(&mut frame)? {
            ensure!(
                started.elapsed() < Duration::from_secs(600),
                "Remux timed out"
            );
            ensure!(
                frame.data.len() <= 64 * 1024 * 1024,
                "Frame exceeds remux limit"
            );
            let Some(t) = tracks.get_mut(&frame.track) else {
                continue;
            };
            ensure!(t.count < 2_000_000, "Too many samples");
            let ns = frame
                .timestamp
                .checked_mul(scale)
                .context("Timestamp overflow")?;
            // Matroska timestamps identify the first frame of a lace. Preserve
            // exact codec cadence for subsequent frames rather than treating
            // their shared block timestamp as duplicated audio samples.
            let lace_offset = if t.video {
                frame.lace_index as u128 * t.duration_ns as u128
            } else {
                frame.lace_index as u128 * t.audio_samples as u128 * 1_000_000_000
                    / t.stream.params.sample_rate.unwrap() as u128
            };
            let ns = ns.checked_add(u64::try_from(lace_offset)?)
                .context("Laced timestamp overflow")?;
            let origin = *t.origin_ns.get_or_insert(ns);
            let start = origin as i128 - t.codec_delay_ns as i128;
            let mut packet = Packet::new(
                t.stream.index,
                t.stream.time_base,
                std::mem::take(&mut frame.data),
            );
            if t.video {
                t.pts.push(ns);
                let dts = (start + t.count as i128 * t.duration_ns as i128).div_euclid(1_000_000);
                let next =
                    (start + (t.count + 1) as i128 * t.duration_ns as i128).div_euclid(1_000_000);
                packet.pts = Some(
                    (ns as i128 - t.codec_delay_ns as i128)
                        .div_euclid(1_000_000)
                        .try_into()?,
                );
                packet.dts = Some(dts.try_into()?);
                packet.duration = Some((next - dts).try_into()?);
            } else {
                if matches!(t.stream.params.codec_id.as_str(), "ac3" | "eac3") {
                    let (config, _, _) = crate::dolby::configuration(
                        t.stream.params.codec_id.as_str(),
                        &packet.data,
                    )?;
                    ensure!(
                        config == t.stream.params.extradata,
                        "Dolby audio configuration changed midstream"
                    );
                }
                let rate = t.stream.params.sample_rate.unwrap() as u128;
                let expected = origin as u128
                    + t.count as u128 * t.audio_samples as u128 * 1_000_000_000 / rate;
                ensure!(
                    expected.abs_diff(ns as u128) <= scale as u128,
                    "Audio offset/gap needs additional timing support"
                );
                let start_samples = (start * rate as i128).div_euclid(1_000_000_000);
                packet.pts =
                    Some((start_samples + (t.count * t.audio_samples) as i128).try_into()?);
                packet.dts = packet.pts;
                packet.duration = Some(t.audio_samples as i64);
            }
            packet.flags.keyframe = !t.video || frame.is_keyframe == Some(true);
            if let Some(mapping) = &mapping {
                let mut record = mapping.lock().map_err(|_| anyhow::anyhow!("Mapping lock failed"))?;
                ensure!(record.pending.is_none(), "Unconsumed media packet");
                record.pending = Some((frame.source_offset, packet.data.clone()));
            }
            mux.write_packet(&packet)?;
            if let Some(mapping) = &mapping {
                ensure!(mapping.lock().map_err(|_| anyhow::anyhow!("Mapping lock failed"))?.pending.is_none(), "Unmapped media packet");
            }
            t.count += 1;
        }
        for t in tracks.values_mut() {
            ensure!(t.count > 0, "Empty media track");
            if t.video {
                t.pts.sort_unstable();
                for (i, pts) in t.pts.iter().enumerate() {
                    let expected = t.origin_ns.unwrap() as u128 + i as u128 * t.duration_ns as u128;
                    ensure!(
                        expected.abs_diff(*pts as u128) <= scale as u128,
                        "Video offset/gap/variable cadence needs additional timing support"
                    );
                }
            }
        }
        ensure!(
            before == crate::subtitle_jobs::revision(input)?,
            "Source changed during remux"
        );
        mux.write_trailer()?;
        drop(mux);
        let durations: BTreeMap<u32, u64> = tracks
            .values()
            .map(|t| {
                let start = t.origin_ns.unwrap() as i128 - t.codec_delay_ns as i128;
                let end_ms = if t.video {
                    (start + t.count as i128 * t.duration_ns as i128).div_euclid(1_000_000)
                } else {
                    let rate = t.stream.params.sample_rate.unwrap() as i128;
                    let samples = (start * rate).div_euclid(1_000_000_000)
                        + (t.count * t.audio_samples) as i128;
                    (samples * 1000).div_euclid(rate)
                };
                (t.stream.index + 1, end_ms.max(0) as u64)
            })
            .collect();
        let visual: BTreeMap<u32, Vec<u8>> = tracks
            .values()
            .filter(|t| t.video)
            .map(|t| (t.stream.index + 1, t.visual_boxes.clone()))
            .collect();
        let labels = tracks
            .values()
            .map(|t| (t.stream.index + 1, t.labels.clone()))
            .collect();
        repair_presentation_durations(output, &durations, &visual, &labels, max_bytes)?;
        Ok(
            serde_json::json!({"tracks":streams.len(),"bytes":output.metadata()?.len(),"development_only":true}),
        )
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(output);
    }
    result
}

// oxideav-mp4 0.0.11 writes media duration into movie/track headers even when
// edit lists offset/trim that media. Update only these fixed-size fields in our
// own finished output; sample tables, compressed packets and edits stay intact.
fn repair_presentation_durations(
    path: &Path,
    durations: &BTreeMap<u32, u64>,
    visual: &BTreeMap<u32, Vec<u8>>,
    labels: &BTreeMap<u32, TrackLabels>,
    max_bytes: u64,
) -> Result<()> {
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let length = file.metadata()?.len();
    let mut offset = 0;
    while offset < length {
        ensure!(length - offset >= 8, "Truncated MP4 box");
        file.seek(SeekFrom::Start(offset))?;
        let mut header = [0u8; 8];
        file.read_exact(&mut header)?;
        let size32 = u32::from_be_bytes(header[..4].try_into()?) as u64;
        let (size, header_size) = if size32 == 1 {
            let mut large = [0; 8];
            file.read_exact(&mut large)?;
            (u64::from_be_bytes(large), 16)
        } else if size32 == 0 {
            (length - offset, 8)
        } else {
            (size32, 8)
        };
        ensure!(
            size >= header_size && size <= length - offset,
            "Invalid MP4 box size"
        );
        if &header[4..] == b"moov" {
            ensure!(
                size - header_size <= 64 * 1024 * 1024,
                "MP4 metadata too large"
            );
            let mut moov = vec![0; (size - header_size) as usize];
            file.read_exact(&mut moov)?;
            let boxes = children(&moov)?;
            let mut movie = false;
            let mut seen = std::collections::BTreeSet::new();
            for (kind, start, end) in boxes {
                if &kind == b"mvhd" {
                    let payload = &mut moov[start..end];
                    let version = *payload.first().context("Missing mvhd version")?;
                    let (scale_pos, duration_pos) = match version {
                        0 => (12, 16),
                        1 => (20, 24),
                        _ => anyhow::bail!("Unsupported mvhd version"),
                    };
                    ensure!(payload.len() >= scale_pos + 4, "Truncated mvhd");
                    ensure!(
                        u32::from_be_bytes(payload[scale_pos..scale_pos + 4].try_into()?) == 1000,
                        "Unexpected movie timescale"
                    );
                    write_duration(
                        payload,
                        duration_pos,
                        version,
                        *durations
                            .values()
                            .max()
                            .context("Missing track durations")?,
                    )?;
                    movie = true;
                } else if &kind == b"trak" {
                    for (child, begin, finish) in children(&moov[start..end])? {
                        if &child != b"tkhd" {
                            continue;
                        }
                        let payload = &mut moov[start + begin..start + finish];
                        let version = *payload.first().context("Missing tkhd version")?;
                        let (id_pos, duration_pos) = match version {
                            0 => (12, 20),
                            1 => (20, 28),
                            _ => anyhow::bail!("Unsupported tkhd version"),
                        };
                        ensure!(payload.len() >= id_pos + 4, "Truncated tkhd");
                        let id = u32::from_be_bytes(payload[id_pos..id_pos + 4].try_into()?);
                        ensure!(seen.insert(id), "Duplicate MP4 track");
                        write_duration(
                            payload,
                            duration_pos,
                            version,
                            *durations.get(&id).context("Unexpected MP4 track")?,
                        )?;
                    }
                }
            }
            ensure!(
                movie && seen.len() == durations.len(),
                "Missing MP4 presentation headers"
            );
            ensure!(
                offset + size == length,
                "Metadata rewrite requires moov at end"
            );
            let moov = rewrite_visual_tracks(&moov, visual, labels)?;
            let boxed = crate::video_metadata::atom(b"moov", &moov)?;
            ensure!(
                offset + boxed.len() as u64 <= max_bytes,
                "Remux metadata exceeded storage limit"
            );
            file.seek(SeekFrom::Start(offset))?;
            file.write_all(&boxed)?;
            file.set_len(offset + boxed.len() as u64)?;
            file.sync_all()?;
            return Ok(());
        }
        offset += size;
    }
    anyhow::bail!("Missing MP4 movie metadata")
}
fn children(data: &[u8]) -> Result<Vec<([u8; 4], usize, usize)>> {
    let mut result = Vec::new();
    let mut offset = 0;
    while offset < data.len() {
        ensure!(
            data.len() - offset >= 8 && result.len() < 4096,
            "Invalid MP4 child boxes"
        );
        let size = u32::from_be_bytes(data[offset..offset + 4].try_into()?) as usize;
        ensure!(
            size >= 8 && size <= data.len() - offset,
            "Invalid MP4 child size"
        );
        result.push((
            data[offset + 4..offset + 8].try_into()?,
            offset + 8,
            offset + size,
        ));
        offset += size;
    }
    Ok(result)
}
fn write_duration(payload: &mut [u8], position: usize, version: u8, value: u64) -> Result<()> {
    let bytes = if version == 0 {
        u32::try_from(value)
            .context("MP4 duration exceeds version-zero range")?
            .to_be_bytes()
            .to_vec()
    } else {
        value.to_be_bytes().to_vec()
    };
    ensure!(
        payload.len() >= position + bytes.len(),
        "Truncated MP4 duration"
    );
    payload[position..position + bytes.len()].copy_from_slice(&bytes);
    Ok(())
}

fn rewrite_visual_tracks(
    moov: &[u8],
    visual: &BTreeMap<u32, Vec<u8>>,
    labels: &BTreeMap<u32, TrackLabels>,
) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for (kind, start, end) in children(moov)? {
        let body = &moov[start..end];
        if &kind == b"trak" {
            let (_, s, e) = children(body)?
                .into_iter()
                .find(|(k, _, _)| k == b"tkhd")
                .context("Missing track header")?;
            let header = &body[s..e];
            let pos = if header[0] == 0 { 12 } else { 20 };
            ensure!(header.len() >= pos + 4, "Truncated track ID");
            let id = u32::from_be_bytes(header[pos..pos + 4].try_into()?);
            let body =
                rewrite_track_labels(body, labels.get(&id).context("Missing track labels")?)?;
            if let Some(extra) = visual.get(&id) {
                out.extend(crate::video_metadata::atom(
                    &kind,
                    &rewrite_visual_container(&body, extra, 0)?,
                )?);
                continue;
            }
            out.extend(crate::video_metadata::atom(&kind, &body)?);
            continue;
        }
        out.extend(crate::video_metadata::atom(&kind, body)?);
    }
    Ok(out)
}
fn rewrite_visual_container(data: &[u8], extra: &[u8], depth: usize) -> Result<Vec<u8>> {
    ensure!(depth < 6, "Unexpected MP4 nesting");
    let mut out = Vec::new();
    for (kind, start, end) in children(data)? {
        let body = &data[start..end];
        let rewritten = if [&b"mdia"[..], &b"minf"[..], &b"stbl"[..]].contains(&kind.as_slice()) {
            rewrite_visual_container(body, extra, depth + 1)?
        } else if &kind == b"stsd" {
            ensure!(
                body.len() >= 8 && u32::from_be_bytes(body[4..8].try_into()?) == 1,
                "Unexpected sample descriptions"
            );
            let entries = children(&body[8..])?;
            ensure!(entries.len() == 1, "Unexpected video sample entry");
            let (codec, s, e) = entries[0];
            ensure!(
                [*b"avc1", *b"hvc1"].contains(&codec),
                "Unexpected video codec"
            );
            let mut entry = body[8 + s..8 + e].to_vec();
            ensure!(entry.len() >= 78, "Truncated video sample entry");
            for (k, _, _) in children(&entry[78..])? {
                ensure!(
                    ![*b"colr", *b"pasp", *b"clli", *b"mdcv"].contains(&k),
                    "Duplicate video metadata"
                );
            }
            entry.extend(extra);
            let mut replacement = body[..8].to_vec();
            replacement.extend(crate::video_metadata::atom(&codec, &entry)?);
            replacement
        } else {
            body.to_vec()
        };
        out.extend(crate::video_metadata::atom(&kind, &rewritten)?);
    }
    Ok(out)
}

#[derive(Clone)]
struct TrackLabels {
    language: String,
    name: Option<String>,
    audio: bool,
    enabled: bool,
}
fn normalized_language(tag: &str) -> Result<String> {
    ensure!(
        tag.len() <= 63
            && !tag.is_empty()
            && tag
                .split('-')
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric())),
        "Invalid track language tag"
    );
    let tag = tag.to_ascii_lowercase();
    Ok(match tag.as_str() {
        "eng" => "en",
        "chi" | "zho" => "zh",
        "jpn" => "ja",
        "fre" | "fra" => "fr",
        "ger" | "deu" => "de",
        "spa" => "es",
        _ => &tag,
    }
    .to_owned())
}
fn packed_language(tag: &str) -> u16 {
    let primary = tag.split('-').next().unwrap_or("und");
    let iso = match primary {
        "en" => "eng",
        "zh" => "zho",
        "ja" => "jpn",
        "fr" => "fra",
        "de" => "deu",
        "es" => "spa",
        "ko" => "kor",
        "it" => "ita",
        "pt" => "por",
        "ru" => "rus",
        "ar" => "ara",
        "hi" => "hin",
        s if s.len() == 3 => s,
        _ => "und",
    };
    if !iso.bytes().all(|b| b.is_ascii_lowercase()) {
        return packed_language("und");
    }
    iso.bytes().fold(0, |n, b| (n << 5) | (b - b'`') as u16)
}
fn rewrite_track_labels(data: &[u8], labels: &TrackLabels) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for (kind, start, end) in children(data)? {
        let mut body = data[start..end].to_vec();
        if &kind == b"tkhd" {
            let group = match body.first() {
                Some(0) => 34,
                Some(1) => 46,
                _ => anyhow::bail!("Unsupported track header"),
            };
            ensure!(body.len() >= group + 2, "Truncated track flags");
            body[3] = (body[3] & !1) | u8::from(labels.enabled);
            body[group..group + 2]
                .copy_from_slice(&(if labels.audio { 1u16 } else { 0u16 }).to_be_bytes());
        } else if &kind == b"mdia" {
            let mut media = Vec::new();
            for (k, s, e) in children(&body)? {
                ensure!(&k != b"elng", "Duplicate language metadata");
                let mut child = body[s..e].to_vec();
                if &k == b"mdhd" {
                    let position = match child.first() {
                        Some(0) => 20,
                        Some(1) => 32,
                        _ => anyhow::bail!("Unsupported media header"),
                    };
                    ensure!(child.len() >= position + 2, "Truncated media language");
                    child[position..position + 2]
                        .copy_from_slice(&packed_language(&labels.language).to_be_bytes());
                } else if &k == b"hdlr" {
                    if let Some(name) = &labels.name {
                        ensure!(
                            child.len() >= 24 && name.len() <= 1024 && !name.contains('\0'),
                            "Invalid track name"
                        );
                        child.truncate(24);
                        child.extend(name.as_bytes());
                        child.push(0);
                    }
                }
                media.extend(crate::video_metadata::atom(&k, &child)?);
            }
            let mut language = vec![0; 4];
            language.extend(labels.language.as_bytes());
            language.push(0);
            media.extend(crate::video_metadata::atom(b"elng", &language)?);
            body = media;
        }
        out.extend(crate::video_metadata::atom(&kind, &body)?);
    }
    Ok(out)
}

// AAC-LC, including backward-compatible SBR signaling. Keep the full ASC
// in esds so the native decoder sees the same configuration as the source.
fn aac_configuration(data: &[u8]) -> Result<(u32, u32, u8)> {
    struct Bits<'a> { bytes: &'a [u8], at: usize }
    impl Bits<'_> {
        fn read(&mut self, count: usize) -> Result<u32> {
            ensure!(self.at + count <= self.bytes.len() * 8, "Truncated AAC configuration");
            let mut value = 0;
            for _ in 0..count { value = (value << 1) | ((self.bytes[self.at / 8] >> (7-self.at%8)) & 1) as u32; self.at += 1; }
            Ok(value)
        }
        fn rate(&mut self) -> Result<u32> {
            const RATES: [u32;13] = [96000,88200,64000,48000,44100,32000,24000,22050,16000,12000,11025,8000,7350];
            RATES.get(self.read(4)? as usize).copied().context("Unsupported AAC sample rate")
        }
    }
    ensure!((2..=8).contains(&data.len()), "AAC configuration needs additional support");
    let mut b = Bits {bytes: data, at: 0};
    ensure!(b.read(5)? == 2, "AAC object type needs additional support");
    let core = b.rate()?;
    let channels = b.read(4)? as u8;
    ensure!((1..=6).contains(&channels), "AAC channel layout needs additional support");
    ensure!(b.read(3)? == 0, "AAC frame length or extension needs additional support");
    let mut output = core;
    if data.len()*8-b.at >= 16 {
        ensure!(b.read(11)? == 0x2b7 && b.read(5)? == 5, "Unsupported AAC sync extension");
        if b.read(1)? == 1 { output = b.rate()?; }
        if data.len()*8-b.at >= 12 {
            ensure!(b.read(11)? == 0x548 && b.read(1)? == 0, "AAC parametric stereo needs additional support");
        }
    }
    ensure!(output == core || output == core*2, "Unsupported AAC SBR ratio");
    while b.at < data.len()*8 { ensure!(b.read(1)? == 0, "Unsupported trailing AAC configuration"); }
    Ok((core,output,channels))
}

#[cfg(test)]
mod aac_tests {
    #[test]
    fn lc_and_sbr_keep_exact_packet_duration() {
        use super::aac_configuration as parse;
        assert_eq!(parse(&[0x11,0x90]).unwrap(), (48000,48000,2));
        assert_eq!(parse(&[0x11,0x90,0x56,0xe5,0x00]).unwrap(), (48000,48000,2));
        assert_eq!(parse(&[0x13,0x30,0x56,0xe5,0x9d,0x48,0x00]).unwrap(), (24000,48000,6));
        assert!(parse(&[0x13,0x30,0x56,0xe5,0x98]).is_ok());
        assert!(parse(&[0x13,0x30,0x56,0xe5]).is_err());
        assert!(parse(&[0x13,0x30,0x56,0xe5,0x9d,0x48,0x80]).is_err());
    }
}
