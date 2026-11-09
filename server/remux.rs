//! Development-only Matroska passthrough. Not admitted to production playback yet.
use anyhow::{Context, Result, ensure};
use matroska_demuxer::{Frame, MatroskaFile, TrackType};
use oxideav_core::{CodecId, CodecParameters, Packet, StreamInfo, TimeBase};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{BufReader, Seek, SeekFrom, Write},
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
            t.content_encodings().is_none() && t.codec_delay().unwrap_or(0) == 0,
            "Encoded tracks or codec delay need additional remux support"
        );
        let video = t.track_type() == TrackType::Video;
        let codec = match t.codec_id() {
            "V_MPEG4/ISO/AVC" => "h264",
            "V_MPEGH/ISO/HEVC" => "hevc",
            "A_AAC" => "aac",
            other => anyhow::bail!("Remux codec not implemented yet: {other}"),
        };
        let mut params = if video {
            CodecParameters::video(CodecId::new(codec))
        } else {
            CodecParameters::audio(CodecId::new(codec))
        };
        params.extradata = t
            .codec_private()
            .context("Missing codec configuration")?
            .to_vec();
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
            },
        );
    }
    ensure!(tracks.values().any(|t| t.video), "No supported video track");
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
            write_edit_list: false,
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
            let mut packet = Packet::new(
                t.stream.index,
                t.stream.time_base,
                std::mem::take(&mut frame.data),
            );
            if t.video {
                t.pts.push(ns);
                let dts = (t.count as u128 * t.duration_ns as u128 / 1_000_000) as i64;
                let next = ((t.count + 1) as u128 * t.duration_ns as u128 / 1_000_000) as i64;
                packet.pts = Some((ns / 1_000_000).try_into()?);
                packet.dts = Some(dts);
                packet.duration = Some(next - dts);
            } else {
                let rate = t.stream.params.sample_rate.unwrap() as u128;
                let expected = t.count as u128 * 1024 * 1_000_000_000 / rate;
                ensure!(
                    expected.abs_diff(ns as u128) <= scale as u128,
                    "Audio offset/gap needs additional timing support"
                );
                packet.pts = Some((t.count * 1024).try_into()?);
                packet.dts = packet.pts;
                packet.duration = Some(1024);
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
                    let expected = i as u128 * t.duration_ns as u128;
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
        Ok(
            serde_json::json!({"tracks":streams.len(),"bytes":output.metadata()?.len(),"development_only":true}),
        )
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(output);
    }
    result
}
