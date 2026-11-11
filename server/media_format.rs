//! Exact audio and video formats of library files, read with the bundled
//! ffprobe and stored in each file's media inventory: trade names such as
//! "Dolby Digital Plus" or "DTS-HD Master Audio", the speaker layout, and the
//! picture's resolution, codec, HDR format and average bitrate.
use crate::api::Service;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{io::Read, path::Path, process::{Command, Stdio}, time::{Duration, Instant}};

/// Stored with the result; raise it to describe every file again.
const VERSION: u64 = 1;

fn helper() -> Option<std::path::PathBuf> {
    let path = std::env::current_exe().ok()?.with_file_name("torrbaby-ffprobe");
    path.is_file().then_some(path)
}

fn audio_format(codec: &str, profile: &str) -> String {
    match codec {
        "eac3" if profile.contains("Atmos") => "Dolby Atmos".into(),
        "eac3" => "Dolby Digital Plus".into(),
        "ac3" => "Dolby Digital".into(),
        "truehd" if profile.contains("Atmos") => "Dolby TrueHD Atmos".into(),
        "truehd" => "Dolby TrueHD".into(),
        "dts" if profile.contains("DTS:X") => "DTS:X".into(),
        "dts" if profile.contains("MA") => "DTS-HD Master Audio".into(),
        "dts" if profile.contains("HRA") => "DTS-HD High Resolution".into(),
        "dts" if profile.contains("ES") => "DTS-ES".into(),
        "dts" if profile.contains("96/24") => "DTS 96/24".into(),
        "dts" if profile.contains("Express") => "DTS Express".into(),
        "dts" => "DTS".into(),
        "aac" if profile.contains("HE") => "HE-AAC".into(),
        "aac" => "AAC".into(),
        "flac" => "FLAC".into(),
        "alac" => "Apple Lossless".into(),
        "opus" => "Opus".into(),
        "vorbis" => "Vorbis".into(),
        "mp3" => "MP3".into(),
        "mp2" => "MP2".into(),
        "mlp" => "MLP".into(),
        other if other.starts_with("pcm_") => "PCM".into(),
        other => other.to_ascii_uppercase(),
    }
}

/// "5.1", "7.1", "stereo", "mono"; other counts as "4ch".
fn layout(channels: u64, named: &str) -> String {
    let prefix: String = named.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    match channels {
        1 => "mono".into(),
        2 => "stereo".into(),
        _ if prefix.contains('.') => prefix,
        6 => "5.1".into(),
        7 => "6.1".into(),
        8 => "7.1".into(),
        n => format!("{n}ch"),
    }
}

fn number(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}

/// A stream's own bitrate: the container's figure, else the statistics tags mkvmerge writes.
fn stream_bitrate(stream: &Value) -> Option<u64> {
    number(&stream["bit_rate"]).or_else(|| {
        stream["tags"].as_object()?.iter().find(|(key, _)| key.as_str() == "BPS" || key.starts_with("BPS-")).and_then(|(_, v)| number(v))
    }).filter(|rate| *rate > 0)
}

fn describe(report: &Value) -> Value {
    let streams = report["streams"].as_array().cloned().unwrap_or_default();
    let audio: Vec<Value> = streams.iter().filter(|s| s["codec_type"] == "audio").map(|s| {
        let channels = s["channels"].as_u64().unwrap_or(0);
        json!({"format":audio_format(s["codec_name"].as_str().unwrap_or(""), s["profile"].as_str().unwrap_or("")),
            "channels":channels,"layout":layout(channels, s["channel_layout"].as_str().unwrap_or("")),"bit_rate":stream_bitrate(s)})
    }).collect();
    let video = streams.iter().find(|s| s["codec_type"] == "video" && s["disposition"]["attached_pic"] != 1).map(|s| {
        let (width, height) = (s["width"].as_u64().unwrap_or(0), s["height"].as_u64().unwrap_or(0));
        // By width as well, so a film cropped to a wide frame keeps its class.
        let resolution = if width >= 3800 || height >= 2000 { "4K".to_owned() }
            else if width >= 1900 || height >= 1000 { "1080p".to_owned() }
            else if width >= 1260 || height >= 700 { "720p".to_owned() }
            else if height > 0 { format!("{height}p") } else { String::new() };
        let codec = match s["codec_name"].as_str().unwrap_or("") {
            "hevc" => "H.265".to_owned(), "h264" => "H.264".to_owned(), "av1" => "AV1".to_owned(), "vp9" => "VP9".to_owned(),
            "mpeg4" => "MPEG-4".to_owned(), "mpeg2video" => "MPEG-2".to_owned(), "vc1" => "VC-1".to_owned(), other => other.to_ascii_uppercase(),
        };
        let dolby_vision = s["side_data_list"].as_array().is_some_and(|list| list.iter().any(|d| d["side_data_type"].as_str().is_some_and(|t| t.contains("DOVI"))));
        let hdr = if dolby_vision { Some("Dolby Vision") } else { match s["color_transfer"].as_str() { Some("smpte2084") => Some("HDR10"), Some("arib-std-b67") => Some("HLG"), _ => None } };
        // Without its own figure, the picture's share is the file's bitrate less the audio tracks.
        let bit_rate = stream_bitrate(s).or_else(|| {
            let total = number(&report["format"]["bit_rate"])?;
            let sound: u64 = streams.iter().filter(|a| a["codec_type"] == "audio").map(|a| stream_bitrate(a).unwrap_or(0)).sum();
            total.checked_sub(sound).filter(|rate| *rate > 0)
        });
        json!({"resolution":resolution,"width":width,"height":height,"codec":codec,"hdr":hdr,"bit_rate":bit_rate})
    });
    json!({"audio":audio,"video":video})
}

fn probe(path: &Path) -> Result<Value> {
    let helper = helper().context("Format reader is not installed")?;
    let mut child = Command::new(helper).args(["-v", "error", "-show_streams", "-show_format", "-of", "json"]).arg(path)
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
    let mut stdout = child.stdout.take().context("Missing format report")?;
    let reader = std::thread::spawn(move || { let mut bytes = Vec::new(); let _ = (&mut stdout).take(8 * 1024 * 1024).read_to_end(&mut bytes); bytes });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? { break status; }
        if started.elapsed() > Duration::from_secs(60) { let _ = child.kill(); let _ = child.wait(); anyhow::bail!("Format reading timed out"); }
        std::thread::sleep(Duration::from_millis(20));
    };
    let bytes = reader.join().unwrap_or_default();
    ensure!(status.success(), "Format reading failed");
    Ok(describe(&serde_json::from_slice(&bytes)?))
}

/// Describe a few inventoried files that have no format yet. Runs repeatedly, so
/// new files and inventories rebuilt by other stages are picked up in turn.
pub async fn tick(s: &Service) -> Result<usize> {
    if helper().is_none() { return Ok(0); }
    let pending: Vec<(String, String, String)> = {
        let db = s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?;
        let mut statement = db.connection.prepare(
            "SELECT m.asset_id,m.revision,json_extract(a.document,'$.path') FROM media_inventory m JOIN library_assets a ON a.id=m.asset_id \
             WHERE json_extract(m.document,'$.state')='ready' AND COALESCE(json_extract(m.document,'$.format_version'),0)<?1 ORDER BY RANDOM() LIMIT 12")?;
        let rows = statement.query_map([VERSION as i64], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?.unwrap_or_default())))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut described = 0;
    for (id, revision, path) in pending {
        let file = std::path::PathBuf::from(&path);
        // A file on an unmounted disk, or one that changed, is left for a later pass.
        if crate::subtitle_jobs::revision(&file).ok().as_deref() != Some(revision.as_str()) { continue; }
        let report = tokio::task::spawn_blocking(move || probe(&file)).await?;
        let db = s.db.lock().map_err(|_| anyhow::anyhow!("Database unavailable"))?;
        // Merge into the row as it is now: other stages may have saved meanwhile.
        let current: Option<(String, String)> = { use rusqlite::OptionalExtension;
            db.connection.query_row("SELECT revision,document FROM media_inventory WHERE asset_id=?", [&id], |r| Ok((r.get(0)?, r.get(1)?))).optional()? };
        let Some((now_revision, text)) = current else { continue };
        if now_revision != revision { continue; }
        let mut info: Value = serde_json::from_str(&text)?;
        info["format_version"] = json!(VERSION);
        match report {
            Ok(report) => {
                info["video"] = report["video"].clone();
                // Audio tracks are listed in file order by both readers.
                if let (Some(tracks), Some(formats)) = (info["audio_tracks"].as_array_mut(), report["audio"].as_array()) {
                    if tracks.len() == formats.len() {
                        for (track, format) in tracks.iter_mut().zip(formats) {
                            for key in ["format", "channels", "layout", "bit_rate"] { track[key] = format[key].clone(); }
                        }
                    }
                }
            }
            Err(error) => { tracing::warn!(asset = id, %error, "Media format unavailable"); }
        }
        db.connection.execute("UPDATE media_inventory SET document=? WHERE asset_id=? AND revision=?", rusqlite::params![info.to_string(), id, revision])?;
        described += 1;
    }
    Ok(described)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formats_use_trade_names_layouts_and_bitrates() {
        let report = json!({"format":{"bit_rate":"16148596"},"streams":[
            {"codec_type":"video","codec_name":"hevc","width":1920,"height":800,"color_transfer":"bt709","disposition":{"attached_pic":0}},
            {"codec_type":"audio","codec_name":"dts","profile":"DTS-HD MA","channels":6,"channel_layout":"5.1(side)","tags":{"BPS":"4099592"}},
            {"codec_type":"audio","codec_name":"eac3","profile":"Dolby Digital Plus + Dolby Atmos","channels":6,"channel_layout":"5.1(side)","bit_rate":"768000"},
            {"codec_type":"audio","codec_name":"aac","profile":"LC","channels":2,"channel_layout":"stereo"},
            {"codec_type":"video","codec_name":"mjpeg","width":600,"height":900,"disposition":{"attached_pic":1}}]});
        let described = describe(&report);
        assert_eq!(described["video"], json!({"resolution":"1080p","width":1920,"height":800,"codec":"H.265","hdr":null,"bit_rate":16148596u64-4099592-768000}));
        let audio: Vec<(String, String)> = described["audio"].as_array().unwrap().iter()
            .map(|a| (a["format"].as_str().unwrap().to_owned(), a["layout"].as_str().unwrap().to_owned())).collect();
        assert_eq!(audio, [("DTS-HD Master Audio".to_owned(), "5.1".to_owned()), ("Dolby Atmos".to_owned(), "5.1".to_owned()), ("AAC".to_owned(), "stereo".to_owned())]);
        let vision = describe(&json!({"streams":[{"codec_type":"video","codec_name":"hevc","width":3840,"height":1600,"bit_rate":"24000000","color_transfer":"smpte2084",
            "side_data_list":[{"side_data_type":"DOVI configuration record"}]}]}));
        assert_eq!((vision["video"]["resolution"].as_str(), vision["video"]["hdr"].as_str(), vision["video"]["bit_rate"].as_u64()), (Some("4K"), Some("Dolby Vision"), Some(24000000)));
        assert_eq!(audio_format("truehd", "Dolby TrueHD + Dolby Atmos"), "Dolby TrueHD Atmos");
        assert_eq!(layout(8, "7.1"), "7.1");
        assert_eq!(layout(4, "quad"), "4ch");
    }
}
