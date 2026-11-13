//! Subtitles made from the title itself, for files that have no usable English or
//! Chinese subtitles. Two sources, each run on this Mac by a helper:
//! - the subtitles printed in the picture, as most Chinese dramas and films carry
//!   them: read from frames of the video, exact where speech is misheard;
//! - the speech, English or Mandarin, transcribed by Apple's SpeechAnalyzer.
//! The result is stored as a private SRT and then flows through the normal
//! subtitle jobs (extraction, then on-device translation to the other language).
//! The sources are tried in turn; one that does not apply is rejected quickly.
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
/// Mandarin drama measured 0.87–0.93 confidence and 115–128 characters per minute.
const MIN_CHARACTERS_PER_MINUTE: f64 = 40.0;
/// The languages speech is transcribed in.
const LANGUAGES: [&str; 2] = ["en", "zh-Hans"];
/// Reading the subtitles printed in the picture.
/// ("burned" was this step's name for one evening, while it could not find the picture
/// in files that open with an empty video stream; verdicts under that name do not count.)
const PICTURE: &str = "picture";
/// Everything tried, in the order a title with a Chinese name is tried.
const STEPS: [&str; 3] = [PICTURE, "zh-Hans", "en"];
const NOT_SPEECH: &str = "No subtitles in the picture, and the audio is not English or Mandarin speech";
const NO_AUDIO: &str = "No decodable audio track";
/// Concurrent transcriptions across the processing pool.
static GATE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

pub enum Transcript {
    /// The transcript SRT and its language, ready to use as a text source.
    Ready(PathBuf, &'static str),
    /// Neither English nor Mandarin speech, or no decodable audio. Final for this file revision.
    Rejected(String),
    /// Try again later (playback preparation, Matroska index still building).
    Deferred,
}

/// Transcripts live beside the subtitle cache, outside the media library.
pub fn directory(data: &Path) -> PathBuf { data.join("transcripts") }
/// How a subtitle source the server made itself was made: "ocr" when read from the
/// picture, "speech" when transcribed; None for any other file.
pub fn method(data: &Path, path: &Path) -> Option<&'static str> {
    if path.parent() != Some(directory(data).as_path()) { return None; }
    Some(if path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(".burned.srt")) { "ocr" } else { "speech" })
}

/// Named for the file's identity that survives a move or rename (device, inode,
/// size, modification time), so filing a title does not transcribe it again.
/// Records made before this used the full revision and keep their names.
fn key(data: &Path, asset: &str, revision: &str) -> String {
    let name = |revision: &str| -> String { sha2::Sha256::digest(format!("{VERSION}\0{asset}\0{revision}").as_bytes()).iter().map(|b| format!("{b:02x}")).collect() };
    let legacy = name(revision);
    if directory(data).join(format!("{legacy}.json")).is_file() { return legacy; }
    name(&revision.split(':').take(5).collect::<Vec<_>>().join(":"))
}

/// Whether transcription already reached a verdict (transcript or rejection)
/// for this file revision.
pub fn decided(data: &Path, asset: &str, revision: &str) -> bool {
    std::fs::read(record(data, asset, revision)).ok().and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|saved| untried(&saved).is_empty())
}
/// What a file has yet to be tried with. Records from when only English speech was
/// transcribed carry no list: a rejection then had tried English alone, which is how
/// every Mandarin title came to have no subtitles. A Mandarin transcript made before
/// the picture was read is redone from the picture, which is far more accurate; an
/// English transcript is kept as it is.
fn untried(saved: &Value) -> Vec<&'static str> {
    let listed = saved["tried"].as_array().map(|list| list.iter().filter_map(Value::as_str).collect::<Vec<_>>());
    let tried: Vec<&str> = match (saved["state"].as_str(), listed) {
        (_, Some(list)) => list,
        (Some("ready"), None) if saved["language"] == "zh-Hans" => vec!["zh-Hans", "en"],
        (Some("ready"), None) => return vec![],
        (_, None) if saved["reason"] == NO_AUDIO => vec!["en", "zh-Hans"],
        (_, None) => vec!["en"],
    };
    if saved["state"] == "ready" { return if tried.contains(&PICTURE) { vec![] } else { vec![PICTURE] }; }
    STEPS.into_iter().filter(|step| !tried.contains(step)).collect()
}
/// Chinese in the file's name: try Mandarin first.
fn chinese_name(input: &Path) -> bool {
    input.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)))
}
/// Assets with an accepted transcript of any version.
pub fn transcribed_assets(data: &Path) -> std::collections::HashSet<String> {
    let Ok(entries) = std::fs::read_dir(directory(data)) else { return Default::default() };
    entries.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_slice::<Value>(&std::fs::read(e.path()).ok()?).ok())
        .filter(|record| record["state"] == "ready")
        .filter_map(|record| record["asset_id"].as_str().map(str::to_owned)).collect()
}
pub(crate) fn record(data: &Path, asset: &str, revision: &str) -> PathBuf {
    directory(data).join(format!("{}.json", key(data, asset, revision)))
}

/// Accept a helper result as speech in the language it was transcribed in, or not.
fn verdict(result: &Value, language: &str) -> bool {
    let confidence = result["confidence"].as_f64().unwrap_or(0.0);
    let minutes = result["duration_seconds"].as_f64().unwrap_or(0.0) / 60.0;
    // Chinese is written without spaces, so its measure is characters.
    let (amount, least) = if language == "zh-Hans" { (result["characters"].as_f64().unwrap_or(0.0), MIN_CHARACTERS_PER_MINUTE) }
        else { (result["words"].as_f64().unwrap_or(0.0), MIN_WORDS_PER_MINUTE) };
    minutes > 0.0 && amount / minutes >= least && confidence >= MIN_CONFIDENCE
}

fn srt(cues: &[crate::subtitles::TimedCue]) -> String {
    // The WebVTT serializer writes SRT-compatible cue blocks after its header.
    crate::subtitles::webvtt(cues).trim_start_matches("WEBVTT\n\n").to_owned()
}

/// The subtitles for this file revision, making them now if needed.
/// `report` receives (seconds done, duration, source) while a helper runs; the source is
/// "picture" while subtitles are read from the picture and "speech" while audio is transcribed.
pub async fn transcript(s: &Service, asset: &Value, input: &Path, report: impl Fn(u64, u64, &str)) -> Result<Transcript> {
    let id = asset["id"].as_str().context("Missing asset ID")?;
    let revision = crate::subtitle_jobs::revision(input)?;
    let root = directory(&s.data_dir);
    std::fs::create_dir_all(&root)?;
    let name = key(&s.data_dir, id, &revision);
    let record = root.join(format!("{name}.json"));
    let language_of = |saved: &Value| LANGUAGES.into_iter().find(|l| saved["language"] == *l).unwrap_or("en");
    // A Chinese name: the picture, then Mandarin. Otherwise English speech comes first.
    let mut remaining: Vec<&'static str> = if chinese_name(Path::new(asset["path"].as_str().unwrap_or_default())) || chinese_name(input) { STEPS.to_vec() } else { vec!["en", PICTURE, "zh-Hans"] };
    let mut tried: Vec<&'static str> = Vec::new();
    // A transcript already made, kept if the picture turns out to hold no subtitles.
    let mut kept: Option<(Value, PathBuf, &'static str)> = None;
    if let Ok(bytes) = std::fs::read(&record) {
        let saved: Value = serde_json::from_slice(&bytes)?;
        let left = untried(&saved);
        let language = language_of(&saved);
        let output = root.join(saved["output"].as_str().map(str::to_owned).unwrap_or_else(|| format!("{name}.{language}.srt")));
        if saved["state"] == "ready" && output.is_file() {
            if left.is_empty() { return Ok(Transcript::Ready(output, language)); }
            kept = Some((saved.clone(), output, language));
        } else if saved["state"] == "rejected" && left.is_empty() {
            return Ok(Transcript::Rejected(saved["reason"].as_str().unwrap_or(NOT_SPEECH).into()));
        }
        if saved["state"] == "ready" || saved["state"] == "rejected" {
            tried = STEPS.into_iter().filter(|step| !left.contains(step)).collect();
            remaining.retain(|step| left.contains(step));
        }
    }
    if s.virtual_media.preparing() { return Ok(Transcript::Deferred); }
    let _permit = GATE.acquire().await?;
    let save = |document: &Value| -> Result<()> {
        let temporary = root.join(format!("{name}.json.tmp"));
        std::fs::write(&temporary, document.to_string())?;
        std::fs::rename(temporary, &record)?; Ok(())
    };
    // What the record says while nothing has been accepted: the kept transcript, or a rejection.
    let unsettled = |tried: &[&str], reason: &str, stats: &Value| -> Value {
        match &kept {
            Some((saved, _, _)) => { let mut saved = saved.clone(); saved["tried"] = json!(tried); saved }
            None => json!({"state":"rejected","reason":reason,"tried":tried,"asset_id":id,"stats":stats,"updated":now()}),
        }
    };
    // The audio, prepared when the first speech step needs it: (what the helper reads,
    // the track's first timestamp, a temporary file to remove).
    let mut audio: Option<(String, u64, Option<PathBuf>)> = None;
    let mut finding: Option<Result<Transcript>> = None;
    let mut reason = NOT_SPEECH.to_owned();
    for step in remaining {
        if tried.contains(&step) { continue; }
        let heard = if step == PICTURE {
            let media = input.to_str().context("Invalid media path")?;
            run(s, &root, &name, "torrbaby-burned-subtitles", &[media], &report).await
        } else {
            if audio.is_none() {
                // AVFoundation cannot decode audio from Matroska, nor read audio samples
                // over HTTP, so copy the chosen audio track's frames into a local raw
                // stream. Cue times are then shifted by that track's first timestamp.
                audio = if crate::container::is_matroska(input) {
                    let (source, directory, base) = (input.to_path_buf(), root.clone(), name.clone());
                    match tokio::task::spawn_blocking(move || extract_audio(&source, &directory, &base)).await?? {
                        Some((path, first_ms)) => Some((path.to_str().context("Invalid audio path")?.to_owned(), first_ms, Some(path))),
                        None => None,
                    }
                } else { Some((input.to_str().context("Invalid media path")?.to_owned(), 0, None)) };
            }
            match &audio {
                Some((media, _, _)) => run(s, &root, &name, "torrbaby-speech-subtitles", &[media, step], &report).await,
                None => Ok(Some(Err(NO_AUDIO.to_owned()))),
            }
        };
        let outcome = match heard {
            Ok(Some(outcome)) => outcome,
            Ok(None) => { finding = Some(Ok(Transcript::Deferred)); break; }
            Err(error) => { finding = Some(Err(error)); break; }
        };
        if crate::subtitle_jobs::revision(input)? != revision { finding = Some(Err(anyhow::anyhow!("Media changed while its subtitles were made"))); break; }
        tried.push(step);
        let result = match outcome {
            Err(why) => {
                // No audio to decode is no better in the other language.
                if step != PICTURE && (why.is_empty() || why == NO_AUDIO) { for language in LANGUAGES { if !tried.contains(&language) { tried.push(language); } } }
                if step != PICTURE { reason = if why.is_empty() { NO_AUDIO.to_owned() } else { why }; }
                save(&unsettled(&tried, &reason, &Value::Null))?;
                continue;
            }
            Ok(result) => result,
        };
        let stats = json!({"source":step,"confidence":result["confidence"],"words":result["words"],"characters":result["characters"],"duration_seconds":result["duration_seconds"],"speech_seconds":result["speech_seconds"],"cue_seconds":result["cue_seconds"]});
        let accepted = if step == PICTURE { result["language"] == "zh-Hans" && result["method"] == "burned" } else { verdict(&result, step) };
        if !accepted {
            tracing::info!(asset = id, %stats, "Transcription rejected");
            if step != PICTURE { reason = NOT_SPEECH.to_owned(); }
            save(&unsettled(&tried, &reason, &stats))?;
            continue;
        }
        let offset_ms = if step == PICTURE { 0 } else { audio.as_ref().map_or(0, |a| a.1) };
        let mut cues: Vec<crate::subtitles::TimedCue> = serde_json::from_value(result["cues"].clone())?;
        for cue in &mut cues { cue.start_ms += offset_ms; cue.end_ms += offset_ms; }
        if cues.is_empty() || !cues.iter().all(|c| c.end_ms > c.start_ms && !c.text.trim().is_empty() && c.text.len() <= 16_384) {
            finding = Some(Err(anyhow::anyhow!("Invalid transcript"))); break;
        }
        let (language, file) = if step == PICTURE { ("zh-Hans", format!("{name}.zh-Hans.burned.srt")) } else { (step, format!("{name}.{step}.srt")) };
        let temporary = root.join(format!("{file}.tmp"));
        std::fs::write(&temporary, srt(&cues))?;
        std::fs::rename(temporary, root.join(&file))?;
        save(&json!({"state":"ready","language":language,"method":if step == PICTURE {"burned"} else {"speech"},"output":file,"tried":tried,"asset_id":id,"stats":stats,"cues":cues.len(),"updated":now()}))?;
        tracing::info!(asset = id, %stats, cues = cues.len(), "Transcription finished");
        finding = Some(Ok(Transcript::Ready(root.join(file), language)));
        break;
    }
    if let Some((_, _, Some(path))) = &audio { let _ = std::fs::remove_file(path); }
    match (finding, kept) {
        (Some(finding), _) => finding,
        (None, Some((_, output, language))) => Ok(Transcript::Ready(output, language)),
        (None, None) => Ok(Transcript::Rejected(reason)),
    }
}

/// Run one helper over the title. None: stopped for playback preparation. Err inside:
/// the helper found nothing of its kind (no subtitles in the picture, no decodable
/// audio, no speech).
async fn run(s: &Service, root: &Path, name: &str, helper: &str, arguments: &[&str], report: &impl Fn(u64, u64, &str)) -> Result<Option<Result<Value, String>>> {
    let source = if helper == "torrbaby-burned-subtitles" { "picture" } else { "speech" };
    let result_path = root.join(format!("{name}.result.tmp"));
    let progress = root.join(format!("{name}.progress"));
    let outcome = async {
        let helper = std::env::current_exe()?.with_file_name(helper);
        // Media first, then where to write, then the helper's own options.
        let mut child = tokio::process::Command::new("/usr/bin/nice").args(["-n", "15"]).arg(helper)
            .arg(arguments[0]).arg(&result_path).arg(&progress).args(&arguments[1..])
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(true).spawn()?;
        let stderr = child.stderr.take().context("Missing helper errors")?;
        use tokio::io::AsyncReadExt;
        let errors = tokio::spawn(async move { let mut v = Vec::new(); stderr.take(8192).read_to_end(&mut v).await.map(|_| v) });
        let started = std::time::Instant::now();
        let status = loop {
            tokio::select! {
                status = child.wait() => break status?,
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            }
            if let Ok(p) = std::fs::read(&progress).map_err(anyhow::Error::from).and_then(|b| Ok(serde_json::from_slice::<Value>(&b)?)) {
                report(p["completed_cues"].as_u64().unwrap_or(0), p["total_cues"].as_u64().unwrap_or(0), source);
            }
            if s.virtual_media.preparing() {
                child.kill().await?; let _ = child.wait().await; errors.abort();
                return Ok(None);
            }
            ensure!(started.elapsed() < Duration::from_secs(3600), "Making subtitles from the title timed out");
        };
        let error = String::from_utf8_lossy(&errors.await??).trim().to_owned();
        // 3: nothing to decode; 4: nothing of this kind in the title (no speech, no subtitles in the picture).
        if matches!(status.code(), Some(3 | 4)) { return Ok(Some(Err(error))); }
        ensure!(status.success(), "Subtitles from the title: {}", if error.is_empty() { "helper failed" } else { &error });
        let bytes = std::fs::read(&result_path)?;
        ensure!(bytes.len() <= 16_777_216, "Transcript too large");
        Ok(Some(Ok(serde_json::from_slice::<Value>(&bytes)?)))
    }.await;
    let _ = std::fs::remove_file(&result_path);
    let _ = std::fs::remove_file(&progress);
    outcome
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
    drop(file);
    let mut mkv = MatroskaFile::open(std::io::BufReader::new(crate::matroska::Bounded::open(input)?)).context("Read Matroska container")?;
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
        assert!(verdict(&english, "en"));
        assert!(!verdict(&mandarin, "en") && !verdict(&japanese, "en"));
        // Confident but nearly silent (music video, ambience) is not a dialogue track.
        assert!(!verdict(&json!({"confidence":0.9,"words":40,"duration_seconds":600.0}), "en"));
    }
    #[test]
    fn mandarin_speech_is_accepted_as_mandarin_and_not_as_english() {
        // Measured on ten minutes each of 尚公主 and 九重紫, in both languages.
        let shang = json!({"confidence":0.933,"words":1149,"characters":1149,"duration_seconds":600.0});
        let jiu = json!({"confidence":0.875,"words":1277,"characters":1275,"duration_seconds":600.0});
        assert!(verdict(&shang, "zh-Hans") && verdict(&jiu, "zh-Hans"));
        assert!(!verdict(&json!({"confidence":0.641,"words":6,"characters":26,"duration_seconds":600.0}), "en"));
        assert!(!verdict(&json!({"confidence":0.246,"words":8,"characters":30,"duration_seconds":600.0}), "en"));
        assert!(!verdict(&json!({"confidence":0.9,"characters":120,"duration_seconds":600.0}), "zh-Hans"), "a few words in a music video");
    }
    #[test]
    fn earlier_verdicts_are_reopened_for_what_they_had_not_tried() {
        // Rejected when only English was transcribed: the picture and Mandarin remain.
        assert_eq!(untried(&json!({"state":"rejected","reason":"The audio is not English speech"})), ["picture", "zh-Hans"]);
        // No audio to decode says nothing about the picture.
        assert_eq!(untried(&json!({"state":"rejected","reason":"No decodable audio track"})), ["picture"]);
        assert!(untried(&json!({"state":"rejected","reason":NOT_SPEECH,"tried":["picture","zh-Hans","en"]})).is_empty());
        assert_eq!(untried(&json!({"state":"rejected","reason":NOT_SPEECH,"tried":["zh-Hans","en"]})), ["picture"]);
        assert_eq!(untried(&json!({"state":"ready","language":"zh-Hans","tried":["en","burned","zh-Hans"]})), ["picture"], "a verdict from before the picture could be found");
        // A Mandarin transcript is redone from the picture once; an English one is kept.
        assert_eq!(untried(&json!({"state":"ready","language":"zh-Hans"})), ["picture"]);
        assert!(untried(&json!({"state":"ready","language":"zh-Hans","tried":["zh-Hans","picture"]})).is_empty());
        assert!(untried(&json!({"state":"ready"})).is_empty() && untried(&json!({"state":"ready","language":"en"})).is_empty());
        assert!(chinese_name(Path::new("/tv/尚公主 - S01E05.mkv")) && !chinese_name(Path::new("/tv/Black Doves - S01E01.mkv")));
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
        assert_eq!(method(data, &directory(data).join("a.en.srt")), Some("speech"));
        assert_eq!(method(data, &directory(data).join("a.zh-Hans.burned.srt")), Some("ocr"), "read from the picture, so named as images are");
        assert_eq!(method(data, Path::new("/media/a.en.srt")), None);
        Ok(())
    }
}
