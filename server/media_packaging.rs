//! Supervised AVFoundation passthrough packaging; not yet exposed to clients.
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
    process::Stdio,
    sync::LazyLock,
    time::Duration,
};
use tokio::io::AsyncReadExt;

fn revision(path: &Path) -> Result<(u64, u64, u64, i64, i64, i64, i64)> {
    let m = path.metadata()?;
    ensure!(m.is_file(), "Media must be a regular file");
    Ok((
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    ))
}

/// Own only a newly created output directory. Never replace user files.
/// The helper independently enforces its byte budget and free-space reserve.
pub async fn package(input: &Path, output: &Path, helper: &Path, max_bytes: u64) -> Result<()> {
    static GATE: LazyLock<tokio::sync::Semaphore> =
        LazyLock::new(|| tokio::sync::Semaphore::new(1));
    let _permit = GATE.try_acquire().context("Media packaging is busy")?;
    ensure!(
        (1_048_576..=34_359_738_368).contains(&max_bytes),
        "Packaging budget must be between 1 MiB and 32 GiB"
    );
    let input = input.canonicalize()?;
    let before = revision(&input)?;
    fs::create_dir(output).context("Packaging requires a new output directory")?;
    fs::set_permissions(output, fs::Permissions::from_mode(0o700))?;
    let result = run(&input, output, helper, max_bytes).await;
    let result = result.and_then(|()| {
        ensure!(
            before == revision(&input)?,
            "Media changed during packaging"
        );
        let playlist = fs::read_to_string(output.join("media.m3u8"))?;
        ensure!(
            playlist.starts_with("#EXTM3U\n") && playlist.contains("#EXT-X-STREAM-INF:"),
            "Missing master playlist"
        );
        let mut total = playlist.len() as u64;
        let mut tracks = 0;
        for entry in fs::read_dir(output)? {
            let entry = entry?;
            if entry.file_name() == "media.m3u8" {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            ensure!(
                entry.file_type()?.is_dir()
                    && name
                        .strip_prefix("track-")
                        .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())),
                "Unexpected packaging directory"
            );
            tracks += 1;
            let media = fs::read_to_string(entry.path().join("media.m3u8"))?;
            ensure!(
                media.starts_with("#EXTM3U\n") && media.ends_with("#EXT-X-ENDLIST\n"),
                "Incomplete track playlist"
            );
            ensure!(
                entry.path().join("init.mp4").metadata()?.len() > 0,
                "Missing initialization segment"
            );
            for file in fs::read_dir(entry.path())? {
                let file = file?;
                ensure!(file.file_type()?.is_file(), "Unexpected packaging artifact");
                total = total
                    .checked_add(file.metadata()?.len())
                    .context("Packaging size overflow")?;
            }
        }
        ensure!((1..=8).contains(&tracks), "Invalid track count");
        ensure!(total <= max_bytes + 1_048_576, "Packaging exceeded budget");
        Ok(())
    });
    if result.is_err() {
        let _ = fs::remove_dir_all(output);
    }
    result
}
async fn run(input: &Path, output: &Path, helper: &Path, max_bytes: u64) -> Result<()> {
    let mut child = tokio::process::Command::new("/bin/sh")
        .args([
            "-c",
            "ulimit -c 0; ulimit -t 600 || exit 1; exec \"$@\"",
            "torrbaby-media-worker",
        ])
        .arg(helper)
        .arg(input)
        .arg(output)
        .arg(max_bytes.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("Start media packaging helper")?;
    let stdout = child.stdout.take().context("Missing packaging output")?;
    let result = tokio::time::timeout(Duration::from_secs(610), async {
        let mut response = Vec::new();
        stdout.take(4097).read_to_end(&mut response).await?;
        ensure!(response.len() <= 4096, "Packaging response too large");
        ensure!(
            child.wait().await?.success(),
            "Media is unsupported or packaging exceeded its limits"
        );
        let response: serde_json::Value = serde_json::from_slice(&response)?;
        ensure!(
            response["version"] == 1 && response["playlist"] == "media.m3u8",
            "Invalid packaging response"
        );
        Ok(())
    })
    .await;
    match result {
        Ok(Ok(())) => Ok(()),
        other => {
            let _ = child.kill().await;
            match other {
                Ok(Err(e)) => Err(e),
                _ => anyhow::bail!("Media packaging timed out"),
            }
        }
    }
}
