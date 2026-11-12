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
#[derive(Serialize, Deserialize)]
pub struct TimedCue {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

/// Whether cue text reads as SDH/closed captions: sound descriptions such as
/// "[door slams]" or "(GRUNTS)", or speaker labels such as "JERRY:". Most rips
/// drop the disc's SDH label, so the text is the only evidence. Measured on the
/// library: labelled SDH tracks mark 12-27% of cues, ordinary subtitles about 0%
/// (an occasional "NARRATOR:" stays under 1%).
pub fn describes_sounds<'a>(texts: impl IntoIterator<Item = &'a str>) -> bool {
    static SOUND: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"\[[^\]\n]{2,40}\]|(?m:^\s*-?\s*\([^)\n]{2,40}\))").expect("valid regex")
    });
    static SPEAKER: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?m)^\s*-?\s*[A-Z][A-Z0-9 .'\-]{1,24}:(\s|$)").expect("valid regex")
    });
    let (mut cues, mut hits, mut shouting) = (0usize, 0usize, 0usize);
    let mut speakers = 0usize;
    for text in texts {
        cues += 1;
        let letters = text.chars().filter(|c| c.is_alphabetic());
        let (upper, all) = letters.fold((0, 0), |(u, a), c| (u + c.is_uppercase() as usize, a + 1));
        if all >= 4 && upper * 10 >= all * 9 { shouting += 1; }
        if SOUND.is_match(text) { hits += 1; } else if SPEAKER.is_match(text) { speakers += 1; }
    }
    // All-caps subtitles ("NO, WAIT: ...") can look like speaker labels.
    if shouting * 2 < cues { hits += speakers; }
    hits >= 10 && hits * 1000 >= cues * 15
}

pub fn plain_text(raw: &str) -> String {
    static FORMATTING: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)</?(?:i|b|u|font)(?:\s[^>]*)?>").expect("valid regex")
    });
    FORMATTING
        .replace_all(raw, "")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
        .replace("\r\n", "\n")
        .replace('\r', "\n")
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
    // Some SRT exporters insert a blank line between a timestamp and its text.
    // Join it only when the following line is text, not another cue number.
    static GAP: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?m)(-->[^\n]+)\n(?:[ \t]*\n)+([ \t]*[^\d\s][^\n]*)").expect("valid regex")
    });
    let normalized = GAP.replace_all(&normalized, "$1\n$2");
    let mut cues = Vec::new();
    let mut seen = HashSet::new();
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
        // Disc rips often carry timed cues with no text (they clear the screen):
        // skip them rather than rejecting the whole track.
        if block.len() <= timing_index + 1 {
            block.clear();
            continue;
        }
        if timing_index == 1 {
            ensure!(
                block[0].trim().bytes().all(|c| c.is_ascii_digit()),
                "Invalid SRT cue number"
            );
        }
        let (start, end) = block[timing_index]
            .split_once("-->")
            .context("Missing subtitle timing")?;
        let end_ms = parse_timestamp(end.trim())?;
        let start_ms = parse_timestamp(start.trim()).or_else(|original| {
            // Some damaged SRT exports lose the leading hour/minute characters.
            // Recover only an exact suffix of a full clock, close to this cue's end.
            let start = start.trim();
            let end = end.trim();
            if start.len() >= 8 && start.len() < 12 && end.len() == 12
                && start.is_ascii() && end.is_ascii() {
                let candidate = format!("{}{}", &end[..12 - start.len()], start);
                if let Ok(time) = parse_timestamp(&candidate) {
                    if time < end_ms && end_ms - time <= 30_000 {
                        return Ok(time);
                    }
                }
            }
            Err(original)
        })?;
        ensure!(end_ms > start_ms, "Subtitle ends before it starts");
        let raw = block[timing_index + 1..].join("\n");
        ensure!(raw.len() <= 16_384, "Subtitle cue too large");
        // Flatten common formatting before translation. Unknown markup remains
        // literal text and is escaped when writing WebVTT, never executed as markup.
        let text = plain_text(&raw);
        if text.trim().is_empty() {
            block.clear();
            continue;
        }
        // Damaged exporters can append a repeated tail, including a truncated clock.
        // Ignore exact duplicates before enforcing chronological order.
        if !seen.insert((start_ms, end_ms, text.clone())) {
            block.clear();
            continue;
        }
        if let Some(previous) = cues.last() {
            let previous: &TimedCue = previous;
            ensure!(
                start_ms >= previous.start_ms,
                "Subtitle cues are out of order"
            );
        }
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

/// Amortize native session startup while retaining bounded JSON expansion.
pub fn translation_batch_len(cues: &[TimedCue]) -> usize {
    let mut bytes = 0;
    let count = cues.iter().take(32).take_while(|cue| {
        bytes += cue.text.len();
        bytes <= 128 * 1024
    }).count();
    // A batch does not end in the middle of a sentence that runs on into the next line.
    let mut end = count;
    while end < cues.len() && end < count + 4 && continues(&cues[end - 1], &cues[end]) { end += 1; }
    end
}

/// Whether a Chinese subtitle line runs on into the next: burned-in subtitles split one
/// sentence over short lines with no punctuation ("还指望我对你们赵家" / "感恩戴德吗"), a
/// second or so apart. A line ending in a full stop or a final particle (吗, 呢, 吧…) ends its sentence.
fn continues(line: &TimedCue, next: &TimedCue) -> bool {
    let text = line.text.trim();
    let han = |t: &str| t.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c));
    han(text) && han(&next.text)
        && !text.ends_with(['。', '！', '？', '!', '?', '…', '」', '”', '吗', '呢', '吧', '啊', '呀', '嘛', '哦', '啦', '罢'])
        && next.start_ms >= line.end_ms && next.start_ms - line.end_ms <= 1500
        && text.chars().count() + next.text.trim().chars().count() <= 40
}

/// Lines translated together: runs of up to four Chinese lines that form one sentence.
fn sentence_groups(cues: &[TimedCue], chinese: bool) -> Vec<std::ops::Range<usize>> {
    let mut groups = Vec::new();
    let mut start = 0;
    while start < cues.len() {
        let mut end = start + 1;
        while chinese && end < cues.len() && end - start < 4 && continues(&cues[end - 1], &cues[end]) { end += 1; }
        groups.push(start..end);
        start = end;
    }
    groups
}

/// A sentence's translation spread back over the lines it came from, in proportion to
/// each line's share of the original, broken between words. None when it will not divide.
fn split_translation(text: &str, weights: &[usize]) -> Option<Vec<String>> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() < weights.len() { return None; }
    let total: usize = weights.iter().sum::<usize>().max(1);
    let mut parts = Vec::new();
    let mut taken = 0;
    let mut share = 0;
    for (i, weight) in weights.iter().enumerate() {
        share += weight;
        let left = weights.len() - i - 1;
        let end = if left == 0 { words.len() } else {
            ((words.len() * share + total / 2) / total).clamp(taken + 1, words.len() - left)
        };
        parts.push(words[taken..end].join(" "));
        taken = end;
    }
    Some(parts)
}

pub async fn translate_cues(
    helper: &Path,
    source: &str,
    target: &str,
    cues: &mut [TimedCue],
) -> Result<()> {
    // Chinese lines that make one sentence are translated as one, then spread back
    // over their original timings; a line out of context translates badly.
    let groups = sentence_groups(cues, source.starts_with("zh"));
    let texts: Vec<String> = groups.iter().map(|group| {
        if group.len() == 1 { cues[group.start].text.clone() }
        else { cues[group.clone()].iter().map(|cue| cue.text.trim()).collect::<Vec<_>>().join("，") }
    }).collect();
    let mut translated: Vec<String> = Vec::with_capacity(texts.len());
    let mut at = 0;
    while at < texts.len() {
        let mut bytes = 0;
        let count = texts[at..].iter().take(32).take_while(|text| { bytes += text.len(); bytes <= 128 * 1024 }).count();
        ensure!(count > 0, "Subtitle cue exceeds translation batch limit");
        let request = TranslationRequest {
            source: source.into(),
            target: target.into(),
            cues: texts[at..at + count].iter().enumerate().map(|(i, text)| Cue { id: i.to_string(), text: text.clone() }).collect(),
        };
        let response = translate(helper, request).await?;
        let batch = response.cues.context("Missing translated cues")?;
        ensure!(batch.len() == count, "Missing translated cues");
        translated.extend(batch.into_iter().map(|cue| cue.text));
        at += count;
    }
    let mut alone: Vec<usize> = Vec::new();
    for (group, text) in groups.iter().zip(&translated) {
        if group.len() == 1 { cues[group.start].text = text.clone(); continue; }
        let weights: Vec<usize> = cues[group.clone()].iter().map(|cue| cue.text.trim().chars().count().max(1)).collect();
        match split_translation(text, &weights) {
            Some(parts) => for (cue, part) in cues[group.clone()].iter_mut().zip(parts) { cue.text = part; },
            None => alone.extend(group.clone()),
        }
    }
    // A sentence whose translation will not divide over its lines: each line on its own, as before.
    for chunk in alone.chunks(32) {
        let request = TranslationRequest {
            source: source.into(),
            target: target.into(),
            cues: chunk.iter().enumerate().map(|(i, &index)| Cue { id: i.to_string(), text: cues[index].text.clone() }).collect(),
        };
        let response = translate(helper, request).await?;
        for (&index, cue) in chunk.iter().zip(response.cues.context("Missing translated cues")?) { cues[index].text = cue.text; }
    }
    Ok(())
}

#[derive(Serialize)]
pub struct Sidecar {
    pub id: String,
    pub name: String,
    pub language: Option<String>,
    pub forced: bool,
    pub hearing_impaired: bool,
}

/// Release-group and download-site advertising inserted into otherwise normal
/// SRT files (for example YTS/YIFY "Downloaded from ..." cues). These are not
/// dialogue: they are dropped from extraction, playback and language detection.
pub fn is_release_promo(text: &str) -> bool {
    static PROMO: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)\b(?:yts|yify|yify-torrents|opensubtitles|subscene|addic7ed|podnapisi|rarbg|eztv|ettv|torrentgalaxy|1337x)\b|^\W*(?:re-?)?sync(?:ed|hronized)?(?: (?:and|&) correct(?:ed|ions))? by\b|^\W*(?:subtitles?|subs|captions?|encoded|ripped|resync) by\b").expect("valid regex")
    });
    text.len() <= 400 && PROMO.is_match(text.trim())
}

/// Language label from a sidecar filename such as `Movie (2020).zh-Hant.forced.srt`.
/// Used only as an encoding hint; discovery owns the authoritative labels.
fn filename_language(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    for label in stem.rsplit('.').take(4) {
        match label.to_ascii_lowercase().as_str() {
            "forced" | "sdh" | "cc" | "hi" => continue,
            "english" => return Some("en".into()),
            "chinese" => return Some("zh".into()),
            code if (2..=7).contains(&code.len())
                && code.split('-').next().is_some_and(|p| (2..=3).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphabetic()))
                && code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') => return Some(code.into()),
            _ => return None,
        }
    }
    None
}

/// Bounded read of a sidecar SRT in any common encoding, without release adverts.
pub fn read_srt_file(path: &Path, language: Option<&str>) -> Result<Vec<TimedCue>> {
    let cues = load_srt(path, language)?;
    ensure!(!cues.is_empty(), "Subtitle file contains only release advertising");
    Ok(cues)
}

fn load_srt(path: &Path, language: Option<&str>) -> Result<Vec<TimedCue>> {
    use std::io::Read;
    let file = std::fs::File::open(path)?;
    ensure!(file.metadata()?.is_file(), "Subtitle is not a regular file");
    let mut bytes = Vec::new();
    file.take(8_388_609).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 8_388_608, "Subtitle file too large");
    let hint = language.map(str::to_owned).or_else(|| filename_language(path));
    let text = crate::text_encoding::decode(&bytes, hint.as_deref())?;
    let mut cues = parse_srt(&text)?;
    cues.retain(|cue| !is_release_promo(&cue.text));
    Ok(cues)
}

enum SidecarContent { Language(Option<String>), Advertising }

// Detect unlabelled text only; never infer audio language from subtitle content.
fn detected_sidecar_language(path: &Path) -> SidecarContent {
    // Advert-only files are not subtitles; unreadable files stay visible so
    // their failure is reported rather than silently hidden.
    match load_srt(path, None) {
        Ok(cues) if cues.is_empty() => SidecarContent::Advertising,
        Ok(cues) => SidecarContent::Language(detect_text_language(&cues)),
        Err(_) => SidecarContent::Language(None),
    }
}

fn detect_text_language(cues: &[TimedCue]) -> Option<String> {
    use std::{collections::HashMap, io::Write, process::{Command, Stdio}, sync::{LazyLock, Mutex}, time::{Duration, Instant}};
    use sha2::Digest;
    static CACHE: LazyLock<Mutex<HashMap<Vec<u8>, Option<String>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
    let sample: String = cues.iter().take(400).map(|c| c.text.as_str()).collect::<Vec<_>>().join(" ").chars().take(6000).collect();
    let key = sha2::Sha256::digest(sample.as_bytes()).to_vec();
    if let Some(result) = CACHE.lock().ok()?.get(&key) { return result.clone(); }
    let helper = std::env::current_exe().ok()?.with_file_name("torrbaby-subtitle-extractor");
    let mut child = Command::new(helper).arg("--detect-language").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    if child.stdin.take()?.write_all(sample.as_bytes()).is_err() { let _ = child.kill(); let _ = child.wait(); return None; }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => { let _ = child.kill(); let _ = child.wait(); return None; }
        }
    }
    let output = child.wait_with_output().ok()?;
    let result: Option<String> = serde_json::from_slice(&output.stdout).ok()?;
    if let Ok(mut cache) = CACHE.lock() { if cache.len() >= 1024 { cache.clear(); } cache.insert(key, result.clone()); }
    result
}

/// Only exact video stems or stem.language/flags suffixes in the same directory.
/// Never guess that arbitrary subtitles in a season folder belong to one episode.
pub fn discover(media: &Path) -> Result<Vec<Sidecar>> {
    use sha2::Digest;
    let parent = media.parent().context("Missing media directory")?;
    let stem = media
        .file_stem()
        .and_then(|s| s.to_str())
        .context("Invalid media filename")?;
    let mut tracks = Vec::new();
    for (count, entry) in std::fs::read_dir(parent)?.enumerate() {
        ensure!(
            count < 20_000,
            "Media directory too large for subtitle discovery"
        );
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let path = entry.path();
        if !path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("srt"))
        {
            continue;
        }
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(candidate) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let suffix = if candidate == stem {
            ""
        } else if let Some(suffix) = candidate.strip_prefix(&format!("{stem}.")) {
            suffix
        } else {
            continue;
        };
        let mut language = None;
        let mut forced = false;
        let mut hearing_impaired = false;
        let mut recognized = true;
        for label in suffix.split('.').filter(|s| !s.is_empty()) {
            match label.to_ascii_lowercase().as_str() {
                "en" | "eng" | "english" if language.is_none() => language = Some("en".to_string()),
                "zh" | "zho" | "chi" | "chinese" if language.is_none() => {
                    language = Some("zh".to_string())
                }
                "zh-hans" | "chs" | "sc" if language.is_none() => {
                    language = Some("zh-Hans".to_string())
                }
                "zh-hant" | "cht" | "tc" if language.is_none() => {
                    language = Some("zh-Hant".to_string())
                }
                "forced" => forced = true,
                "sdh" | "cc" | "hi" => hearing_impaired = true,
                // Language codes are useful even before translation supports them.
                code if language.is_none()
                    && (code.len() == 2 || code.len() == 3)
                    && code.bytes().all(|b| b.is_ascii_lowercase()) =>
                {
                    language = Some(code.to_string())
                }
                _ => recognized = false,
            }
        }
        if !recognized || entry.metadata()?.len() > 8_388_608 {
            continue;
        }
        if language.is_none() {
            match detected_sidecar_language(&path) {
                SidecarContent::Language(detected) => language = detected,
                SidecarContent::Advertising => continue,
            }
        }
        let id: String = sha2::Sha256::digest(name.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        tracks.push(Sidecar {
            id,
            name: name.to_string(),
            language,
            forced,
            hearing_impaired,
        });
        ensure!(tracks.len() <= 128, "Too many subtitle tracks");
    }
    tracks.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(tracks)
}

pub fn sidecar_path(media: &Path, id: &str) -> Result<std::path::PathBuf> {
    let track = discover(media)?.into_iter().find(|t| t.id == id).context("Subtitle track not found")?;
    crate::api::checked_path(media.parent().context("Missing directory")?, &track.name)
}

pub fn read_sidecar(media: &Path, id: &str) -> Result<String> {
    ensure!(
        id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid subtitle ID"
    );
    let track = discover(media)?
        .into_iter()
        .find(|track| track.id == id)
        .context("Subtitle track not found")?;
    let path = crate::api::checked_path(media.parent().context("Missing directory")?, &track.name)?;
    Ok(webvtt(&read_srt_file(&path, track.language.as_deref())?))
}

#[cfg(test)]
mod sidecar_tests {
    #[test]
    fn chinese_lines_of_one_sentence_are_translated_together_and_spread_back() {
        let cue = |start: u64, end: u64, text: &str| TimedCue { start_ms: start, end_ms: end, text: text.into() };
        let cues = [cue(0, 1000, "还指望我对你们赵家"), cue(1050, 2000, "感恩戴德吗"), cue(4000, 4500, "告辞吧"),
            cue(5100, 6000, "好大的威风啊。"), cue(6050, 7000, "不过当初")];
        assert_eq!(sentence_groups(&cues, true), vec![0..2, 2..3, 3..4, 4..5]);
        assert_eq!(sentence_groups(&cues, false).len(), 5);
        let parts = split_translation("And you still expect me to be grateful to your Zhao family?", &[9, 5]).unwrap();
        assert_eq!(parts.len(), 2);
        assert!(parts.iter().all(|p| !p.is_empty()));
        assert_eq!(parts.join(" "), "And you still expect me to be grateful to your Zhao family?");
        assert_eq!(split_translation("Go.", &[3, 3]), None);
        // A batch is not cut inside a sentence.
        let long: Vec<TimedCue> = (0..34).map(|i| cue(i * 1000, i * 1000 + 950, "台词")).collect();
        assert_eq!(translation_batch_len(&long), 34);
    }

    use super::*;
    #[test]
    fn sdh_is_recognized_from_sound_descriptions_and_speaker_labels() {
        let plain: Vec<String> = (0..200).map(|i| format!("Line {i} of dialogue.")).collect();
        assert!(!describes_sounds(plain.iter().map(String::as_str)));
        let mut sdh = plain.clone();
        for i in (0..200).step_by(10) { sdh[i] = "[door slams]".into(); }
        assert!(describes_sounds(sdh.iter().map(String::as_str)));
        let mut labels = plain.clone();
        for i in (0..200).step_by(10) { labels[i] = "JERRY: Hello.".into(); }
        assert!(describes_sounds(labels.iter().map(String::as_str)));
        // A narrator now and then is not SDH.
        let mut narrator = plain.clone();
        narrator[5] = "NARRATOR: Long ago.".into();
        assert!(!describes_sounds(narrator.iter().map(String::as_str)));
        // All-caps subtitles: colons are dialogue, not speaker labels.
        let shouting: Vec<String> = (0..200).map(|i| if i % 5 == 0 { "WAIT: STOP!".into() } else { format!("I SAID LINE {i}.") }).collect();
        assert!(!describes_sounds(shouting.iter().map(String::as_str)));
    }
    fn scratch() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("torrbaby-sidecar-encoding-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn legacy_encoded_sidecars_are_read_and_release_adverts_dropped() {
        // Shape of the Pulp Fiction (1994) YIFY sidecar: CRLF, Windows-1252, adverts.
        let dir = scratch();
        let media = dir.join("Pulp Fiction (1994).mp4");
        std::fs::write(&media, b"").unwrap();
        let srt = dir.join("Pulp Fiction (1994).srt");
        std::fs::write(&srt, b"1\r\n00:00:01,000 --> 00:00:03,000\r\nDownloaded from yify-torrents.com\r\n\r\n2\r\n00:01:00,000 --> 00:01:02,000\r\nGar\xe7on, coffee!\r\n\r\n3\r\n00:01:03,000 --> 00:01:05,000\r\n- \xbfDonde esta el zapato?\r\n\r\n4\r\n00:01:06,000 --> 00:01:07,000\r\nsynced by sundox\r\n").unwrap();
        let cues = read_srt_file(&srt, None).unwrap();
        assert_eq!(cues.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), ["Garçon, coffee!", "- ¿Donde esta el zapato?"]);
        assert_eq!((cues[0].start_ms, cues[0].end_ms), (60_000, 62_000));
        // Playback serving uses the same decoder; previously "Subtitles must be UTF-8".
        let english = dir.join("Pulp Fiction (1994).en.srt");
        std::fs::rename(&srt, &english).unwrap();
        let id = discover(&media).unwrap()[0].id.clone();
        assert!(read_sidecar(&media, &id).unwrap().contains("Garçon, coffee!"));
        // A Chinese label steers GBK decoding.
        let chinese = dir.join("Pulp Fiction (1994).chs.srt");
        let gbk = encoding_rs::GB18030.encode("1\n00:00:01,000 --> 00:00:02,000\n我们现在就去吧，这个问题很重要。\n").0.into_owned();
        std::fs::write(&chinese, gbk).unwrap();
        assert_eq!(read_srt_file(&chinese, None).unwrap()[0].text, "我们现在就去吧，这个问题很重要。");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn advert_only_sidecar_is_not_a_subtitle_track() {
        // Exact content of the Parasite (2019) YTS sidecar: no dialogue at all.
        let dir = scratch();
        let media = dir.join("Parasite (2019).mp4");
        std::fs::write(&media, b"").unwrap();
        std::fs::write(dir.join("Parasite (2019).srt"), "1\n00:00:30,000 --> 00:00:36,000\nProvided by YTS.LT\n\n2\n00:00:36,500 --> 00:00:42,000\nFind the official YIFY movies site at\nhttps://YTS.LT\n\n3\n00:30:30,000 --> 00:30:36,000\nDownloaded from YTS.LT\n\n4\n01:00:30,000 --> 01:00:36,000\nDownload more movies for free\nfrom YTS.LT\n").unwrap();
        assert!(discover(&media).unwrap().is_empty());
        assert!(read_srt_file(&dir.join("Parasite (2019).srt"), None).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn dialogue_is_not_mistaken_for_release_adverts() {
        for line in ["Created and Encoded by --  Bokutox -- of  www.YIFY-TORRENTS.com.", "Provided by YTS.LT", "Subtitles by explosiveskull", "Sync & corrections by Rafael UPD"] {
            assert!(is_release_promo(line), "{line}");
        }
        for line in ["I synced the files by noon.", "Are you subscribed?", "The yeti is out there.", "Encoded? By whom?", "Visit www.thefacebook.com"] {
            assert!(!is_release_promo(line), "{line}");
        }
    }

    #[test]
    fn duplicated_damaged_tail_does_not_discard_valid_subtitles() {
        let cues = parse_srt("1\n00:00:01,000 --> 00:00:02,000\nHello\n\n2\n00:00:03,000 --> 00:00:04,000\n\n3\n00:00:05,000 --> 00:00:06,000\n<i></i>\n\n4\n00:00:07,000 --> 00:00:08,000\nBye\n").unwrap();
        assert_eq!(cues.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), ["Hello", "Bye"], "empty cues are skipped");
        let cues = parse_srt("1\n02:09:07,965 --> 02:09:11,423\nGrowls\n\n2\n02:09:47,442 --> 02:09:48,932\nBeeping\n\n9:07,965 --> 02:09:11,423\nGrowls\n\n3\n02:09:47,442 --> 02:09:48,932\nBeeping\n").unwrap();
        assert_eq!(cues.len(), 2);
    }

    #[test]
    fn recovers_truncated_start_only_when_close_to_full_end() {
        let cues = parse_srt("1\n02:09:01,000 --> 02:09:04,000\nBefore\n\n2\n9:07,965 --> 02:09:11,423\nAfter\n").unwrap();
        assert_eq!(cues[1].start_ms, 7_747_965);
        assert_eq!(cues[1].end_ms, 7_751_423);
        assert!(parse_srt("1\n8:07,965 --> 02:09:11,423\nInvalid\n").is_err());
        assert!(parse_srt("1\n9:17,965 --> 02:09:11,423\nInvalid\n").is_err());
    }

    #[test]
    fn blank_line_before_dialogue_does_not_reject_track() {
        let cues = parse_srt("1\n00:00:01,000 --> 00:00:02,000\n\n ... juicy ... \n\n2\n00:00:03,000 --> 00:00:04,000\nNext line\n").unwrap();
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].start_ms, 1000);
        assert_eq!(cues[1].text, "Next line");
    }
}
