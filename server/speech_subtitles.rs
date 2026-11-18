//! Subtitles made from the title itself, for files that have no usable English or
//! Chinese subtitles. Two sources, each run on this Mac by a helper:
//! - the subtitles printed in the picture, as most Chinese dramas and films carry
//!   them: read from frames of the video, exact where speech is misheard;
//! - the speech, English or Mandarin, transcribed by Apple's SpeechAnalyzer.
//! The result is stored as a private SRT and then flows through the normal
//! subtitle jobs (extraction, then on-device translation to the other language).
//! The sources are tried in turn; one that does not apply is rejected quickly.
use crate::{api::Service, storage::{Database, now}};
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
/// English subtitles people wrote, from OpenSubtitles: tried before any transcript of the
/// audio, and they replace English transcripts made before this source existed.
const OPENSUBTITLES: &str = "opensubtitles";
/// Everything tried, in the order a title with a Chinese name is tried: its picture's
/// Chinese subtitles are exact, so they come first there.
const STEPS: [&str; 4] = [PICTURE, OPENSUBTITLES, "zh-Hans", "en"];
const NOT_SPEECH: &str = "No subtitles in the picture, and the audio is not English or Mandarin speech";
const NO_AUDIO: &str = "No decodable audio track";
/// Records made since audio AVFoundation cannot decode goes through ffmpeg carry this; a
/// "no decodable audio" verdict without it is tried again (Creed's DTS, say).
const AUDIO_DECODER: u64 = 2;
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
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    Some(if name.ends_with(".burned.srt") { "ocr" } else if name.ends_with(".opensubtitles.srt") { "opensubtitles" } else { "speech" })
}

/// Named for the file's identity that survives a move or rename (device, inode,
/// size, modification time), so filing a title does not transcribe it again.
/// Records made before this used the full revision and keep their names.
fn key(db: &Database, asset: &str, revision: &str) -> String {
    let name = |revision: &str| -> String { sha2::Sha256::digest(format!("{VERSION}\0{asset}\0{revision}").as_bytes()).iter().map(|b| format!("{b:02x}")).collect() };
    let legacy = name(revision);
    if load(db, &legacy).is_some() { return legacy; }
    name(&revision.split(':').take(5).collect::<Vec<_>>().join(":"))
}

/// Each file's transcript record ("<key>") and OpenSubtitles records ("<key>.en-opensubtitles",
/// "<key>.zh-opensubtitles") are rows of subtitle_records; the subtitles they name are files
/// in `directory`. (They were JSON files beside them, which processing read by the thousand
/// every few seconds.)
pub(crate) fn load(db: &Database, name: &str) -> Option<Value> {
    use rusqlite::OptionalExtension;
    let text: Option<String> = db.connection.query_row("SELECT document FROM subtitle_records WHERE name=?", [name], |r| r.get(0)).optional().ok()?;
    serde_json::from_str(&text?).ok()
}
pub(crate) fn store(db: &Database, name: &str, document: &Value) -> Result<()> {
    let kind = name.split_once('.').map_or("transcript", |(_, kind)| kind.trim_end_matches("-opensubtitles"));
    db.connection.execute("INSERT INTO subtitle_records VALUES(?,?,?,?) ON CONFLICT(name) DO UPDATE SET asset_id=excluded.asset_id,kind=excluded.kind,document=excluded.document",
        rusqlite::params![name, document["asset_id"].as_str(), kind, document.to_string()])?;
    Ok(())
}
fn database(s: &Service) -> Result<std::sync::MutexGuard<'_, Database>> { s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable")) }

/// `asset` with its title's name and year (a movie) or its show's name (an episode), from the
/// title's metadata, so OpenSubtitles can be searched by name when it lacks the TMDB id.
fn titled(s: &Service, asset: &Value) -> Value {
    let mut asset = asset.clone();
    let Some(entity) = asset["entity_id"].as_str().map(str::to_owned) else { return asset };
    let document = database(s).ok().and_then(|db| db.connection.query_row("SELECT document FROM library_entities WHERE id=?", [&entity], |r| r.get::<_, String>(0)).ok());
    let Some(title) = document.and_then(|d| serde_json::from_str::<Value>(&d).ok()) else { return asset };
    if entity.starts_with("tmdb:movie:") {
        asset["title"] = title["title"].clone();
        asset["year"] = title["year"].as_u64().or_else(|| title["year"].as_str().and_then(|y| y.parse().ok())).map_or(Value::Null, |y| json!(y));
    } else {
        asset["show"] = title["title"].clone();
    }
    asset
}

/// Records kept as JSON files by earlier builds move into the database, once.
pub fn import_records(db: &Database, data: &Path) -> Result<usize> {
    let Ok(entries) = std::fs::read_dir(directory(data)) else { return Ok(0) };
    let mut moved = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(file) = path.file_name().and_then(|n| n.to_str()) else { continue };
        if file.ends_with(".json.tmp") { let _ = std::fs::remove_file(&path); continue; }
        let Some(name) = file.strip_suffix(".json") else { continue };
        let (stem, kind) = name.split_once('.').unwrap_or((name, ""));
        if stem.len() != 64 || !stem.bytes().all(|b| b.is_ascii_hexdigit()) || !["", "en-opensubtitles", "zh-opensubtitles"].contains(&kind) { continue; }
        let document: Value = match std::fs::read(&path).map_err(anyhow::Error::from).and_then(|b| Ok(serde_json::from_slice(&b)?)) {
            Ok(document) => document,
            Err(error) => { tracing::warn!(file, %error, "Subtitle record unreadable; left in place"); continue }
        };
        if load(db, name).is_none() { store(db, name, &document)?; }
        std::fs::remove_file(&path)?;
        moved += 1;
    }
    Ok(moved)
}

/// Whether transcription already reached a verdict (transcript or rejection)
/// for this file revision.
pub fn decided(db: &Database, asset: &str, revision: &str) -> bool {
    load(db, &key(db, asset, revision)).is_some_and(|saved| untried(&saved).is_empty())
}
/// What a file has yet to be tried with. Records from when only English speech was
/// transcribed carry no list: a rejection then had tried English alone, which is how
/// every Mandarin title came to have no subtitles. A Mandarin transcript made before
/// the picture was read is redone from the picture, which is far more accurate, and so is
/// an English one when the picture holds subtitles (the audio is only ever a last resort).
fn untried(saved: &Value) -> Vec<&'static str> {
    let listed = saved["tried"].as_array().map(|list| list.iter().filter_map(Value::as_str).collect::<Vec<_>>());
    let tried: Vec<&str> = match (saved["state"].as_str(), listed) {
        (_, Some(list)) => list,
        (Some("ready"), None) if saved["language"] == "zh-Hans" => vec!["zh-Hans", "en"],
        // An English transcript made before the picture was looked at: look at it now.
        (Some("ready"), None) => vec!["en"],
        (_, None) if saved["reason"] == NO_AUDIO => vec!["en", "zh-Hans"],
        (_, None) => vec!["en"],
    };
    let mut tried = tried;
    // OpenSubtitles could not be asked last time: not yet due again.
    if saved["opensubtitles_after"].as_f64().is_some_and(|t| t > now()) && !tried.contains(&OPENSUBTITLES) { tried.push(OPENSUBTITLES); }
    if saved["state"] == "rejected" && saved["reason"] == NO_AUDIO && saved["audio_decoder"].as_u64() != Some(AUDIO_DECODER) {
        tried.retain(|step| !LANGUAGES.contains(step));
    }
    if saved["state"] == "ready" {
        // Subtitles people wrote, or read from the picture, stay. A transcript of the audio,
        // English or Mandarin, gives way to OpenSubtitles' English when it has some (the
        // user's rule, 2026-11-15: transcripts are poor); its Chinese is then translated on
        // device. No transcript was translated by the external fleet, whose work stays.
        if saved["method"] == OPENSUBTITLES || saved["method"] == "burned" { return Vec::new(); }
        let mut left = Vec::new();
        if !tried.contains(&PICTURE) { left.push(PICTURE); }
        if !tried.contains(&OPENSUBTITLES) { left.push(OPENSUBTITLES); }
        return left;
    }
    STEPS.into_iter().filter(|step| !tried.contains(step)).collect()
}
/// Chinese in the file's name: try Mandarin first.
fn chinese_name(input: &Path) -> bool {
    input.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)))
}
/// Assets with an accepted transcript of any version.
pub fn transcribed_assets(db: &Database) -> Result<std::collections::HashSet<String>> {
    // Transcript records only: the OpenSubtitles records also say "ready", and counting them
    // sent every title with OpenSubtitles' English back to be transcribed, again and again (2026-11-16).
    let mut statement = db.connection.prepare("SELECT asset_id FROM subtitle_records WHERE kind='transcript' AND asset_id IS NOT NULL AND json_extract(document,'$.state')='ready'")?;
    let ids = statement.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(ids)
}
/// The name of a file's transcript record.
pub(crate) fn record(db: &Database, asset: &str, revision: &str) -> String { key(db, asset, revision) }

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
    let (name, stored) = { let db = database(s)?; let name = key(&db, id, &revision); let stored = load(&db, &name); (name, stored) };
    let language_of = |saved: &Value| LANGUAGES.into_iter().find(|l| saved["language"] == *l).unwrap_or("en");
    // Subtitles printed in the picture always come first; the audio is the last resort,
    // transcribed only when the picture holds none. A Chinese name tries Mandarin speech
    // before English, any other name English first.
    let mut remaining: Vec<&'static str> = if chinese_name(Path::new(asset["path"].as_str().unwrap_or_default())) || chinese_name(input) { STEPS.to_vec() } else { vec![OPENSUBTITLES, PICTURE, "en", "zh-Hans"] };
    let mut tried: Vec<&'static str> = Vec::new();
    // A transcript already made, kept if the picture turns out to hold no subtitles.
    let mut kept: Option<(Value, PathBuf, &'static str)> = None;
    if let Some(saved) = stored {
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
    let save = |document: &Value| -> Result<()> { store(&*database(s)?, &name, document) };
    // What the record says while nothing has been accepted: the kept transcript, or a rejection.
    let unsettled = |tried: &[&str], reason: &str, stats: &Value| -> Value {
        match &kept {
            Some((saved, _, _)) => { let mut saved = saved.clone(); saved["tried"] = json!(tried); saved }
            None => json!({"state":"rejected","reason":reason,"tried":tried,"asset_id":id,"stats":stats,"audio_decoder":AUDIO_DECODER,"updated":now()}),
        }
    };
    // The audio, prepared when the first speech step needs it: (what the helper reads,
    // the track's first timestamp, a temporary file to remove).
    let mut audio: Option<(String, u64, Option<PathBuf>)> = None;
    let mut finding: Option<Result<Transcript>> = None;
    // When OpenSubtitles could not be asked now, when to ask again.
    let mut opensubtitles_after: Option<f64> = None;
    let mut reason = NOT_SPEECH.to_owned();
    for step in remaining {
        if tried.contains(&step) { continue; }
        if step == OPENSUBTITLES {
            // A limit or a network failure is an error: the file waits and tries again,
            // rather than being transcribed when people's subtitles may exist.
            match crate::opensubtitles::find(&s.data_dir, &titled(s, asset), input, &root, &name).await {
                Ok(crate::opensubtitles::Found::Ready(path, provenance)) => {
                    tried.push(step);
                    let file = path.file_name().and_then(|n| n.to_str()).context("Invalid subtitle path")?.to_owned();
                    save(&json!({"state":"ready","language":"en","method":OPENSUBTITLES,"output":file,"tried":tried,"asset_id":id,"audio_decoder":AUDIO_DECODER,"provenance":provenance,"updated":now()}))?;
                    finding = Some(Ok(Transcript::Ready(path, "en")));
                    break;
                }
                // No file to keep was named, so there is none.
                Ok(crate::opensubtitles::Found::Kept) => { tried.push(step); continue; }
                Ok(crate::opensubtitles::Found::Missing(why)) => {
                    tracing::info!(asset = id, reason = %why, "No subtitles from OpenSubtitles");
                    tried.push(step);
                    save(&unsettled(&tried, &reason, &Value::Null))?;
                    continue;
                }
                Err(error) => {
                    // The day's downloads used up, or the site unreachable: go on with what the
                    // title has (or the picture and the audio) so it gets Chinese now, and try
                    // OpenSubtitles again later; its English then replaces this and the Chinese
                    // is translated again (the user's rule, 2026-11-15).
                    let message = error.to_string();
                    let until = crate::opensubtitles::waits_until(&message).unwrap_or(now() + 3600.0);
                    tracing::info!(asset = id, %message, "OpenSubtitles later; going on without it");
                    opensubtitles_after = Some(until);
                    continue;
                }
            }
        }
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
                    // Frames copied as they are where AVFoundation decodes them (AAC, AC-3,
                    // E-AC-3); otherwise (DTS, TrueHD, FLAC, Opus…, or a file the Matroska
                    // reader trips on) the bundled ffmpeg decodes the track to PCM.
                    let copied = tokio::task::spawn_blocking(move || extract_audio(&source, &directory, &base)).await?;
                    let found = match copied {
                        Ok(Some(found)) => Some(found),
                        Ok(None) => decode_audio(input, &root, &name).await?,
                        Err(error) => {
                            tracing::info!(asset = id, %error, "Audio copy failed; decoding the audio instead");
                            decode_audio(input, &root, &name).await?
                        }
                    };
                    match found {
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
        save(&json!({"state":"ready","language":language,"method":if step == PICTURE {"burned"} else {"speech"},"output":file,"tried":tried,"asset_id":id,"stats":stats,"cues":cues.len(),"audio_decoder":AUDIO_DECODER,"updated":now()}))?;
        tracing::info!(asset = id, %stats, cues = cues.len(), "Transcription finished");
        finding = Some(Ok(Transcript::Ready(root.join(file), language)));
        break;
    }
    if let Some((_, _, Some(path))) = &audio { let _ = std::fs::remove_file(path); }
    if let Some(until) = opensubtitles_after {
        let saved = load(&*database(s)?, &name);
        if let Some(mut saved) = saved {
            saved["opensubtitles_after"] = json!(until);
            save(&saved)?;
        }
    }
    match (finding, kept) {
        (Some(finding), _) => finding,
        (None, Some((_, output, language))) => Ok(Transcript::Ready(output, language)),
        (None, None) => Ok(Transcript::Rejected(reason)),
    }
}

/// Chinese subtitles people wrote, from OpenSubtitles, for a title whose Chinese would
/// otherwise be translated on device (while the account's `chinese` switch is on). The
/// answer is kept per file, so a title is looked up once; when OpenSubtitles cannot be
/// asked now, the translation goes ahead and the title is looked up again after the reset.
pub async fn people_chinese(s: &Service, asset: &Value, input: &Path) -> Result<Option<PathBuf>> {
    if !crate::opensubtitles::chinese(&s.data_dir) { return Ok(None); }
    people(s, asset, input, "zh-cn").await
}

/// English subtitles people wrote, from OpenSubtitles, for a title whose English the server
/// made itself (read from a bitmap track, or translated from Chinese read from the picture).
/// Looked up once per file, like `people_chinese`.
pub async fn people_english(s: &Service, asset: &Value, input: &Path) -> Result<Option<PathBuf>> {
    if !crate::opensubtitles::read_english(&s.data_dir) { return Ok(None); }
    people(s, asset, input, "en").await
}

/// OpenSubtitles' subtitles in `language` ("en" or "zh-cn") for any title, while every title
/// is to take them (`opensubtitles::everything`). Looked up once per file, like the others.
pub async fn people_everywhere(s: &Service, asset: &Value, input: &Path, language: &str) -> Result<Option<PathBuf>> {
    if !crate::opensubtitles::everything(&s.data_dir) { return Ok(None); }
    people(s, asset, input, language).await
}

/// Whether OpenSubtitles has been asked for this file's subtitles in `tag` ("en" or "zh").
pub fn people_decided(db: &Database, asset: &str, revision: &str, tag: &str) -> bool {
    load(db, &format!("{}.{tag}-opensubtitles", key(db, asset, revision))).is_some()
}

/// Whether OpenSubtitles gave an answer that stands for this file in `tag`: subtitles, or none
/// suitable; or it could not be asked and the time to ask again has not come (the title was
/// reopened on every pass meanwhile, and Spirited Away's picture was read and failed 119 times).
/// Not when its file was refused for cues out of place.
/// Subtitles taken before they were checked against the file's length are not settled.
/// `everything`: opensubtitles::everything, read once by the caller.
pub fn people_settled(db: &Database, everything: bool, asset: &str, revision: &str, tag: &str, special: bool) -> bool {
    load(db, &format!("{}.{tag}-opensubtitles", key(db, asset, revision))).is_some_and(|saved|
        (saved["state"] == "ready" && fit_checked(&saved) && !rechoose(&saved)) || (saved["state"] == "later" && saved["after"].as_f64().is_some_and(|t| t > now()))
            || (saved["state"] == "missing" && !(everything && saved["set_aside"] == true)
            && !saved["reason"].as_str().is_some_and(|r| r.ends_with("Subtitle cues are out of order"))
            && !ask_again(&saved, special)))
}

/// The OpenSubtitles lookup version a missing record was made by. Version 3 looks specials up
/// by their name (a pack's S00E01 is seldom TMDB's), and reads files with a cue that ends
/// before it starts: records from before that, for those, are asked again. Version 4 takes
/// Traditional Chinese (converted to Simplified) when there is no Simplified: Chinese records
/// with nothing found before it are asked again, once. Version 5 searches by name when
/// OpenSubtitles lacks the TMDB id (The Threesome, 2025, was transcribed from its audio while
/// OpenSubtitles had administrator uploads): every record with nothing found is asked again, once.
const LOOKUP: u64 = 5;
fn ask_again(saved: &Value, special: bool) -> bool {
    let lookup = saved["lookup"].as_u64().unwrap_or(0);
    let reason = saved["reason"].as_str().unwrap_or("");
    saved["state"] == "missing" && lookup < LOOKUP
        && ((lookup < 3 && (special || reason.ends_with("Subtitle ends before it starts")))
            || (lookup < 4 && reason.starts_with("No zh-Hans subtitles on OpenSubtitles"))
            || (reason.starts_with("No en subtitles on OpenSubtitles") || reason.starts_with("No zh-Hans subtitles on OpenSubtitles")))
}

/// A ready record chosen before ranking version 2 (any hash match first, so a careless upload
/// beat the administrator's from the disc: The Office): looked up again once, keeping it when
/// it still ranks first among those that fit (nothing downloaded) or nothing better is found.
fn rechoose(saved: &Value) -> bool {
    saved["state"] == "ready" && saved["provenance"]["source"] == "opensubtitles"
        && match saved["provenance"]["ranking"].as_u64().unwrap_or(1) {
            // Ranking 3 timed by shifting only: what it could not line up is asked again.
            3 => !saved["provenance"]["better_text_out_of_time"].is_null(),
            ranking => ranking < crate::opensubtitles::RANKING,
        }
        && saved["after"].as_f64().is_none_or(|t| t <= now())
}

/// Whether a ready record's subtitles were checked against the file's length.
fn fit_checked(saved: &Value) -> bool { saved["provenance"]["fits"] == true }

/// Leave a file out of the OpenSubtitles lookup for now, saying why; a later round removes
/// these records to look again.
pub fn set_aside(s: &Service, asset: &Value, input: &Path, tag: &str, reason: &str) -> Result<()> {
    let id = asset["id"].as_str().context("Missing asset ID")?;
    let revision = crate::subtitle_jobs::revision(input)?;
    let db = database(s)?;
    let name = format!("{}.{tag}-opensubtitles", key(&db, id, &revision));
    if load(&db, &name).is_none() { store(&db, &name, &json!({"state":"missing","asset_id":id,"reason":reason,"set_aside":true,"updated":now()}))?; }
    Ok(())
}

async fn people(s: &Service, asset: &Value, input: &Path, language: &str) -> Result<Option<PathBuf>> {
    let tag = if language == "en" { "en" } else { "zh" };
    let id = asset["id"].as_str().context("Missing asset ID")?;
    let revision = crate::subtitle_jobs::revision(input)?;
    let root = directory(&s.data_dir);
    std::fs::create_dir_all(&root)?;
    let (name, stored) = { let db = database(s)?; let name = key(&db, id, &revision); let stored = load(&db, &format!("{name}.{tag}-opensubtitles")); (name, stored) };
    let record = format!("{name}.{tag}-opensubtitles");
    let save = |document: Value| -> Result<()> { store(&*database(s)?, &record, &document) };
    let mut refused: Vec<u64> = Vec::new();
    // An anonymous upload chosen by the old ranking: asked again, and kept unless a better one
    // is found (or OpenSubtitles cannot be asked now).
    if let Some(saved) = stored.as_ref().filter(|saved| rechoose(saved)) {
        let kept = root.join(saved["output"].as_str().unwrap_or_default());
        if kept.is_file() {
            let mut saved = saved.clone();
            // Kept as it is only when its timing is known to be this file's (hash-matched, or
            // already lined up with the hash-matched upload).
            let in_time = saved["provenance"]["moviehash_match"] == true || !saved["provenance"]["timed_by"].is_null();
            let keep = saved["provenance"]["file_id"].as_u64().filter(|_| in_time);
            match crate::opensubtitles::find_language(&s.data_dir, &titled(s, asset), input, &root, &name, language, &[], keep).await {
                Ok(crate::opensubtitles::Found::Ready(path, provenance)) => {
                    tracing::info!(asset = id, tag, was = saved["provenance"]["release"].as_str().unwrap_or(""), now = provenance["release"].as_str().unwrap_or(""), "OpenSubtitles chosen again by the new ranking");
                    save(json!({"state":"ready","asset_id":id,"output":path.file_name().and_then(|n| n.to_str()),"provenance":provenance,"updated":now()}))?;
                    return Ok(Some(path));
                }
                Ok(crate::opensubtitles::Found::Missing(_) | crate::opensubtitles::Found::Kept) => { saved["provenance"]["ranking"] = json!(crate::opensubtitles::RANKING); }
                Err(error) => { saved["after"] = json!(crate::opensubtitles::waits_until(&error.to_string()).unwrap_or(now() + 3600.0)); }
            }
            save(saved)?;
            return Ok(Some(kept));
        }
    }
    if let Some(mut saved) = stored {
        if saved["state"] == "ready" {
            let path = root.join(saved["output"].as_str().unwrap_or_default());
            if path.is_file() {
                if fit_checked(&saved) { return Ok(Some(path)); }
                // Taken before subtitles were checked against the file's length: checked now
                // (BSG's Miniseries part 1 had another special's).
                let (checked, file) = (path.clone(), input.to_owned());
                let (fits, last_end, duration) = tokio::task::spawn_blocking(move || {
                    let last_end = std::fs::read_to_string(&checked).ok().and_then(|t| crate::subtitles::parse_srt(&t).ok())
                        .map(|cues| cues.iter().map(|c| c.end_ms).max().unwrap_or(0) as f64 / 1000.0).unwrap_or(0.0);
                    let duration = crate::media_format::duration(&file);
                    (crate::opensubtitles::fits(last_end, duration), last_end, duration)
                }).await?;
                if fits {
                    saved["provenance"]["fits"] = json!(true);
                    save(saved)?;
                    return Ok(Some(path));
                }
                tracing::info!(asset = id, tag, last_end, duration, release = saved["provenance"]["release"].as_str().unwrap_or(""), "OpenSubtitles file does not fit this file's length; looking again");
                refused.extend(saved["provenance"]["file_id"].as_u64());
                let _ = std::fs::remove_file(&path);
            }
        }
        // Lookups before version 2 never matched Chinese ("zh-CN" against "zh-cn"): asked again.
        // Files refused for cues out of place are now put in order (Creed's Chinese): asked again.
        let unordered = saved["reason"].as_str().is_some_and(|r| r.ends_with("Subtitle cues are out of order"));
        // Set aside (fleet translations, for a later round): that round is every title.
        let set_aside = saved["set_aside"] == true && !crate::opensubtitles::everything(&s.data_dir);
        let special = asset["season"].as_u64() == Some(0);
        if saved["state"] == "missing" && !unordered && !ask_again(&saved, special) && (saved["lookup"].as_u64() >= Some(2) || set_aside) { return Ok(None); }
        if saved["after"].as_f64().is_some_and(|t| t > now()) { return Ok(None); }
    }
    match crate::opensubtitles::find_language(&s.data_dir, &titled(s, asset), input, &root, &name, language, &refused, None).await {
        Ok(crate::opensubtitles::Found::Ready(path, provenance)) => {
            save(json!({"state":"ready","asset_id":id,"output":path.file_name().and_then(|n| n.to_str()),"provenance":provenance,"updated":now()}))?;
            Ok(Some(path))
        }
        Ok(crate::opensubtitles::Found::Kept) => Ok(None), // No file to keep was named.
        Ok(crate::opensubtitles::Found::Missing(why)) => {
            save(json!({"state":"missing","asset_id":id,"reason":why,"lookup":LOOKUP,"updated":now()}))?;
            Ok(None)
        }
        Err(error) => {
            let message = error.to_string();
            let after = crate::opensubtitles::waits_until(&message).unwrap_or(now() + 3600.0);
            tracing::info!(asset = id, language, %message, "OpenSubtitles later; going on with what the title has");
            save(json!({"state":"later","asset_id":id,"after":after,"reason":message,"updated":now()}))?;
            Ok(None)
        }
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
        // Finish when the helper exits; stop when playback preparation starts. Its progress is
        // read every 2 s only while someone has the Processing page open.
        let mut preparation = crate::wake::PREPARATION.subscribe();
        let status = loop {
            let watched = crate::wake::progress_watched();
            tokio::select! {
                status = child.wait() => break status?,
                _ = preparation.changed() => {}
                () = async { if watched { tokio::time::sleep(Duration::from_secs(2)).await } else { std::future::pending().await } } => {}
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
/// The audio track decoded by the bundled ffmpeg to 16 kHz mono PCM (WAV), for codecs
/// AVFoundation cannot decode and Matroska files the packet reader cannot walk. The track is
/// chosen as `extract_audio` chooses (English, else the first); the cue offset is the track's
/// start time, as the frames' first timestamp is there. None: no audio track, or nothing decoded.
async fn decode_audio(input: &Path, directory: &Path, name: &str) -> Result<Option<(PathBuf, u64)>> {
    decode_audio_with(input, directory, name, &std::env::current_exe()?).await
}
async fn decode_audio_with(input: &Path, directory: &Path, name: &str, bin: &Path) -> Result<Option<(PathBuf, u64)>> {
    let (ffmpeg, ffprobe) = (bin.with_file_name("torrbaby-ffmpeg"), bin.with_file_name("torrbaby-ffprobe"));
    if !ffmpeg.is_file() || !ffprobe.is_file() { return Ok(None); }
    let probe = tokio::time::timeout(Duration::from_secs(60), tokio::process::Command::new(&ffprobe)
        .args(["-v", "error", "-select_streams", "a", "-show_entries", "stream=index,start_time:stream_tags=language", "-of", "json"])
        .arg(input).stdin(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true).output()).await.context("Audio probe timed out")??;
    if !probe.status.success() { return Ok(None); }
    let streams: Value = serde_json::from_slice(&probe.stdout)?;
    let streams = streams["streams"].as_array().cloned().unwrap_or_default();
    let english = |s: &&Value| s["tags"]["language"].as_str().is_some_and(|l| { let l = l.to_ascii_lowercase(); l == "en" || l == "eng" || l.starts_with("en-") });
    let Some(position) = streams.iter().position(|s| english(&s)).or((!streams.is_empty()).then_some(0)) else { return Ok(None) };
    let start = streams[position]["start_time"].as_str().and_then(|t| t.parse::<f64>().ok()).filter(|t| t.is_finite() && *t > 0.0).unwrap_or(0.0);
    let path = directory.join(format!("{name}.audio.wav"));
    let temporary = directory.join(format!("{name}.audio.wav.tmp"));
    let status = tokio::time::timeout(Duration::from_secs(3 * 3600), tokio::process::Command::new(&ffmpeg)
        .args(["-v", "error", "-nostdin", "-y", "-i"]).arg(input)
        .args(["-map", &format!("0:a:{position}"), "-vn", "-sn", "-dn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le", "-f", "wav"])
        .arg(&temporary).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true).status()).await.context("Audio decoding timed out")??;
    // 16 kHz mono 16-bit: a minute of audio is 1.9 MB; less than a second means nothing decoded.
    if !status.success() || std::fs::metadata(&temporary).map_or(0, |m| m.len()) < 32_000 {
        let _ = std::fs::remove_file(&temporary);
        return Ok(None);
    }
    std::fs::rename(&temporary, &path)?;
    Ok(Some((path, (start * 1000.0).round() as u64)))
}

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
        assert_eq!(untried(&json!({"state":"rejected","reason":"The audio is not English speech"})), ["picture", "opensubtitles", "zh-Hans"]);
        // No audio to decode says nothing about the picture.
        assert_eq!(untried(&json!({"state":"rejected","reason":"No decodable audio track","audio_decoder":AUDIO_DECODER})), ["picture", "opensubtitles"]);
        // A transcript gives way to OpenSubtitles; the picture's Chinese does not.
        assert_eq!(untried(&json!({"state":"ready","language":"en","method":"speech","tried":["picture","en"]})), ["opensubtitles"]);
        assert_eq!(untried(&json!({"state":"ready","language":"zh-Hans","method":"speech","tried":["picture","zh-Hans"]})), ["opensubtitles"]);
        assert!(untried(&json!({"state":"ready","language":"zh-Hans","method":"burned","tried":["picture"]})).is_empty());
        assert!(untried(&json!({"state":"ready","language":"en","method":"opensubtitles","tried":["opensubtitles"]})).is_empty());
        // Downloads used up: asked again only after the reset.
        assert!(untried(&json!({"state":"ready","language":"en","method":"speech","tried":["picture","en"],"opensubtitles_after":now()+3600.0})).is_empty());
        assert_eq!(untried(&json!({"state":"ready","language":"en","method":"speech","tried":["picture","en"],"opensubtitles_after":now()-1.0})), ["opensubtitles"]);
        // Rejected before ffmpeg decoded what AVFoundation cannot: the speech is tried again.
        assert_eq!(untried(&json!({"state":"rejected","reason":"No decodable audio track","tried":["picture","en","zh-Hans"]})), ["opensubtitles","zh-Hans","en"]);
        assert_eq!(untried(&json!({"state":"rejected","reason":NOT_SPEECH,"tried":["picture","zh-Hans","en"]})), ["opensubtitles"]);
        assert!(untried(&json!({"state":"rejected","reason":NOT_SPEECH,"tried":["picture","opensubtitles","zh-Hans","en"]})).is_empty());
        assert_eq!(untried(&json!({"state":"rejected","reason":NOT_SPEECH,"tried":["zh-Hans","en"]})), ["picture","opensubtitles"]);
        assert_eq!(untried(&json!({"state":"ready","language":"zh-Hans","tried":["en","burned","zh-Hans"]})), ["picture","opensubtitles"], "a verdict from before the picture could be found");
        // A transcript made before the picture was looked at is redone from the picture once,
        // Mandarin or English: the audio is only ever a last resort.
        assert_eq!(untried(&json!({"state":"ready","language":"zh-Hans"})), ["picture","opensubtitles"]);
        assert_eq!(untried(&json!({"state":"ready","language":"zh-Hans","tried":["zh-Hans","picture"]})), ["opensubtitles"]);
        assert_eq!(untried(&json!({"state":"ready"})), ["picture","opensubtitles"]);
        assert_eq!(untried(&json!({"state":"ready","language":"en"})), ["picture","opensubtitles"]);
        assert_eq!(untried(&json!({"state":"ready","language":"en","tried":["picture","en"]})), ["opensubtitles"]);
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
