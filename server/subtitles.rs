//! Apple Translation process boundary. Timings never cross this boundary.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, path::Path, process::Stdio, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Serialize, Deserialize)]
pub struct Cue {
    pub id: String,
    pub text: String,
}
#[derive(Serialize, Deserialize)]
pub struct TranslationRequest {
    pub source: String,
    pub target: String,
    pub cues: Vec<Cue>,
}
#[derive(Deserialize, Serialize)]
pub struct TranslationResponse {
    pub version: u32,
    pub cues: Option<Vec<Cue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub async fn translate(helper: &Path, request: TranslationRequest) -> Result<TranslationResponse> {
    ensure!(
        ["en", "zh-Hans"].contains(&request.source.as_str())
            && ["en", "zh-Hans"].contains(&request.target.as_str())
            && request.source != request.target,
        "Unsupported translation pair"
    );
    let ids: HashSet<_> = request.cues.iter().map(|cue| cue.id.as_str()).collect();
    ensure!(
        !request.cues.is_empty()
            && request.cues.len() <= 128
            && ids.len() == request.cues.len()
            && request.cues.iter().all(|cue| !cue.id.is_empty()
                && cue.id.len() <= 128
                && !cue.text.trim().is_empty()
                && cue.text.len() <= 16_384),
        "Invalid subtitle batch"
    );
    let input = serde_json::to_vec(&request)?;
    ensure!(input.len() <= 1_048_576, "Subtitle batch too large");
    let mut child = tokio::process::Command::new(helper)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("Start subtitle translator")?;
    let mut stdin = child.stdin.take().context("Translator stdin unavailable")?;
    let stdout = child
        .stdout
        .take()
        .context("Translator stdout unavailable")?;
    let outcome = tokio::time::timeout(Duration::from_secs(120), async {
        let write = async {
            stdin.write_all(&input).await?;
            stdin.shutdown().await?;
            drop(stdin);
            Ok::<_, std::io::Error>(())
        };
        let read = async {
            let mut output = Vec::new();
            stdout.take(4_194_305).read_to_end(&mut output).await?;
            ensure!(output.len() <= 4_194_304, "Translator response too large");
            Ok::<_, anyhow::Error>(output)
        };
        let (_, output) =
            tokio::try_join!(async { write.await.map_err(anyhow::Error::from) }, read)?;
        let status = child.wait().await?;
        let response: TranslationResponse =
            serde_json::from_slice(&output).context("Invalid translator response")?;
        ensure!(response.version == 1, "Unsupported translator protocol");
        if !status.success() || response.error.is_some() {
            // Whitelist codes: never reflect arbitrary subprocess output in logs.
            let code = match response.error.as_deref() {
                Some("models_not_installed") => "models_not_installed",
                Some("invalid_request") => "invalid_request",
                _ => "translation_failed",
            };
            anyhow::bail!("Subtitle translation: {code}");
        }
        let cues = response.cues.as_ref().context("Missing translated cues")?;
        ensure!(
            cues.len() == request.cues.len()
                && cues
                    .iter()
                    .zip(&request.cues)
                    .all(|(actual, expected)| actual.id == expected.id
                        && !actual.text.trim().is_empty()),
            "Translator changed cue identities or returned empty text"
        );
        Ok(response)
    })
    .await;
    match outcome {
        Ok(Ok(response)) => Ok(response),
        failure => {
            // Explicitly kill and reap on errors; kill_on_drop also covers task cancellation.
            let _ = child.kill().await;
            match failure {
                Ok(Err(error)) => Err(error),
                _ => anyhow::bail!("Subtitle translation timed out"),
            }
        }
    }
}

/// A normalized text cue. Integer milliseconds avoid timestamp drift in translation.
pub struct TimedCue {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

pub fn parse_srt(input: &str) -> Result<Vec<TimedCue>> {
    ensure!(
        input.len() <= 8_388_608 && !input.contains('\0'),
        "Invalid or oversized subtitle file"
    );
    let normalized = input
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let formatting = regex::Regex::new(r"(?i)</?(?:i|b|u|font)(?:\s[^>]*)?>").unwrap();
    let mut cues = Vec::new();
    let mut block = Vec::new();
    for line in normalized.lines().chain(std::iter::once("")) {
        if !line.trim().is_empty() {
            block.push(line);
            continue;
        }
        if block.is_empty() {
            continue;
        }
        ensure!(cues.len() < 100_000, "Too many subtitle cues");
        let timing_index = if block[0].contains("-->") { 0 } else { 1 };
        ensure!(block.len() > timing_index + 1, "Subtitle cue has no text");
        if timing_index == 1 {
            ensure!(
                block[0].trim().bytes().all(|c| c.is_ascii_digit()),
                "Invalid SRT cue number"
            );
        }
        let (start, end) = block[timing_index]
            .split_once("-->")
            .context("Missing subtitle timing")?;
        let start_ms = parse_timestamp(start.trim())?;
        let end_ms = parse_timestamp(end.trim())?;
        ensure!(end_ms > start_ms, "Subtitle ends before it starts");
        if let Some(previous) = cues.last() {
            let previous: &TimedCue = previous;
            ensure!(
                start_ms >= previous.start_ms,
                "Subtitle cues are out of order"
            );
        }
        let raw = block[timing_index + 1..].join("\n");
        ensure!(raw.len() <= 16_384, "Subtitle cue too large");
        // Flatten common formatting before translation. Unknown markup remains
        // literal text and is escaped when writing WebVTT, never executed as markup.
        let text = formatting
            .replace_all(&raw, "")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&nbsp;", " ")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&");
        ensure!(!text.trim().is_empty(), "Empty subtitle text");
        cues.push(TimedCue {
            start_ms,
            end_ms,
            text,
        });
        block.clear();
    }
    ensure!(!cues.is_empty(), "No subtitle cues");
    Ok(cues)
}

fn parse_timestamp(value: &str) -> Result<u64> {
    let clock = value.as_bytes();
    ensure!(
        clock.len() >= 12
            && clock[clock.len() - 4..clock.len() - 3]
                .iter()
                .all(|c| *c == b',' || *c == b'.')
            && clock[clock.len() - 7] == b':'
            && clock[clock.len() - 10] == b':',
        "Invalid subtitle timestamp"
    );
    let parts: Vec<_> = value.split([':', ',', '.']).collect();
    ensure!(
        parts.len() == 4
            && parts[0].len() >= 2
            && parts[0].len() <= 6
            && parts[1].len() == 2
            && parts[2].len() == 2
            && parts[3].len() == 3
            && parts.iter().all(|p| p.bytes().all(|c| c.is_ascii_digit())),
        "Invalid subtitle timestamp"
    );
    let values: Vec<u64> = parts.iter().map(|p| p.parse()).collect::<Result<_, _>>()?;
    ensure!(
        values[1] < 60 && values[2] < 60,
        "Invalid subtitle timestamp"
    );
    Ok(((values[0] * 60 + values[1]) * 60 + values[2]) * 1000 + values[3])
}

pub fn webvtt(cues: &[TimedCue]) -> String {
    fn time(ms: u64) -> String {
        format!(
            "{:02}:{:02}:{:02}.{:03}",
            ms / 3_600_000,
            ms / 60_000 % 60,
            ms / 1000 % 60,
            ms % 1000
        )
    }
    let mut output = String::from("WEBVTT\n\n");
    for (index, cue) in cues.iter().enumerate() {
        // Blank lines delimit WebVTT cues; keep translated paragraph breaks inside
        // a cue from accidentally creating new cue blocks.
        let text = cue
            .text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n")
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        output.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            index + 1,
            time(cue.start_ms),
            time(cue.end_ms),
            text
        ));
    }
    output
}

pub async fn translate_cues(
    helper: &Path,
    source: &str,
    target: &str,
    cues: &mut [TimedCue],
) -> Result<()> {
    // Small batches leave room for JSON escaping and framework output expansion.
    for batch in cues.chunks_mut(8) {
        let request = TranslationRequest {
            source: source.into(),
            target: target.into(),
            cues: batch
                .iter()
                .enumerate()
                .map(|(i, cue)| Cue {
                    id: i.to_string(),
                    text: cue.text.clone(),
                })
                .collect(),
        };
        let response = translate(helper, request).await?;
        for (cue, translated) in batch
            .iter_mut()
            .zip(response.cues.context("Missing translated cues")?)
        {
            cue.text = translated.text;
        }
    }
    Ok(())
}
