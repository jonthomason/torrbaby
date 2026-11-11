//! English subtitles transcribed from a title's audio, for files that have no
//! usable English or Chinese subtitles. Apple's on-device SpeechAnalyzer runs in
//! a helper; the transcript is stored as a private SRT and then flows through
//! the normal subtitle jobs (extraction, then on-device translation to Chinese).
use crate::{api::Service, storage::now};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::Digest;
use std::{path::{Path, PathBuf}, process::Stdio, time::Duration};

/// Bump to transcribe again after a helper or segmentation change.
/// 2: Matroska audio is copied to a local raw stream (AVFoundation cannot read
/// audio from the loopback virtual MP4).
/// 3: cue text on one line, so translation sees whole sentences.
const VERSION: u64 = 3;
/// English speech measured 0.85 mean word confidence and ~45 words per minute
/// of running time on feature films; Mandarin and Japanese audio forced through
/// the English model measured 0.37–0.40 and under 5 words per minute.
const MIN_CONFIDENCE: f64 = 0.7;
const MIN_WORDS_PER_MINUTE: f64 = 15.0;
/// Concurrent transcriptions across the processing pool.
static GATE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

pub enum Transcript {
    /// The transcript SRT, ready to use as an English text source.
    Ready(PathBuf),
    /// Not English speech, or no decodable audio. Final for this file revision.
    Rejected(String),
    /// Try again later (playback preparation, Matroska index still building).
    Deferred,
}

/// Transcripts live beside the subtitle cache, outside the media library.
pub fn directory(data: &Path) -> PathBuf { data.join("transcripts") }
pub fn is_transcript(data: &Path, path: &Path) -> bool { path.parent() == Some(directory(data).as_path()) }

fn key(asset: &str, revision: &str) -> String {
    sha2::Sha256::digest(format!("{VERSION}\0{asset}\0{revision}").as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether transcription already reached a verdict (transcript or rejection)
/// for this file revision.
pub fn decided(data: &Path, asset: &str, revision: &str) -> bool { record(data, asset, revision).is_file() }
/// Assets with an accepted transcript of any version.
pub fn transcribed_assets(data: &Path) -> std::collections::HashSet<String> {
    let Ok(entries) = std::fs::read_dir(directory(data)) else { return Default::default() };
    entries.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_slice::<Value>(&std::fs::read(e.path()).ok()?).ok())
        .filter(|record| record["state"] == "ready")
        .filter_map(|record| record["asset_id"].as_str().map(str::to_owned)).collect()
}
pub(crate) fn record(data: &Path, asset: &str, revision: &str) -> PathBuf {
    directory(data).join(format!("{}.json", key(asset, revision)))
}

/// Accept a helper result as English speech, or say why not.
fn verdict(result: &Value) -> Result<(), String> {
    let confidence = result["confidence"].as_f64().unwrap_or(0.0);
    let words = result["words"].as_f64().unwrap_or(0.0);
    let minutes = result["duration_seconds"].as_f64().unwrap_or(0.0) / 60.0;
    if minutes <= 0.0 || words / minutes < MIN_WORDS_PER_MINUTE || confidence < MIN_CONFIDENCE {
        return Err("The audio is not English speech".into());
    }
    Ok(())
}

fn srt(cues: &[crate::subtitles::TimedCue]) -> String {
    // The WebVTT serializer writes SRT-compatible cue blocks after its header.
    crate::subtitles::webvtt(cues).trim_start_matches("WEBVTT\n\n").to_owned()
}

/// The transcript for this file revision, transcribing it now if needed.
/// `report` receives (seconds transcribed, duration) while the helper runs.
pub async fn transcript(s: &Service, asset: &Value, input: &Path, report: impl Fn(u64, u64)) -> Result<Transcript> {
    let id = asset["id"].as_str().context("Missing asset ID")?;
    let revision = crate::subtitle_jobs::revision(input)?;
    let root = directory(&s.data_dir);
    std::fs::create_dir_all(&root)?;
    let name = key(id, &revision);
    let output = root.join(format!("{name}.en.srt"));
    let record = root.join(format!("{name}.json"));
    if let Ok(bytes) = std::fs::read(&record) {
        let saved: Value = serde_json::from_slice(&bytes)?;
        if saved["state"] == "rejected" { return Ok(Transcript::Rejected(saved["reason"].as_str().unwrap_or("No English speech").into())); }
        if saved["state"] == "ready" && output.is_file() { return Ok(Transcript::Ready(output)); }
    }
    if s.virtual_media.preparing() { return Ok(Transcript::Deferred); }
    let _permit = GATE.acquire().await?;
    // AVFoundation cannot decode audio from Matroska, nor read audio samples
    // over HTTP, so copy the chosen audio track's frames into a local raw
    // stream. Cue times are then shifted by that track's first timestamp.
    let mut temporary_audio = None;
    let mut offset_ms = 0;
    let media = if crate::container::is_matroska(input) {
        let (source, directory, base) = (input.to_path_buf(), root.clone(), name.clone());
        match tokio::task::spawn_blocking(move || extract_audio(&source, &directory, &base)).await?? {
            Some((path, first_ms)) => { offset_ms = first_ms; temporary_audio = Some(path.clone()); path.to_str().context("Invalid audio path")?.to_owned() }
            None => {
                let reason = "No decodable audio track";
                let temporary = root.join(format!("{name}.json.tmp"));
                std::fs::write(&temporary, json!({"state":"rejected","reason":reason,"asset_id":id,"updated":now()}).to_string())?;
                std::fs::rename(temporary, &record)?;
                return Ok(Transcript::Rejected(reason.into()));
            }
        }
    } else { input.to_str().context("Invalid media path")?.to_owned() };
    let result_path = root.join(format!("{name}.result.tmp"));
    let progress = root.join(format!("{name}.progress"));
    let outcome = async {
        let helper = std::env::current_exe()?.with_file_name("torrbaby-speech-subtitles");
        let mut child = tokio::process::Command::new("/usr/bin/nice").args(["-n", "15"]).arg(helper)
            .arg(&media).arg(&result_path).arg(&progress)
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(true).spawn()?;
        let stderr = child.stderr.take().context("Missing transcriber errors")?;
        use tokio::io::AsyncReadExt;
        let errors = tokio::spawn(async move { let mut v = Vec::new(); stderr.take(8192).read_to_end(&mut v).await.map(|_| v) });
        let started = std::time::Instant::now();
        let status = loop {
            tokio::select! {
                status = child.wait() => break status?,
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            }
            if let Ok(p) = std::fs::read(&progress).map_err(anyhow::Error::from).and_then(|b| Ok(serde_json::from_slice::<Value>(&b)?)) {
                report(p["completed_cues"].as_u64().unwrap_or(0), p["total_cues"].as_u64().unwrap_or(0));
            }
            if s.virtual_media.preparing() {
                child.kill().await?; let _ = child.wait().await; errors.abort();
                return Ok(None);
            }
            ensure!(started.elapsed() < Duration::from_secs(1800), "Speech transcription timed out");
        };
        let error = String::from_utf8_lossy(&errors.await??).trim().to_owned();
        // 3: no decodable audio; 4: no speech at all (music, ambience).
        if matches!(status.code(), Some(3 | 4)) { return Ok(Some(Err(error))); }
        ensure!(status.success(), "Speech transcription: {}", if error.is_empty() { "helper failed" } else { &error });
        let bytes = std::fs::read(&result_path)?;
        ensure!(bytes.len() <= 16_777_216, "Transcript too large");
        Ok(Some(Ok(serde_json::from_slice::<Value>(&bytes)?)))
    }.await;
    if let Some(path) = &temporary_audio { let _ = std::fs::remove_file(path); }
    let _ = std::fs::remove_file(&result_path);
    let _ = std::fs::remove_file(&progress);
    let Some(outcome) = outcome? else { return Ok(Transcript::Deferred) };
    ensure!(crate::subtitle_jobs::revision(input)? == revision, "Media changed during speech transcription");
    let save = |document: &Value| -> Result<()> {
        let temporary = root.join(format!("{name}.json.tmp"));
        std::fs::write(&temporary, document.to_string())?;
        std::fs::rename(temporary, &record)?; Ok(())
    };
    let result = match outcome {
        Err(reason) => {
            let reason = if reason.is_empty() { "No decodable audio track".to_owned() } else { reason };
            save(&json!({"state":"rejected","reason":reason,"asset_id":id,"updated":now()}))?;
            return Ok(Transcript::Rejected(reason));
        }
        Ok(result) => result,
    };
    let stats = json!({"confidence":result["confidence"],"words":result["words"],"duration_seconds":result["duration_seconds"],"speech_seconds":result["speech_seconds"]});
    if let Err(reason) = verdict(&result) {
        save(&json!({"state":"rejected","reason":reason,"asset_id":id,"stats":stats,"updated":now()}))?;
        tracing::info!(asset = id, %stats, "Speech transcription rejected");
        return Ok(Transcript::Rejected(reason));
    }
    let mut cues: Vec<crate::subtitles::TimedCue> = serde_json::from_value(result["cues"].clone())?;
    for cue in &mut cues { cue.start_ms += offset_ms; cue.end_ms += offset_ms; }
    ensure!(!cues.is_empty() && cues.iter().all(|c| c.end_ms > c.start_ms && !c.text.trim().is_empty() && c.text.len() <= 16_384),
        "Invalid speech transcript");
    let temporary = root.join(format!("{name}.en.srt.tmp"));
    std::fs::write(&temporary, srt(&cues))?;
    std::fs::rename(temporary, &output)?;
    save(&json!({"state":"ready","asset_id":id,"stats":stats,"cues":cues.len(),"updated":now()}))?;
    tracing::info!(asset = id, %stats, cues = cues.len(), "Speech transcription finished");
    Ok(Transcript::Ready(output))
}

/// ADTS header fields from an AAC AudioSpecificConfig: (profile, sampling
/// frequency index, channel configuration). HE-AAC signals its AAC-LC core.
fn adts_config(config: &[u8]) -> Result<(u8, u8, u8)> {
    ensure!(config.len() >= 2, "Invalid AAC configuration");
    let bits = u32::from(config[0]) << 24 | u32::from(config[1]) << 16
        | u32::from(*config.get(2).unwrap_or(&0)) << 8 | u32::from(*config.get(3).unwrap_or(&0));
    let mut object = (bits >> 27) as u8;
    let mut frequency = ((bits >> 23) & 0xF) as u8;
    let channels = ((bits >> 19) & 0xF) as u8;
    if object == 5 || object == 29 {
        // Explicit SBR/PS signalling: the core sampling rate is the one above;
        // the extension rate and core object type follow.
        object = ((bits >> 10) & 0x1F) as u8;
        if object == 0 { object = 2; }
    }
    ensure!((1..=4).contains(&object) && frequency < 13 && (1..=7).contains(&channels), "Unsupported AAC configuration");
    frequency &= 0xF;
    Ok((object - 1, frequency, channels))
}
fn adts_header((profile, frequency, channels): (u8, u8, u8), payload: usize) -> Result<[u8; 7]> {
    let length = payload + 7;
    ensure!(length < 8192, "AAC frame too large for ADTS");
    Ok([0xFF, 0xF1, profile << 6 | frequency << 2 | channels >> 2, (channels & 3) << 6 | (length >> 11) as u8,
        (length >> 3) as u8, ((length & 7) as u8) << 5 | 0x1F, 0xFC])
}
/// Copy one Matroska audio track (English when tagged, else the first
/// decodable one) into a raw stream AVFoundation reads: AC-3/E-AC-3 frames as
/// they are, AAC with ADTS headers. Returns the file and the track's first
/// timestamp in milliseconds, or None when no track has a decodable codec.
fn extract_audio(input: &Path, directory: &Path, name: &str) -> Result<Option<(PathBuf, u64)>> {
    use matroska_demuxer::{Frame, MatroskaFile, TrackType};
    use std::io::Write;
    let file = std::fs::File::open(input)?;
    ensure!(file.metadata()?.is_file(), "Expected a regular media file");
    let mut mkv = MatroskaFile::open(std::io::BufReader::new(file)).context("Read Matroska container")?;
    let scale = mkv.info().timestamp_scale().get();
    let candidates: Vec<_> = mkv.tracks().iter().filter(|t| t.track_type() == TrackType::Audio
        && matches!(t.codec_id(), "A_AAC" | "A_AC3" | "A_EAC3")
        && crate::matroska::encodings_supported(t.content_encodings())).collect();
    let english = |t: &&&matroska_demuxer::TrackEntry| {
        let tag = t.language_bcp47().or(t.language()).unwrap_or("").to_ascii_lowercase();
        tag == "en" || tag == "eng" || tag.starts_with("en-")
    };
    let Some(track) = candidates.iter().find(english).or(candidates.first()) else { return Ok(None) };
    let number = track.track_number().get();
    let encodings = track.content_encodings().map(|e| e.to_vec());
    let (extension, adts) = match track.codec_id() {
        "A_AAC" => {
            let private = track.codec_private().context("Missing AAC configuration")?;
            let config = crate::matroska::decode_private(encodings.as_deref(), private, 1024)?;
            ("aac", Some(adts_config(&config)?))
        }
        "A_AC3" => ("ac3", None),
        _ => ("ec3", None),
    };
    let path = directory.join(format!("{name}.audio.{extension}"));
    let mut output = std::io::BufWriter::new(std::fs::File::create(&path)?);
    let (mut frame, mut first, mut written) = (Frame::default(), None, 0usize);
    while mkv.next_frame(&mut frame)? {
        if frame.track != number { continue; }
        let data = crate::matroska::decode_frame(encodings.as_deref(), &frame.data, 1 << 20)?;
        first.get_or_insert(frame.timestamp.saturating_mul(scale) / 1_000_000);
        if let Some(config) = adts { output.write_all(&adts_header(config, data.len())?)?; }
        output.write_all(&data)?;
        written += data.len();
        ensure!(written < 8 << 30, "Audio track too large");
    }
    output.flush()?;
    Ok(first.map(|first| (path, first)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_confident_english_speech_is_accepted() {
        // Measured: The Spy Who Loved Me (English), Mandarin and Japanese samples.
        let english = json!({"confidence":0.857,"words":5544,"duration_seconds":7540.5});
        let mandarin = json!({"confidence":0.404,"words":73,"duration_seconds":900.0});
        let japanese = json!({"confidence":0.365,"words":21,"duration_seconds":900.0});
        assert!(verdict(&english).is_ok());
        assert!(verdict(&mandarin).is_err() && verdict(&japanese).is_err());
        // Confident but nearly silent (music video, ambience) is not a dialogue track.
        assert!(verdict(&json!({"confidence":0.9,"words":40,"duration_seconds":600.0})).is_err());
    }
    #[test]
    fn aac_configuration_becomes_adts_headers() -> Result<()> {
        // AAC-LC, 48 kHz (index 3), stereo: AudioSpecificConfig 0x11 0x90.
        let config = adts_config(&[0x11, 0x90])?;
        assert_eq!(config, (1, 3, 2));
        let header = adts_header(config, 100)?;
        assert_eq!(header, [0xFF, 0xF1, 0x4C, 0x80, 0x0D, 0x7F, 0xFC]);
        // 5.1 AAC-LC at 48 kHz: 0x11 0xB0.
        assert_eq!(adts_config(&[0x11, 0xB0])?, (1, 3, 6));
        assert!(adts_config(&[0xF8, 0x00]).is_err(), "escape object types are not ADTS");
        Ok(())
    }
    #[test]
    fn transcripts_are_srt_that_the_text_pipeline_reads_back() -> Result<()> {
        let cues = vec![crate::subtitles::TimedCue { start_ms: 1500, end_ms: 3200, text: "Bond.\nJames Bond.".into() }];
        let text = srt(&cues);
        assert!(text.starts_with("1\n"));
        let back = crate::subtitles::parse_srt(&text)?;
        assert_eq!((back[0].start_ms, back[0].end_ms, back[0].text.as_str()), (1500, 3200, "Bond.\nJames Bond."));
        let data = Path::new("/data");
        assert!(is_transcript(data, &directory(data).join("a.en.srt")));
        assert!(!is_transcript(data, Path::new("/media/a.en.srt")));
        Ok(())
    }
}
