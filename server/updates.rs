//! Builds of the apps, served by Diamond so a device finds and installs the newest one from the
//! server it already reaches, wherever it is (docs/updates.md).
//!
//! `scripts/package_ota.py --diamond` puts each iPhone/iPad build in
//! `<data>/updates/ios/<build>-<random>/` (manifest.plist and Torrbaby.ipa), and
//! `scripts/publish_android_update.py` each TV build in `<data>/updates/android/<build>-<random>/`
//! (Torrbaby.apk); each then writes the platform's `current.json`, which the apps read through the
//! API like anything else.
//! iOS's installer fetches the manifest and the app itself, without the app's credentials, so
//! those two files are served openly, but only under the random name of a published build.
use anyhow::{bail, Context, Result};
use axum::{body::Body, http::Response};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use regex::Regex;

/// Each platform and the files of a build that are served openly.
const PLATFORMS: [(&str, &[&str]); 2] = [("ios", &["manifest.plist", "Torrbaby.ipa"]), ("android", &["Torrbaby.apk"])];

fn root(data: &Path, platform: &str) -> Result<PathBuf> {
    if !PLATFORMS.iter().any(|(name, _)| *name == platform) { bail!("No builds for {platform}"); }
    Ok(data.join("updates").join(platform))
}

/// When each build's app file was last asked for (seconds since 1970), by build folder. iOS installs
/// only once Torrbaby is off the screen; the app learns from this that Install was chosen (iOS's
/// installer has begun fetching it) and goes to the home screen by itself, as ABT does.
static FETCHED: LazyLock<std::sync::Mutex<std::collections::HashMap<String, f64>>> = LazyLock::new(Default::default);

/// The newest published build of a platform: `build`, `version` and the `manifest` URL to install,
/// with `fetched_at` once its app file has been asked for.
pub fn current(data: &Path, platform: &str) -> Result<Value> {
    let file = root(data, platform)?.join("current.json");
    let text = std::fs::read_to_string(&file).with_context(|| format!("No build of {platform} is published on this server"))?;
    let mut current: Value = serde_json::from_str(&text)?;
    let link = current["manifest"].as_str().or(current["apk"].as_str()).unwrap_or("").to_owned();
    let fetched = FETCHED.lock().ok().and_then(|f| f.iter().find(|(tag, _)| link.contains(&format!("/{tag}/"))).map(|(_, at)| *at));
    if let Some(at) = fetched { current["fetched_at"] = serde_json::json!(at); }
    Ok(current)
}

/// `/updates/<platform>/<build>-<random>/<file>`, or nothing.
pub fn requested(path: &str) -> Option<(&str, &str, &str)> {
    static PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^/updates/([a-z]+)/([0-9]{1,6}-[0-9a-f]{8})/([A-Za-z]+\.[a-z]+)$").expect("valid regex"));
    let captures = PATH.captures(path)?;
    let (platform, tag, name) = (captures.get(1)?.as_str(), captures.get(2)?.as_str(), captures.get(3)?.as_str());
    PLATFORMS.iter().any(|(served, files)| *served == platform && files.contains(&name)).then_some((platform, tag, name))
}

pub async fn file(data: &Path, platform: &str, tag: &str, name: &str) -> Result<Response<Body>> {
    use tokio::io::AsyncReadExt;
    let path = root(data, platform)?.join(tag).join(name);
    let file = tokio::fs::File::open(&path).await.context("No such build")?;
    if name.starts_with("Torrbaby.") && !name.ends_with(".plist") {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
        if let Ok(mut fetched) = FETCHED.lock() { fetched.insert(tag.to_owned(), now); }
    }
    let size = file.metadata().await?.len();
    let kind = if name.ends_with(".plist") { "text/xml" } else if name.ends_with(".apk") { "application/vnd.android.package-archive" } else { "application/octet-stream" };
    let chunks = futures_util::stream::try_unfold((file, size), |(mut file, remaining)| async move {
        if remaining == 0 { return Ok::<_, std::io::Error>(None); }
        let mut buffer = vec![0; remaining.min(256 * 1024) as usize];
        let count = file.read(&mut buffer).await?;
        if count == 0 { return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "Build ended early")); }
        buffer.truncate(count);
        Ok(Some((buffer, (file, remaining - count as u64))))
    });
    Ok(Response::builder()
        .header("Content-Type", kind)
        .header("Content-Length", size)
        .header("Cache-Control", "no-store")
        .body(Body::from_stream(chunks))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_published_builds_two_files_are_served_openly() {
        assert_eq!(requested("/updates/ios/381-25bca8bb/Torrbaby.ipa"), Some(("ios", "381-25bca8bb", "Torrbaby.ipa")));
        assert_eq!(requested("/updates/ios/381-25bca8bb/manifest.plist"), Some(("ios", "381-25bca8bb", "manifest.plist")));
        assert_eq!(requested("/updates/android/125-25bca8bb/Torrbaby.apk"), Some(("android", "125-25bca8bb", "Torrbaby.apk")));
        for path in ["/updates/ios/current.json", "/updates/ios/381/Torrbaby.ipa", "/updates/ios/381-25bca8bb/../../state.sqlite3",
            "/updates/mac/381-25bca8bb/Torrbaby.ipa", "/updates/android/125-25bca8bb/Torrbaby.ipa", "/updates/ios/381-25BCA8BB/Torrbaby.ipa", "/updates/ios/381-25bca8bb/other.ipa"] {
            assert_eq!(requested(path), None, "{path}");
        }
    }

    #[test]
    fn the_newest_build_is_what_the_publisher_wrote() -> Result<()> {
        let data = std::env::temp_dir().join(format!("torrbaby-updates-{}", std::process::id()));
        assert!(current(&data, "ios").is_err());
        std::fs::create_dir_all(data.join("updates/ios"))?;
        std::fs::write(data.join("updates/ios/current.json"), r#"{"build":"382","version":"1.0","manifest":"https://x/updates/ios/382-0123abcd/manifest.plist"}"#)?;
        assert_eq!(current(&data, "ios")?["build"], "382");
        assert!(current(&data, "mac").is_err());
        std::fs::remove_dir_all(&data)?;
        Ok(())
    }
}
