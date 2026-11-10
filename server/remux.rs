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
    // The newest video packet is held so the final sample's decode duration
    // can cover any presentation slots missing from the source.
    held: Option<(Packet, u64)>,
    // Per video sample in decode order: first VCL NAL unit type, or
    // UNKNOWN_NAL, plus CONTAINER_KEY when Matroska marked a keyframe.
    nal_kinds: Vec<u8>,
    nal_length: usize,
}

const UNKNOWN_NAL: u8 = 0x7f;
const CONTAINER_KEY: u8 = 0x80;

/// First VCL NAL unit type of a length-prefixed AVC/HEVC access unit.
fn first_vcl_type(data: &[u8], length_size: usize, hevc: bool) -> u8 {
    let mut at = 0usize;
    while length_size > 0 && at + length_size < data.len() {
        let length = data[at..at + length_size].iter().fold(0usize, |n, b| n << 8 | *b as usize);
        if length == 0 || length > data.len() - at - length_size {
            break;
        }
        let header = data[at + length_size];
        let kind = if hevc { (header >> 1) & 0x3f } else { header & 0x1f };
        if (hevc && kind < 32) || (!hevc && (1..=5).contains(&kind)) {
            return kind;
        }
        match at.checked_add(length_size + length) {
            Some(next) if length > 0 => at = next,
            _ => break,
        }
    }
    UNKNOWN_NAL
}

/// MP4 random-access tables for one video track: 1-based decode-order sample
/// numbers. `full` goes to stss; `partial` goes to QuickTime stps.
#[derive(Debug, Default, PartialEq)]
struct RandomAccess {
    full: Vec<u32>,
    partial: Vec<u32>,
    /// Open AVC keyframes left as ordinary samples.
    demoted: usize,
}

/// Classify random-access points. An HEVC CRA/BLA followed by RASL pictures
/// is an open-GOP point: those leading pictures reference the previous GOP.
/// In stss, AVFoundation starts decoding there and fails with -11821/-12911
/// for times inside the leading window; left out of stss entirely, distant
/// seeks fail with -12431 when every later keyframe is a CRA. Native checks
/// show AVFoundation honours QuickTime stps (partial sync) for these: every
/// frame resolves at normal seek cost.
///
/// An AVC non-IDR keyframe followed by pictures presented before it is also
/// open, but AVC has no RASL marking: VideoToolbox decodes those leading
/// pictures, and later frames may reference them, so starting there fails in
/// either table. Such keyframes are ordinary samples; decoding starts from the
/// previous closed keyframe or IDR.
fn random_access(kinds: &[u8], decode_pts: &[u64], hevc: bool) -> RandomAccess {
    let mut access = RandomAccess::default();
    let known = kinds.iter().any(|k| k & !CONTAINER_KEY != UNKNOWN_NAL);
    for (i, &kind) in kinds.iter().enumerate() {
        let nal = kind & !CONTAINER_KEY;
        let number = i as u32 + 1;
        if hevc && known {
            if !(16..=23).contains(&nal) {
                continue;
            }
            // Leading pictures directly follow their IRAP in decode order.
            let rasl = kinds[i + 1..]
                .iter()
                .map(|k| k & !CONTAINER_KEY)
                .take_while(|n| (6..=9).contains(n))
                .any(|n| n == 8 || n == 9);
            if rasl { access.partial.push(number) } else { access.full.push(number) }
        } else if kind & CONTAINER_KEY != 0 {
            let open = !hevc && nal != 5 && nal != UNKNOWN_NAL && kinds[i + 1..]
                .iter()
                .zip(&decode_pts[i + 1..])
                .take_while(|(k, _)| *k & CONTAINER_KEY == 0)
                .any(|(_, pts)| *pts < decode_pts[i]);
            if !open || access.full.is_empty() { access.full.push(number) } else { access.demoted += 1 }
        }
    }
    // Playback must be able to begin somewhere: promote the first point.
    if access.full.is_empty() && !access.partial.is_empty() {
        access.full.push(access.partial.remove(0));
    }
    access
}

struct VisualRewrite {
    boxes: Vec<u8>,
    access: RandomAccess,
}

fn sample_table(kind: &[u8; 4], numbers: &[u32]) -> Result<Vec<u8>> {
    let mut body = vec![0u8; 4];
    body.extend(u32::try_from(numbers.len())?.to_be_bytes());
    for n in numbers {
        body.extend(n.to_be_bytes());
    }
    crate::video_metadata::atom(kind, &body)
}

/// Matroska timestamps are quantized to TimestampScale and some muxers round
/// twice, so container times jitter around exact codec cadence (2 ms has been
/// observed in E-AC-3 tracks with no missing audio). A quarter frame still
/// assigns every packet to exactly one cadence slot: missing or duplicated
/// frames and cumulative drift remain rejected because each packet is compared
/// with the absolute cadence from the track origin, never with its neighbour.
fn cadence_tolerance_ns(frame_ns: u128, scale: u64) -> u128 {
    (frame_ns / 4).max(scale as u128)
}

/// Validate presentation-ordered video times against a constant cadence and
/// return the number of cadence slots spanned. Every frame must sit on its own
/// slot; empty slots are frames absent from the source (for example dropped
/// leading pictures) and are presented by holding the previous frame, exactly
/// as the source timestamps describe.
fn video_cadence_slots(sorted_pts: &[u64], origin: u64, duration_ns: u64, scale: u64) -> Result<u64> {
    let tolerance = cadence_tolerance_ns(duration_ns as u128, scale) as i128;
    let duration = duration_ns as i128;
    let mut next_slot = 0i128;
    for &pts in sorted_pts {
        let offset = pts as i128 - origin as i128;
        let slot = (offset + duration / 2).div_euclid(duration);
        ensure!(
            slot >= next_slot && (offset - slot * duration).abs() <= tolerance,
            "Video offset/gap/variable cadence needs additional timing support"
        );
        next_slot = slot + 1;
    }
    Ok(next_slot.try_into()?)
}

type Mapping = Option<std::sync::Arc<std::sync::Mutex<crate::virtual_mkv::Recording>>>;

fn write_mapped<M: oxideav_core::Muxer + ?Sized>(
    mux: &mut M,
    mapping: &Mapping,
    packet: &Packet,
    source_offset: u64,
) -> Result<()> {
    if let Some(mapping) = mapping {
        let mut record = mapping.lock().map_err(|_| anyhow::anyhow!("Mapping lock failed"))?;
        ensure!(record.pending.is_none(), "Unconsumed media packet");
        record.pending = Some((source_offset, packet.data.clone()));
    }
    mux.write_packet(packet)?;
    if let Some(mapping) = mapping {
        ensure!(mapping.lock().map_err(|_| anyhow::anyhow!("Mapping lock failed"))?.pending.is_none(), "Unmapped media packet");
    }
    Ok(())
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
    // Audio formats this passthrough cannot carry (for example DTS, which
    // Apple platforms cannot decode) are left out rather than failing the
    // whole title. Callers decide whether a title left with no audio track is
    // acceptable (video-only analysis) or not (playback).
    let mut omitted_audio = Vec::new();
    for t in mkv
        .tracks()
        .iter()
        .filter(|t| matches!(t.track_type(), TrackType::Video | TrackType::Audio))
    {
        if t.track_type() == TrackType::Audio
            && !matches!(t.codec_id(), "A_AAC" | "A_AC3" | "A_EAC3")
        {
            omitted_audio.push(t.codec_id().to_owned());
            continue;
        }
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
        // hvcC byte 21 / avcC byte 4 carry lengthSizeMinusOne.
        let nal_length = match codec {
            "h265" => params.extradata.get(21).map_or(0, |b| (b & 3) as usize + 1),
            "h264" => params.extradata.get(4).map_or(0, |b| (b & 3) as usize + 1),
            _ => 0,
        };
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
                held: None,
                nal_kinds: Vec::new(),
                // hvcC byte 21 / avcC byte 4: lengthSizeMinusOne.
                nal_length,
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
                let hevc = t.stream.params.codec_id.as_str() == "h265";
                let key = if frame.is_keyframe == Some(true) { CONTAINER_KEY } else { 0 };
                t.nal_kinds.push(key | first_vcl_type(&packet.data, t.nal_length, hevc));
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
                let frame_ns = t.audio_samples as u128 * 1_000_000_000 / rate;
                ensure!(
                    expected.abs_diff(ns as u128) <= cadence_tolerance_ns(frame_ns, scale),
                    "Audio offset/gap needs additional timing support"
                );
                let start_samples = (start * rate as i128).div_euclid(1_000_000_000);
                packet.pts =
                    Some((start_samples + (t.count * t.audio_samples) as i128).try_into()?);
                packet.dts = packet.pts;
                packet.duration = Some(t.audio_samples as i64);
            }
            packet.flags.keyframe = !t.video || frame.is_keyframe == Some(true);
            if t.video {
                if let Some((previous, offset)) = t.held.replace((packet, frame.source_offset)) {
                    write_mapped(&mut *mux, &mapping, &previous, offset)?;
                }
            } else {
                write_mapped(&mut *mux, &mapping, &packet, frame.source_offset)?;
            }
            t.count += 1;
        }
        let mut slots = BTreeMap::new();
        let mut access = BTreeMap::new();
        for t in tracks.values_mut() {
            ensure!(t.count > 0, "Empty media track");
            if t.video {
                let hevc = t.stream.params.codec_id.as_str() == "h265";
                access.insert(t.stream.index + 1, random_access(&t.nal_kinds, &t.pts, hevc));
                t.pts.sort_unstable();
                let spanned = video_cadence_slots(&t.pts, t.origin_ns.unwrap(), t.duration_ns, scale)?;
                // Extend only the final decode duration so the media timeline
                // reaches the last presented frame when source slots are empty.
                let start = t.origin_ns.unwrap() as i128 - t.codec_delay_ns as i128;
                let end = (start + spanned as i128 * t.duration_ns as i128).div_euclid(1_000_000);
                let (mut last, offset) = t.held.take().context("Missing final video packet")?;
                let dts = last.dts.context("Missing video decode time")? as i128;
                last.duration = Some((end - dts).try_into()?);
                write_mapped(&mut *mux, &mapping, &last, offset)?;
                slots.insert(t.stream.index, spanned);
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
                    (start + slots[&t.stream.index] as i128 * t.duration_ns as i128).div_euclid(1_000_000)
                } else {
                    let rate = t.stream.params.sample_rate.unwrap() as i128;
                    let samples = (start * rate).div_euclid(1_000_000_000)
                        + (t.count * t.audio_samples) as i128;
                    (samples * 1000).div_euclid(rate)
                };
                (t.stream.index + 1, end_ms.max(0) as u64)
            })
            .collect();
        let partial_sync_samples: usize = access.values().map(|a| a.partial.len()).sum();
        let demoted_keyframes: usize = access.values().map(|a| a.demoted).sum();
        let visual: BTreeMap<u32, VisualRewrite> = tracks
            .values()
            .filter(|t| t.video)
            .map(|t| {
                let id = t.stream.index + 1;
                (id, VisualRewrite { boxes: t.visual_boxes.clone(), access: access.remove(&id).unwrap_or_default() })
            })
            .collect();
        let labels = tracks
            .values()
            .map(|t| (t.stream.index + 1, t.labels.clone()))
            .collect();
        repair_presentation_durations(output, &durations, &visual, &labels, max_bytes)?;
        Ok(
            serde_json::json!({"tracks":streams.len(),"bytes":output.metadata()?.len(),"development_only":true,
                "audio_tracks":tracks.values().filter(|t|!t.video).count(),"omitted_audio":omitted_audio,
                "partial_sync_samples":partial_sync_samples,"demoted_keyframes":demoted_keyframes}),
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
    visual: &BTreeMap<u32, VisualRewrite>,
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
    visual: &BTreeMap<u32, VisualRewrite>,
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
fn rewrite_visual_container(data: &[u8], visual: &VisualRewrite, depth: usize) -> Result<Vec<u8>> {
    ensure!(depth < 6, "Unexpected MP4 nesting");
    let extra = &visual.boxes;
    let mut out = Vec::new();
    let in_stbl = depth == 3;
    let mut tables = false;
    for (kind, start, end) in children(data)? {
        let body = &data[start..end];
        if in_stbl && [*b"stss", *b"stps"].contains(&kind) {
            continue;
        }
        let rewritten = if [&b"mdia"[..], &b"minf"[..], &b"stbl"[..]].contains(&kind.as_slice()) {
            rewrite_visual_container(body, visual, depth + 1)?
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
        // Random-access tables follow the decode-time tables in stbl.
        if in_stbl && &kind == b"stsz" {
            ensure!(!visual.access.full.is_empty(), "Missing video random-access point");
            out.extend(sample_table(b"stss", &visual.access.full)?);
            if !visual.access.partial.is_empty() {
                out.extend(sample_table(b"stps", &visual.access.partial)?);
            }
            tables = true;
        }
    }
    ensure!(!in_stbl || tables, "Missing video sample sizes");
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

#[cfg(test)]
mod timing_tests {
    use super::*;

    // Minimal Matroska writer for synthetic timing fixtures (test-only).
    fn element(id: u32, body: &[u8]) -> Vec<u8> {
        let mut out: Vec<u8> = id.to_be_bytes().into_iter().skip_while(|b| *b == 0).collect();
        out.push(0x01);
        out.extend_from_slice(&(body.len() as u64).to_be_bytes()[1..]);
        out.extend_from_slice(body);
        out
    }
    fn uint(id: u32, value: u64) -> Vec<u8> {
        element(id, &value.to_be_bytes())
    }
    struct Fixture {
        video_pts_ms: Vec<i64>,
        aac_ms: Option<Vec<i64>>,
        dts: bool,
        // HEVC NAL unit type per video packet (decode order); None writes AVC.
        hevc_nals: Option<Vec<u8>>,
    }
    fn matroska(fixture: &Fixture) -> Vec<u8> {
        let header = [
            uint(0x4286, 1), uint(0x42F7, 1), uint(0x42F2, 4), uint(0x42F3, 8),
            element(0x4282, b"matroska"), uint(0x4287, 4), uint(0x4285, 2),
        ]
        .concat();
        let info = element(0x1549A966, &[uint(0x2AD7B1, 1_000_000), element(0x4D80, b"t"), element(0x5741, b"t")].concat());
        // hvcC: 22 fixed bytes with lengthSizeMinusOne = 3, then no arrays.
        let mut hvcc = vec![1u8; 21];
        hvcc.extend([0x03, 0]);
        let (codec, private): (&[u8], Vec<u8>) = match fixture.hevc_nals {
            Some(_) => (b"V_MPEGH/ISO/HEVC", hvcc),
            None => (b"V_MPEG4/ISO/AVC", vec![1, 0x64, 0, 0x1f, 0xff, 0xe0, 0]),
        };
        let video = element(0xAE, &[
            uint(0xD7, 1), uint(0x73C5, 1), uint(0x83, 1), element(0x86, codec),
            element(0x63A2, &private), uint(0x23E383, 41_708_333),
            element(0xE0, &[uint(0xB0, 64), uint(0xBA, 48)].concat()),
        ].concat());
        let audio = |number: u64, codec: &[u8], private: Option<&[u8]>| element(0xAE, &[
            uint(0xD7, number), uint(0x73C5, number), uint(0x83, 2), element(0x86, codec),
            private.map(|p| element(0x63A2, p)).unwrap_or_default(),
            element(0xE1, &[element(0xB5, &48000f64.to_be_bytes()), uint(0x9F, 2)].concat()),
        ].concat());
        let mut tracks = video;
        if fixture.aac_ms.is_some() { tracks.extend(audio(2, b"A_AAC", Some(&[0x11, 0x90]))); }
        if fixture.dts { tracks.extend(audio(3, b"A_DTS", None)); }
        let block = |track: u8, ms: i64, key: bool, payload: &[u8]| element(0xA3, &[
            &[0x80 | track][..], &(ms as i16).to_be_bytes(), &[if key { 0x80 } else { 0 }], payload,
        ].concat());
        let mut cluster = uint(0xE7, 0);
        // Video is muxed in decode order (one cadence step per packet).
        let mut events: Vec<(i64, Vec<u8>)> = fixture.video_pts_ms.iter().enumerate()
            .map(|(i, ms)| {
                let decode = ((i as f64) * 41.708333).round() as i64;
                match &fixture.hevc_nals {
                    Some(nals) => (decode, block(1, *ms, (16..=23).contains(&nals[i]), &[0, 0, 0, 2, nals[i] << 1, 1, i as u8])),
                    None => (decode, block(1, *ms, i == 0, &[0x21, i as u8, i as u8, 0x5a])),
                }
            }).collect();
        for (i, ms) in fixture.aac_ms.iter().flatten().enumerate() { events.push((*ms, block(2, *ms, true, &[0x21, i as u8, i as u8, 0x5a]))); }
        if fixture.dts { events.push((0, block(3, 0, true, &[0x21, 0, 0, 0x5a]))); }
        events.sort_by_key(|(ms, _)| *ms);
        for (_, b) in events { cluster.extend(b); }
        let segment = [info, element(0x1654AE6B, &tracks), element(0x1F43B675, &cluster)].concat();
        [element(0x1A45DFA3, &header), element(0x18538067, &segment)].concat()
    }
    fn remux(fixture: &Fixture) -> (Result<serde_json::Value>, Option<Vec<u8>>) {
        let dir = std::env::temp_dir().join(format!("torrbaby-remux-timing-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let (input, output) = (dir.join("in.mkv"), dir.join("out.mp4"));
        std::fs::write(&input, matroska(fixture)).unwrap();
        let result = run(&input, &output, 16 * 1024 * 1024);
        let bytes = std::fs::read(&output).ok();
        std::fs::remove_dir_all(dir).unwrap();
        (result, bytes)
    }
    // Movie-timescale tkhd duration, media mdhd duration and media timescale per track.
    fn track_durations(mp4: &[u8]) -> Vec<(u64, u64, u32)> {
        fn find<'a>(data: &'a [u8], kind: &[u8; 4]) -> Vec<&'a [u8]> {
            children(data).unwrap().into_iter().filter(|(k, _, _)| k == kind).map(|(_, s, e)| &data[s..e]).collect()
        }
        // Top level holds a 64-bit mdat, which children() does not walk.
        let mut at = 0;
        let moov = loop {
            let size32 = u32::from_be_bytes(mp4[at..at + 4].try_into().unwrap()) as usize;
            let (size, header) = if size32 == 1 { (u64::from_be_bytes(mp4[at + 8..at + 16].try_into().unwrap()) as usize, 16) } else { (size32, 8) };
            if &mp4[at + 4..at + 8] == b"moov" { break &mp4[at + header..at + size]; }
            at += size;
        };
        find(moov, b"trak").into_iter().map(|trak| {
            let tkhd = find(trak, b"tkhd")[0];
            let tk = if tkhd[0] == 1 { u64::from_be_bytes(tkhd[28..36].try_into().unwrap()) } else { u32::from_be_bytes(tkhd[20..24].try_into().unwrap()) as u64 };
            let mdhd = find(find(trak, b"mdia")[0], b"mdhd")[0];
            let (scale, md) = if mdhd[0] == 1 {
                (u32::from_be_bytes(mdhd[20..24].try_into().unwrap()), u64::from_be_bytes(mdhd[24..32].try_into().unwrap()))
            } else {
                (u32::from_be_bytes(mdhd[12..16].try_into().unwrap()), u32::from_be_bytes(mdhd[16..20].try_into().unwrap()) as u64)
            };
            (tk, md, scale)
        }).collect()
    }
    // stss and stps sample numbers of the first (video) track; None when absent.
    fn sync_tables(mp4: &[u8]) -> (Option<Vec<u32>>, Option<Vec<u32>>) {
        fn find<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
            children(data).unwrap().into_iter().find(|(k, _, _)| k == kind).map(|(_, s, e)| &data[s..e])
        }
        let mut at = 0;
        let moov = loop {
            let size32 = u32::from_be_bytes(mp4[at..at + 4].try_into().unwrap()) as usize;
            let (size, header) = if size32 == 1 { (u64::from_be_bytes(mp4[at + 8..at + 16].try_into().unwrap()) as usize, 16) } else { (size32, 8) };
            if &mp4[at + 4..at + 8] == b"moov" { break &mp4[at + header..at + size]; }
            at += size;
        };
        let stbl = find(find(find(find(moov, b"trak").unwrap(), b"mdia").unwrap(), b"minf").unwrap(), b"stbl").unwrap();
        let table = |kind: &[u8; 4]| find(stbl, kind).map(|b| {
            let n = u32::from_be_bytes(b[4..8].try_into().unwrap()) as usize;
            (0..n).map(|i| u32::from_be_bytes(b[8 + 4 * i..12 + 4 * i].try_into().unwrap())).collect()
        });
        (table(b"stss"), table(b"stps"))
    }
    fn video_ms(slots: impl Iterator<Item = u64>) -> Vec<i64> {
        slots.map(|i| ((i as f64) * 41.708333).round() as i64).collect()
    }

    #[test]
    fn first_vcl_nal_type_skips_parameter_sets_and_sei() {
        // HEVC: VPS(32), SEI(39), then CRA(21).
        let hevc = [0, 0, 0, 2, 32 << 1, 1, 0, 0, 0, 3, 39 << 1, 1, 5, 0, 0, 0, 2, 21 << 1, 1];
        assert_eq!(first_vcl_type(&hevc, 4, true), 21);
        // AVC with 2-byte lengths: SPS(7), PPS(8), then non-IDR slice(1).
        let avc = [0, 1, 0x67, 0, 1, 0x68, 0, 2, 0x41, 0x9a];
        assert_eq!(first_vcl_type(&avc, 2, false), 1);
        assert_eq!(first_vcl_type(&[0, 0, 0, 9, 0x26], 4, true), UNKNOWN_NAL);
        assert_eq!(first_vcl_type(&[0, 0, 0, 0], 4, true), UNKNOWN_NAL);
        assert_eq!(first_vcl_type(&hevc, 0, true), UNKNOWN_NAL);
    }

    #[test]
    fn open_gop_points_are_partial_sync_only_with_rasl_pictures() {
        let k = CONTAINER_KEY;
        // IDR_W_RADL, trailing, CRA + RASL, trailing, CRA + RADL only, BLA + RASL.
        let kinds = [19 | k, 1, 0, 21 | k, 9, 8, 1, 21 | k, 7, 1, 16 | k, 8, 1];
        let access = random_access(&kinds, &[0; 13], true);
        assert_eq!(access, RandomAccess { full: vec![1, 8], partial: vec![4, 11], demoted: 0 });
        // A stream that starts on an open CRA still needs one starting point.
        let access = random_access(&[21 | k, 9, 1, 21 | k, 8, 1], &[0; 6], true);
        assert_eq!((access.full, access.partial), (vec![1], vec![4]));
        // Unparseable HEVC falls back to the container keyframes.
        let access = random_access(&[UNKNOWN_NAL | k, UNKNOWN_NAL, UNKNOWN_NAL | k], &[0; 3], true);
        assert_eq!((access.full, access.partial), (vec![1, 3], vec![]));
    }

    #[test]
    fn open_avc_keyframes_become_ordinary_samples() {
        let k = CONTAINER_KEY;
        // IDR, then a non-IDR keyframe with a leading B frame (pts 3 < 4),
        // then a non-IDR keyframe whose followers are all presented later.
        let kinds = [5 | k, 1, 1, 1 | k, 1, 1, 1 | k, 1];
        let pts = [0, 2, 1, 4, 3, 5, 7, 8];
        let access = random_access(&kinds, &pts, false);
        assert_eq!(access, RandomAccess { full: vec![1, 7], partial: vec![], demoted: 1 });
    }

    #[test]
    fn remuxed_hevc_signals_cra_with_rasl_as_partial_sync() {
        // 24 packets in decode order: IDR, 11 trailing, CRA, 3 RASL, 8 trailing.
        let mut nals = vec![19u8];
        nals.extend([1; 11]);
        nals.extend([21, 8, 8, 8]);
        nals.extend([1; 8]);
        let mut pts: Vec<i64> = video_ms(0..24);
        // The CRA is presented after its three leading pictures.
        pts[12..16].rotate_right(1);
        let (result, mp4) = remux(&Fixture { video_pts_ms: pts, aac_ms: None, dts: false, hevc_nals: Some(nals) });
        assert_eq!(result.unwrap()["partial_sync_samples"], 1);
        assert_eq!(sync_tables(&mp4.unwrap()), (Some(vec![1]), Some(vec![13])));
        // AVC output still carries its sync table and no partial sync table.
        let (_, mp4) = remux(&Fixture { video_pts_ms: video_ms(0..24), aac_ms: None, dts: false, hevc_nals: None });
        assert_eq!(sync_tables(&mp4.unwrap()), (Some(vec![1]), None));
    }
    fn aac_ms(count: u64) -> Vec<i64> {
        (0..count).map(|i| ((i as f64) * 1024.0 / 48.0).round() as i64).collect()
    }

    #[test]
    fn tolerance_is_a_quarter_frame_but_never_below_one_tick() {
        assert_eq!(cadence_tolerance_ns(32_000_000, 1_000_000), 8_000_000);
        assert_eq!(cadence_tolerance_ns(2_000_000, 1_000_000), 1_000_000);
    }

    #[test]
    fn video_slots_accept_missing_pictures_but_not_duplicates_or_variable_cadence() {
        let d = 41_708_333;
        // Gone Girl (2014): presentation slots 2 and 3 are absent in the source.
        let gap: Vec<u64> = [0u64, 42, 167, 209, 250, 292].iter().map(|ms| ms * 1_000_000).collect();
        assert_eq!(video_cadence_slots(&gap, 0, d, 1_000_000).unwrap(), 8);
        let regular: Vec<u64> = (0..1000u64).map(|i| (i * d).div_ceil(1_000_000) * 1_000_000).collect();
        assert_eq!(video_cadence_slots(&regular, 0, d, 1_000_000).unwrap(), 1000);
        assert!(video_cadence_slots(&[0, 42_000_000, 42_000_000], 0, d, 1_000_000).is_err());
        assert!(video_cadence_slots(&[0, 20_000_000], 0, d, 1_000_000).is_err());
        assert!(video_cadence_slots(&[0, 33_000_000, 66_000_000], 0, d, 1_000_000).is_err());
        assert!(video_cadence_slots(&[0, 42_000_000], 42_000_000, d, 1_000_000).is_err());
    }

    #[test]
    fn quantization_jitter_is_accepted_and_missing_audio_frames_are_not() {
        // Citizen Kane / Gone Girl: some packets are stamped 2 ms early, never drifting.
        let mut jitter = aac_ms(240);
        for (i, ms) in jitter.iter_mut().enumerate() { if i % 7 == 3 { *ms -= 2; } }
        let (result, mp4) = remux(&Fixture { video_pts_ms: video_ms(0..120), aac_ms: Some(jitter), dts: false, hevc_nals: None });
        assert_eq!(result.unwrap()["audio_tracks"], 1);
        let durations = track_durations(&mp4.unwrap());
        assert_eq!(durations[0].0, 120 * 41_708_333 / 1_000_000);
        assert_eq!(durations[1].0, 240 * 1024 * 1000 / 48000);

        let mut missing = aac_ms(241);
        missing.remove(100);
        let (result, mp4) = remux(&Fixture { video_pts_ms: video_ms(0..120), aac_ms: Some(missing), dts: false, hevc_nals: None });
        assert_eq!(result.unwrap_err().to_string(), "Audio offset/gap needs additional timing support");
        assert!(mp4.is_none(), "failed remux must remove its output");

        let drift: Vec<i64> = aac_ms(240).iter().enumerate().map(|(i, ms)| ms + i as i64 / 20).collect();
        assert!(remux(&Fixture { video_pts_ms: video_ms(0..120), aac_ms: Some(drift), dts: false, hevc_nals: None }).0.is_err());
    }

    #[test]
    fn empty_video_slots_extend_the_final_decode_duration() {
        let slots = (0..120).filter(|i| ![2, 3].contains(i));
        let (result, mp4) = remux(&Fixture { video_pts_ms: video_ms(slots), aac_ms: Some(aac_ms(240)), dts: false, hevc_nals: None });
        result.unwrap();
        let (presented, media, scale) = track_durations(&mp4.unwrap())[0];
        // 118 frames span 120 cadence slots; media and presentation durations agree.
        assert_eq!(presented, 120 * 41_708_333 / 1_000_000);
        assert_eq!(media * 1000 / scale as u64, presented);
        let duplicate = [video_ms(0..60), video_ms(59..60)].concat();
        assert!(remux(&Fixture { video_pts_ms: duplicate, aac_ms: Some(aac_ms(120)), dts: false, hevc_nals: None }).0.is_err());
    }

    #[test]
    fn unsupported_audio_is_omitted_and_reported() {
        let (result, _) = remux(&Fixture { video_pts_ms: video_ms(0..48), aac_ms: Some(aac_ms(96)), dts: true, hevc_nals: None });
        let result = result.unwrap();
        assert_eq!((result["audio_tracks"].as_u64(), result["omitted_audio"].clone()), (Some(1), serde_json::json!(["A_DTS"])));
        // Creed (2015): DTS is the only audio track; output is video-only for analysis.
        let (result, mp4) = remux(&Fixture { video_pts_ms: video_ms(0..48), aac_ms: None, dts: true, hevc_nals: None });
        let result = result.unwrap();
        assert_eq!((result["audio_tracks"].as_u64(), result["omitted_audio"].clone()), (Some(0), serde_json::json!(["A_DTS"])));
        assert_eq!(track_durations(&mp4.unwrap()).len(), 1);
    }
}
