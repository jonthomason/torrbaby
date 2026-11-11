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
const VERSION: u64 = 1;
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
    // Matroska goes through the analysis index as a virtual MP4, like credits.
    let mut server = None;
    let media = if crate::container::is_matroska(input) {
        let job = s.virtual_media.prepare_analysis(id, input.to_path_buf())?;
        if job["state"] == "failed" { anyhow::bail!("{}", job["error"].as_str().unwrap_or("Media index failed")); }
        if job["state"] != "ready" { return Ok(Transcript::Deferred); }
        let reader = s.virtual_media.analysis_reader(job["id"].as_str().context("Missing index")?, id, input).await?;
        let (url, task) = crate::credits::serve(reader).await?;
        server = Some(task);
        url
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
        if status.code() == Some(3) { return Ok(Some(Err(error))); }
        ensure!(status.success(), "Speech transcription: {}", if error.is_empty() { "helper failed" } else { &error });
        let bytes = std::fs::read(&result_path)?;
        ensure!(bytes.len() <= 16_777_216, "Transcript too large");
        Ok(Some(Ok(serde_json::from_slice::<Value>(&bytes)?)))
    }.await;
    if let Some(task) = server { task.abort(); }
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
    let cues: Vec<crate::subtitles::TimedCue> = serde_json::from_value(result["cues"].clone())?;
    ensure!(!cues.is_empty() && cues.iter().all(|c| c.end_ms > c.start_ms && !c.text.trim().is_empty() && c.text.len() <= 16_384),
        "Invalid speech transcript");
    let temporary = root.join(format!("{name}.en.srt.tmp"));
    std::fs::write(&temporary, srt(&cues))?;
    std::fs::rename(temporary, &output)?;
    save(&json!({"state":"ready","asset_id":id,"stats":stats,"cues":cues.len(),"updated":now()}))?;
    tracing::info!(asset = id, %stats, cues = cues.len(), "Speech transcription finished");
    Ok(Transcript::Ready(output))
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
