//! Development-only Matroska passthrough. Not admitted to production playback yet.
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
}
impl Write for LimitedFile {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let end = self.file.stream_position()?.checked_add(bytes.len() as u64);
        if end.is_none_or(|end| end > self.limit) {
            return Err(std::io::Error::other("Remux storage limit reached"));
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
}

pub fn run(input: &Path, output: &Path, max_bytes: u64) -> Result<serde_json::Value> {
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
            let asc = &params.extradata;
            // Initial support is ordinary AAC-LC with 1024 samples/access unit.
            ensure!(
                asc.len() == 2 && asc[0] >> 3 == 2 && asc[1] & 7 == 0,
                "AAC configuration needs additional support"
            );
            let rates = [
                96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000,
                7350,
            ];
            let rate = *rates
                .get(((asc[0] & 7) << 1 | asc[1] >> 7) as usize)
                .context("Invalid AAC sample rate")?;
            let channels = (asc[1] >> 3) & 15;
            ensure!(
                (1..=6).contains(&channels)
                    && a.sampling_frequency() == rate as f64
                    && a.channels().get() == channels as u64,
                "AAC layout mismatch"
            );
            params.sample_rate = Some(rate);
            params.channels = Some(channels.into());
            duration_ns = 0;
            time_base = TimeBase::new(1, rate as i64);
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
                    1024
                },
                origin_ns: None,
                codec_delay_ns: t.codec_delay().unwrap_or(0),
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
            mux.write_packet(&packet)?;
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
        repair_presentation_durations(output, &durations)?;
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
fn repair_presentation_durations(path: &Path, durations: &BTreeMap<u32, u64>) -> Result<()> {
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
            file.seek(SeekFrom::Start(offset + header_size))?;
            file.write_all(&moov)?;
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
