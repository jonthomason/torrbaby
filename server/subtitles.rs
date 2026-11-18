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
#[derive(Serialize, Deserialize, Clone)]
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

/// Text that cannot belong to a subtitle line, cleaned where subtitles leave the server
/// (`tidy_vtt`), not where they are parsed: virtual MP4 bytes come from the stored text,
/// so a title's file changes only when its cleaned text, and so its job ID, does.
/// - SubRip files often carry SSA override blocks ({\an8} moves a line to the top, {\a6}
///   is the older form, {\pos(x,y)}); braces without a backslash ({gunshot}) are text.
/// - Vision reads a straight closing quote after a full stop or question mark (`pilot."`)
///   as Vietnamese letters (`pilotiẠỊi`), on discs' English subtitles; such runs are
///   dropped (with the stray i or l it reads beside them).
/// - Latin letters read as Cyrillic look-alikes (`"В"`) are put back.
/// - In Chinese read from the picture, kana that look like characters (ー 一, オ 才, カ 力)
///   become them; other kana are misreadings and dropped. ・ in names becomes ·.
pub fn tidy(text: &str, language: &str) -> String {
    use std::sync::LazyLock;
    static OVERRIDES: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\{\\[^}]*\}").expect("valid regex"));
    static VIETNAMESE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"[iIl]?[\x{1EA0}-\x{1EF9}][\x{1EA0}-\x{1EF9}iIl]*").expect("valid regex")
    });
    let text = OVERRIDES.replace_all(text, "");
    let text = VIETNAMESE.replace_all(&text, "");
    let chinese = language.starts_with("zh");
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        let mapped = match c {
            'А' => Some('A'), 'В' => Some('B'), 'Е' => Some('E'), 'К' => Some('K'), 'М' => Some('M'),
            'Н' => Some('H'), 'О' => Some('O'), 'Р' => Some('P'), 'С' => Some('C'), 'Т' => Some('T'),
            'Х' => Some('X'), 'а' => Some('a'), 'е' => Some('e'), 'о' => Some('o'), 'р' => Some('p'),
            'с' => Some('c'), 'у' => Some('y'), 'х' => Some('x'),
            '・' if chinese => Some('·'),
            'ー' | 'ｰ' if chinese => Some('一'),
            'オ' if chinese => Some('才'),
            'カ' | 'ガ' | 'か' if chinese => Some('力'),
            'ル' if chinese => Some('儿'),
            'エ' if chinese => Some('工'),
            'ロ' if chinese => Some('口'),
            'ニ' if chinese => Some('二'),
            'ハ' if chinese => Some('八'),
            'タ' if chinese => Some('夕'),
            'ヌ' if chinese => Some('又'),
            'ト' if chinese => Some('卜'),
            '\u{3040}'..='\u{30ff}' if chinese => None,
            other => Some(other),
        };
        if let Some(c) = mapped { out.push(c); }
    }
    out.lines().map(|l| l.trim_end()).collect::<Vec<_>>().join("\n")
}

/// `tidy` for a whole prepared WebVTT document. Unchanged text keeps its exact bytes.
pub fn tidy_vtt(text: &str, language: &str) -> String {
    let Ok(cues) = parse_srt(text.strip_prefix("WEBVTT\n\n").unwrap_or(text)) else { return text.to_owned() };
    let mut changed = false;
    let cues: Vec<TimedCue> = cues.into_iter().filter_map(|cue| {
        let cleaned = tidy(&cue.text, language);
        if cleaned != cue.text { changed = true; }
        (!cleaned.trim().is_empty()).then_some(TimedCue { text: cleaned, ..cue })
    }).collect();
    if !changed || cues.is_empty() { return text.to_owned(); }
    webvtt(&cues)
}

/// How `cues` line up with `reference`, two uploads of the same dialogue for different
/// releases: their times are multiplied by `scale` (another frame rate: PAL's 25 against film's
/// 23.976 runs 4% fast), then `shift` ms is added.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timing { pub scale: f64, pub shift: i64 }

/// The speed ratios tried: the same speed, then 25 / 24 / 23.976 fps against each other.
const SCALES: [f64; 7] = [1.0, 25.0 / (24000.0 / 1001.0), (24000.0 / 1001.0) / 25.0, 24.0 / (24000.0 / 1001.0), (24000.0 / 1001.0) / 24.0, 25.0 / 24.0, 24.0 / 25.0];

/// The timing at which the most cue starts land within 0.4 s of one in `reference`, among the
/// speed ratios and, for each, the offsets between their first 40 starts. None when none lines
/// up at least half the cues (another cut: with this many timings tried, a third can land by
/// chance); the same speed is preferred on a tie, and an offset within 0.3 s is none.
pub fn alignment(reference: &[TimedCue], cues: &[TimedCue]) -> Option<Timing> {
    if reference.len() < 10 || cues.len() < 10 { return None }
    let starts: Vec<i64> = reference.iter().map(|c| c.start_ms as i64).collect();
    let lands = |scale: f64, shift: i64| cues.iter().filter(|c| {
        let at = (c.start_ms as f64 * scale) as i64 + shift;
        let i = starts.partition_point(|s| *s < at);
        [i.checked_sub(1), Some(i)].into_iter().flatten().filter_map(|i| starts.get(i)).any(|s| (s - at).abs() <= 400)
    }).count();
    let mut best: Option<(usize, Timing)> = None;
    for scale in SCALES {
        let mut tried = std::collections::HashSet::new();
        for r in starts.iter().take(40) {
            for c in cues.iter().take(40) {
                let shift = r - (c.start_ms as f64 * scale) as i64;
                if shift.abs() > 300_000 || !tried.insert(shift / 100) { continue }
                let landed = lands(scale, shift);
                if best.is_none_or(|(most, _)| landed > most) { best = Some((landed, Timing { scale, shift })) }
            }
        }
    }
    let (landed, timing) = best?;
    (landed * 2 >= cues.len()).then_some(if timing.scale == 1.0 && timing.shift.abs() <= 300 { Timing { scale: 1.0, shift: 0 } } else { timing })
}

/// Where there is sound in a film, 50 ms at a time: louder than half again its median.
pub fn sound_frames(levels: &[f32]) -> Vec<bool> {
    let mut sorted: Vec<f32> = levels.to_vec();
    sorted.sort_by(f32::total_cmp);
    let median = sorted.get(sorted.len() / 2).copied().unwrap_or(0.0);
    levels.iter().map(|l| *l > median * 1.5).collect()
}

/// The timing that puts the most sound under `cues` and the least outside them, against the
/// film's own audio: `windows` of it (start in seconds, 50 ms frames of sound), among the speed
/// ratios and offsets up to 30 s. Windows near the start, middle and end catch both an offset and
/// a drift (another frame rate) without decoding the whole film. The reference no upload can get
/// wrong: Parasite's "hash-matched" English ran 10 s late and fell 5 s further behind by the end.
/// None when the cues already fit as well as any timing does (within 0.02), or none fits clearly.
pub fn audio_timing(windows: &[(f64, Vec<bool>)], cues: &[TimedCue]) -> Option<Timing> {
    const FRAME: f64 = 0.05;
    if cues.len() < 20 || windows.iter().map(|(_, w)| w.len()).sum::<usize>() < 1200 { return None }
    let prepared: Vec<(f64, Vec<u32>)> = windows.iter().map(|(start, sound)| {
        let mut prefix = vec![0u32; sound.len() + 1];
        for (i, s) in sound.iter().enumerate() { prefix[i + 1] = prefix[i] + *s as u32 }
        (*start, prefix)
    }).collect();
    let fit_in = |t: Timing, only: Option<usize>| {
        let (mut loud, mut frames, mut total, mut all) = (0f64, 0f64, 0f64, 0f64);
        for (index, (start, prefix)) in prepared.iter().enumerate() {
            if only.is_some_and(|o| o != index) { continue }
            let len = prefix.len() - 1;
            total += prefix[len] as f64;
            all += len as f64;
            let mut last = 0usize;
            for c in cues {
                let at = |ms: u64| ((ms as f64 * t.scale + t.shift as f64) / 1000.0 - start) / FRAME;
                let (a, b) = (at(c.start_ms), at(c.end_ms));
                if b <= 0.0 || a >= len as f64 { continue }
                let (a, b) = ((a.max(0.0) as usize).max(last), (b as usize).min(len));
                if b > a { loud += (prefix[b] - prefix[a]) as f64; frames += (b - a) as f64; last = b }
            }
        }
        if frames == 0.0 || frames >= all { return f64::MIN }
        loud / frames - (total - loud) / (all - frames)
    };
    let fit = |t: Timing| fit_in(t, None);
    let mut best = (f64::MIN, Timing { scale: 1.0, shift: 0 });
    for scale in SCALES {
        for tenth in -300..=300 {
            let timing = Timing { scale, shift: tenth * 100 };
            let score = fit(timing);
            if score > best.0 { best = (score, timing) }
        }
    }
    let now = fit(Timing { scale: 1.0, shift: 0 });
    // Only a clear answer: speech that stands out (sitcom laugh tracks, music and rain gave weak
    // fits that put The Office and Friends 10-20 s out), and every window, on its own, agreeing
    // within a second at that speed (a wrong speed agrees in one window at most).
    if best.0 < 0.15 || best.0 - now < 0.03 { return None }
    let (scale, shift) = (best.1.scale, best.1.shift);
    let agree = (0..prepared.len()).all(|w| {
        let own = (-300..=300).map(|tenth| (fit_in(Timing { scale, shift: tenth * 100 }, Some(w)), tenth * 100))
            .max_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, s)| s).unwrap_or(shift);
        (own - shift).abs() <= 1000
    });
    agree.then_some(best.1)
}

/// `cues` with `timing` undone (a retiming found wrong).
pub fn unshifted(cues: &[TimedCue], timing: Timing) -> Vec<TimedCue> {
    let back = |ms: u64| ((ms as f64 - timing.shift as f64) / timing.scale).round().max(0.0) as u64;
    cues.iter().map(|c| TimedCue { start_ms: back(c.start_ms), end_ms: back(c.end_ms), text: c.text.clone() }).collect()
}

/// `cues` retimed by `timing`; a cue that would start before 0 is dropped.
pub fn shifted(cues: &[TimedCue], timing: Timing) -> Vec<TimedCue> {
    let at = |ms: u64| (ms as f64 * timing.scale) as i64 + timing.shift;
    cues.iter().filter_map(|c| {
        let start = at(c.start_ms);
        (start >= 0).then(|| TimedCue { start_ms: start as u64, end_ms: at(c.end_ms).max(start) as u64, text: c.text.clone() })
    }).collect()
}

/// `cues` as SRT.
pub fn to_srt(cues: &[TimedCue]) -> String {
    let time = |ms: u64| format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000);
    cues.iter().enumerate().map(|(i, c)| format!("{}\n{} --> {}\n{}\n\n", i + 1, time(c.start_ms), time(c.end_ms), c.text.trim_end())).collect()
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
        // A cue that ends before it starts (a typo in a file people made: OpenSubtitles'
        // Chinese for BSG S03E09) cannot be shown; the rest of the file can.
        if end_ms <= start_ms {
            block.clear();
            continue;
        }
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
        cues.push(TimedCue {
            start_ms,
            end_ms,
            text,
        });
        block.clear();
    }
    ensure!(!cues.is_empty(), "No subtitle cues");
    // Files people make can list a cue out of place (OpenSubtitles' Chinese for Creed): put
    // in time order, not refused. Stable, so cues at the same time keep the file's order.
    cues.sort_by_key(|cue: &TimedCue| cue.start_ms);
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
    use std::{collections::HashMap, io::Write, process::{Command, Stdio}, sync::{LazyLock, Mutex}, time::Duration};
    use sha2::Digest;
    static CACHE: LazyLock<Mutex<HashMap<Vec<u8>, Option<String>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
    let sample: String = cues.iter().take(400).map(|c| c.text.as_str()).collect::<Vec<_>>().join(" ").chars().take(6000).collect();
    let key = sha2::Sha256::digest(sample.as_bytes()).to_vec();
    if let Some(result) = CACHE.lock().ok()?.get(&key) { return result.clone(); }
    let helper = std::env::current_exe().ok()?.with_file_name("torrbaby-subtitle-extractor");
    let mut child = Command::new(helper).arg("--detect-language").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    if child.stdin.take()?.write_all(sample.as_bytes()).is_err() { let _ = child.kill(); let _ = child.wait(); return None; }
    // The answer is one short line: it fits the pipe, so it is read once the helper exits.
    let mut stdout = child.stdout.take()?;
    crate::wake::wait_child(&mut child, Duration::from_secs(10)).ok()??;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut stdout, &mut bytes).ok()?;
    let result: Option<String> = serde_json::from_slice(&bytes).ok()?;
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
    let language = track.language.clone().unwrap_or_default();
    Ok(tidy_vtt(&webvtt(&read_srt_file(&path, track.language.as_deref())?), &language))
}

#[cfg(test)]
mod alignment_tests {
    use super::*;
    fn cues(starts: &[u64]) -> Vec<TimedCue> { starts.iter().map(|s| TimedCue { start_ms: *s, end_ms: s + 1500, text: format!("line {s}") }).collect() }
    #[test]
    fn another_release_is_lined_up_with_the_hash_matched_one() {
        // The Office S01E01: the same lines 23 s earlier in a WEB-DL's upload, some cues split
        // differently.
        // Dialogue: lines 1.2 to 6 s apart, irregularly.
        let mut seed = 7u64;
        let mut gap = || { seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); 1_200 + (seed >> 33) % 4_800 };
        let mut at = 55_315;
        let reference: Vec<u64> = (0..60).map(|_| { let start = at; at += gap(); start }).collect();
        let other: Vec<u64> = reference.iter().enumerate().filter(|(i, _)| i % 9 != 4).map(|(_, s)| s - 22_850).collect();
        assert_eq!(alignment(&cues(&reference), &cues(&other)), Some(Timing { scale: 1.0, shift: 22_850 }));
        assert_eq!(alignment(&cues(&reference), &cues(&reference)), Some(Timing { scale: 1.0, shift: 0 }));
        // A PAL release's upload: 25 fps against this file's 23.976, so 4% fast, and 5 s off.
        let film = 24000.0 / 1001.0;
        let pal: Vec<u64> = reference.iter().map(|s| (*s as f64 * film / 25.0) as u64 + 5_000).collect();
        let timing = alignment(&cues(&reference), &cues(&pal)).unwrap();
        assert!((timing.scale - 25.0 / film).abs() < 1e-9, "{timing:?}");
        let back = shifted(&cues(&pal), timing);
        assert!(back.iter().zip(&reference).all(|(c, r)| (c.start_ms as i64 - *r as i64).abs() <= 400));
        // Another cut, other dialogue: nothing lines up.
        let mut at = 40_000;
        let cut: Vec<u64> = (0..60).map(|_| { let start = at; at += gap(); start }).collect();
        assert_eq!(alignment(&cues(&reference), &cues(&cut)), None);
        let moved = shifted(&cues(&other), Timing { scale: 1.0, shift: 22_850 });
        assert_eq!(moved[0].start_ms, 55_315);
        assert!(to_srt(&moved).starts_with("1\n00:00:55,315 --> 00:00:56,815\nline 32465\n\n2\n"));
    }
}

#[cfg(test)]
mod sidecar_tests {
    #[test]
    fn cues_out_of_place_are_put_in_order() {
        use super::parse_srt;
        let text = "1\n00:00:05,000 --> 00:00:06,000\nsecond\n\n2\n00:00:01,000 --> 00:00:02,000\nfirst\n\n3\n00:00:09,000 --> 00:00:10,000\nthird\n";
        let cues = parse_srt(text).unwrap();
        assert_eq!(cues.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), ["first", "second", "third"]);
    }

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
    fn misreadings_and_override_blocks_are_tidied() {
        assert_eq!(tidy("{\\an8}We are Hellfire!", "en"), "We are Hellfire!");
        assert_eq!(tidy("{\\a6}{\\pos(262,221)}Top", "en"), "Top");
        assert_eq!(tidy("{gunshot}", "en"), "{gunshot}");
        assert_eq!(tidy("of a Viper pilotiẠỊi", "en"), "of a Viper pilot");
        assert_eq!(tidy("of my beingỊẠỊồ", "en"), "of my being");
        assert_eq!(tidy("looks like, DoctorạẠạ", "en"), "looks like, Doctor");
        assert_eq!(tidy("from \"A\" to \"В.\"", "en"), "from \"A\" to \"B.\"");
        assert_eq!(tidy("看起来，医生 ạạạ。", "zh-Hans"), "看起来，医生 。");
        assert_eq!(tidy("处处露オ扬己", "zh-Hans"), "处处露才扬己");
        assert_eq!(tidy("一寸ー寸搜", "zh-Hans"), "一寸一寸搜");
        assert_eq!(tidy("功カ教足", "zh-Hans"), "功力教足");
        assert_eq!(tidy("史蒂夫・乔布斯", "zh-Hans"), "史蒂夫·乔布斯");
        assert_eq!(tidy("多谢のとさな", "zh-Hans"), "多谢");
        assert_eq!(tidy("“Garçon”是男孩的意思。", "zh-Hans"), "“Garçon”是男孩的意思。");
        let vtt = webvtt(&parse_srt("1\n00:00:01,000 --> 00:00:02,000\nFine & <well>\n").unwrap());
        assert_eq!(tidy_vtt(&vtt, "en"), vtt, "clean text keeps its bytes");
        let dirty = webvtt(&parse_srt("1\n00:00:01,000 --> 00:00:02,000\nof a Viper pilotiẠỊi\n\n2\n00:00:03,000 --> 00:00:04,000\nỊẠ\n").unwrap());
        assert_eq!(tidy_vtt(&dirty, "en"), "WEBVTT\n\n1\n00:00:01.000 --> 00:00:02.000\nof a Viper pilot\n\n");
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
