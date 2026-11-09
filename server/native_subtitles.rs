//! Bounded native MP4 text reader, sharing the Matroska subtitle job interface.
use crate::subtitles::TimedCue;
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{path::Path, process::Stdio, sync::LazyLock, time::Duration};
use tokio::io::AsyncReadExt;
fn matroska(path: &Path) -> bool {
    path.extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("mkv") || s.eq_ignore_ascii_case("webm"))
}
pub async fn tracks(path: &Path) -> Result<Value> {
    if matroska(path) {
        return crate::matroska::tracks_worker(path).await;
    }
    let value: Value = serde_json::from_slice(&run(path, None).await?)?;
    let tracks = value["tracks"]
        .as_array()
        .context("Invalid subtitle inventory")?;
    ensure!(
        tracks.len() <= 256
            && tracks
                .iter()
                .all(|t| t["track"].as_u64().is_some_and(|n| n > 0)),
        "Invalid subtitle inventory"
    );
    Ok(value)
}
pub async fn extract(path: &Path, track: u64) -> Result<Vec<TimedCue>> {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("srt")) {
        ensure!(track == 1, "Invalid sidecar track");
        let input = path.to_owned();
        return tokio::task::spawn_blocking(move || {
            use std::io::Read;
            let file = std::fs::File::open(input)?;
            let mut raw = String::new();
            file.take(8_388_609).read_to_string(&mut raw)?;
            crate::subtitles::parse_srt(&raw)
        }).await?;
    }
    if matroska(path) {
        return crate::matroska::extract_worker(path, track).await;
    }
    let mut cues: Vec<TimedCue> = serde_json::from_slice(&run(path, Some(track)).await?)?;
    ensure!(
        !cues.is_empty()
            && cues.len() <= 100_000
            && cues
                .iter()
                .all(|c| c.end_ms > c.start_ms && c.text.len() <= 16_384),
        "Invalid subtitle cues"
    );
    for cue in &mut cues {
        cue.text = crate::subtitles::plain_text(&cue.text);
    }
    ensure!(
        cues.windows(2).all(|c| c[0].start_ms <= c[1].start_ms),
        "Unordered subtitle cues"
    );
    Ok(cues)
}
async fn run(path: &Path, track: Option<u64>) -> Result<Vec<u8>> {
    static GATE: LazyLock<tokio::sync::Semaphore> =
        LazyLock::new(|| tokio::sync::Semaphore::new(2));
    let _permit = GATE
        .try_acquire()
        .context("Subtitle reader busy; try again shortly")?;
    let helper = std::env::current_exe()?.with_file_name("torrbaby-subtitle-extractor");
    let mut command = tokio::process::Command::new("/bin/sh");
    command
        .args([
            "-c",
            "ulimit -c 0; ulimit -t 120 || exit 1; exec \"$@\"",
            "torrbaby-native-subtitles",
        ])
        .arg(helper)
        .arg(path);
    if let Some(track) = track {
        command.arg(track.to_string());
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .context("Missing subtitle reader stdout")?;
    let limit = if track.is_some() {
        16_777_216
    } else {
        1_048_576
    };
    let result = tokio::time::timeout(
        Duration::from_secs(if track.is_some() { 130 } else { 15 }),
        async {
            let mut bytes = Vec::new();
            stdout.take(limit + 1).read_to_end(&mut bytes).await?;
            ensure!(
                bytes.len() as u64 <= limit,
                "Subtitle reader output too large"
            );
            ensure!(
                child.wait().await?.success(),
                "Native text subtitle extraction failed"
            );
            Ok(bytes)
        },
    )
    .await;
    match result {
        Ok(Ok(bytes)) => Ok(bytes),
        failed => {
            let _ = child.kill().await;
            match failed {
                Ok(Err(e)) => Err(e),
                _ => anyhow::bail!("Native subtitle reader timed out"),
            }
        }
    }
}
