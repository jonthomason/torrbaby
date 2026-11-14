//! Logs a device sends from Settings → Connection → Send logs to Diamond, kept on the
//! server so problems seen away from home (the tunnel, the relay, playback) can be read
//! afterwards. Requests are capped at 64 KB, so a device sends its logs in parts; each
//! upload becomes one file under `client-logs/`.

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{fs, io::Write, path::Path};

/// The largest one upload may grow to.
const MAX_UPLOAD: u64 = 8 * 1024 * 1024;
/// Older uploads are removed past this many files or bytes.
const KEEP_FILES: usize = 300;
const KEEP_BYTES: u64 = 256 * 1024 * 1024;

fn file_name(upload: &str, device: &str, started: f64) -> String {
    let device: String = device.chars().map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' }).take(40).collect();
    let stamp = chrono_stamp(started);
    format!("{stamp}-{device}-{}.log", &upload[..8])
}

/// "20261114-121530" in UTC, from seconds since 1970.
fn chrono_stamp(seconds: f64) -> String {
    let secs = seconds.max(0.0) as i64;
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    format!("{year:04}{month:02}{day:02}-{:02}{:02}{:02}", rest / 3600, rest % 3600 / 60, rest % 60)
}

/// Adds one part of an upload: `upload` (a UUID the device picks), `part` (0, 1, …), `device`
/// and `text`. Part 0 starts the file. Returns the file's name.
pub fn receive(root: &Path, body: &Value) -> Result<Value> {
    let upload = body["upload"].as_str().context("Missing upload")?;
    ensure!(upload.len() == 36 && upload.chars().all(|c| c.is_ascii_hexdigit() || c == '-'), "Invalid upload");
    let part = body["part"].as_u64().context("Missing part")?;
    let text = body["text"].as_str().context("Missing text")?;
    let device = body["device"].as_str().unwrap_or("device");
    let directory = root.join("client-logs");
    fs::create_dir_all(&directory)?;
    let existing = fs::read_dir(&directory)?.filter_map(|entry| entry.ok())
        .find(|entry| entry.file_name().to_string_lossy().ends_with(&format!("-{}.log", &upload[..8])));
    let path = match (part, existing) {
        (0, _) => {
            prune(&directory)?;
            directory.join(file_name(upload, device, crate::storage::now()))
        }
        (_, Some(entry)) => entry.path(),
        (_, None) => anyhow::bail!("Upload not started"),
    };
    let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    ensure!(size + text.len() as u64 <= MAX_UPLOAD, "Upload too large");
    let mut file = fs::OpenOptions::new().create(true).append(true).open(&path)?;
    file.write_all(text.as_bytes())?;
    if body["last"] == true { eprintln!("Client logs received: {}", path.display()); }
    Ok(json!({"saved": path.file_name().map(|n| n.to_string_lossy().into_owned())}))
}

/// Keeps the newest uploads within the file and byte limits.
fn prune(directory: &Path) -> Result<()> {
    let mut files: Vec<(std::time::SystemTime, u64, std::path::PathBuf)> = fs::read_dir(directory)?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| { let meta = entry.metadata().ok()?; Some((meta.modified().ok()?, meta.len(), entry.path())) })
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    let mut total = 0;
    for (index, (_, size, path)) in files.into_iter().enumerate() {
        total += size;
        if index >= KEEP_FILES - 1 || total > KEEP_BYTES { let _ = fs::remove_file(path); }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uploads_arrive_in_parts_as_one_file() -> Result<()> {
        let root = std::env::temp_dir().join(format!("torrbaby-diagnostics-{}", std::process::id()));
        let upload = "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0";
        let first = receive(&root, &json!({"upload":upload,"part":0,"device":"Jon's iPhone","text":"one\n"}))?;
        receive(&root, &json!({"upload":upload,"part":1,"device":"Jon's iPhone","text":"two\n","last":true}))?;
        let name = first["saved"].as_str().unwrap();
        assert!(name.ends_with("-Jon_s_iPhone-0f1e2d3c.log"), "{name}");
        assert_eq!(fs::read_to_string(root.join("client-logs").join(name))?, "one\ntwo\n");
        assert!(receive(&root, &json!({"upload":"../../etc/passwd-xxxxxxxxxxxxxxxxxxxxxx","part":0,"text":"x"})).is_err());
        assert!(receive(&root, &json!({"upload":"11111111-2222-3333-4444-555555555555","part":3,"text":"x"})).is_err());
        assert_eq!(chrono_stamp(1_794_629_730.0), "20261114-041530");
        fs::remove_dir_all(root)?;
        Ok(())
    }
}
