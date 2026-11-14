//! Native server and CLI. Production cutover follows API and data parity checks.
mod trials;
mod punch;
mod direct;
mod rendezvous;
mod api;
mod audio_transcode;
mod catalog;
mod config;
mod container;
mod credit_series;
mod credits;
mod diagnostics;
mod dolby;
mod hls;
mod lan;
mod matroska;
mod media_format;
mod media_inventory;
mod processing;
mod media_packaging;
mod metadata;
mod native_subtitles;
mod ocr_subtitles;
mod pgs_subtitles;
mod offline;
mod organizer;
mod pairing;
mod profiles;
mod library_files;
mod remux;
mod virtual_mkv;
mod virtual_media;
mod virtual_captions;
mod scanner;
mod speech_subtitles;
mod storage;
mod subtitle_jobs;
mod subtitle_preferences;
mod subtitles;
mod text_encoding;
mod telemetry;
mod torrent_search;
mod cloud;
mod watcher;
mod transmission;
mod video_metadata;
mod drives;
mod drive_settings;
mod drive_moves;
mod scan_report;
use anyhow::Result;
use clap::{Parser, Subcommand};
use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

#[derive(Parser)]
#[command(version, about = "Torrbaby standalone media server")]
struct Arguments {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Internal isolated bitmap subtitle decoding worker.
    OcrSubtitles { #[arg(long)] input: PathBuf, #[arg(long)] track: u64, #[arg(long)] output: PathBuf },
    /// Show how library matching identifies media paths (file name, then its folder),
    /// without the database or library changes. Uses and fills the TMDB response cache
    /// in the metadata directory, which holds tmdb.token.
    IdentifyMedia {
        #[arg(long)]
        metadata_dir: PathBuf,
        paths: Vec<PathBuf>,
    },
    /// Search torrent indexers without adding anything to Transmission.
    SearchTorrents {
        query: String,
        #[arg(long, default_value="world", value_parser=["world", "china"])]
        market: String,
        #[arg(long)]
        year: Option<u16>,
        #[arg(long)]
        season: Option<u16>,
        #[arg(long)]
        episode: Option<u16>,
        #[arg(long = "source")]
        sources: Vec<String>,
    },
    /// Index MKV packets as a virtual MP4 without copying media payloads.
    IndexMatroska {
        #[arg(long)] input: PathBuf,
        #[arg(long)] output: PathBuf,
    },
    /// Serve an existing virtual MKV index on loopback for playback checks.
    ServeIndexedMatroska {
        #[arg(long)] input: PathBuf,
        #[arg(long)] index: PathBuf,
        #[arg(long, default_value_t = 18770)] port: u16,
    },
    /// Isolated native subtitle playback proof with a JSON array of caption artifacts.
    ServeCaptionedMedia {
        #[arg(long)] input: PathBuf,
        #[arg(long)] index: Option<PathBuf>,
        #[arg(long)] captions: PathBuf,
        #[arg(long, default_value_t = 18769)] port: u16,
    },
    /// Development-only Matroska to MP4 passthrough; not production playback.
    RemuxMatroska {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long, default_value_t = 536_870_912)]
        max_bytes: u64,
    },
    /// Package compatible media as HLS for diagnostics (not yet client playback).
    PackageMedia {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        helper: Option<PathBuf>,
        #[arg(long, default_value_t = 536_870_912)]
        max_bytes: u64,
    },
    /// Inspect Matroska codecs and a bounded sample of frame timing without decoding.
    InspectMatroska {
        #[arg(long)]
        input: PathBuf,
        #[arg(long, default_value_t = 0)]
        frames: usize,
    },
    /// Report each file's container as detected from its content, not its extension.
    #[command(hide = true)]
    SniffContainer { inputs: Vec<PathBuf> },
    /// List embedded Matroska subtitle tracks, or extract one as WebVTT.
    MatroskaSubtitles {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        track: Option<u64>,
        #[arg(long, requires = "track", hide = true)]
        json_cues: bool,
    },
    /// Convert UTF-8 SRT subtitles to WebVTT, optionally translating their text.
    PrepareSubtitles {
        #[arg(long)]
        input: PathBuf,
        /// Read this embedded Matroska track instead of an SRT sidecar.
        #[arg(long)]
        track: Option<u64>,
        #[arg(long, requires = "target")]
        source: Option<String>,
        #[arg(long, requires = "source")]
        target: Option<String>,
        #[arg(long)]
        helper: Option<PathBuf>,
    },
    /// Translate a bounded JSON cue batch from stdin using installed Apple models.
    TranslateSubtitles {
        #[arg(long)]
        helper: Option<PathBuf>,
    },
    /// Run the standalone server and background library/organization workers.
    Serve {
        #[arg(long)]
        config: PathBuf,
    },
    /// Run the migration preview against a copy of the database. No organizer runs.
    Preview {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        database: PathBuf,
        #[arg(long, default_value = "127.0.0.1:8766")]
        listen: String,
    },
    /// Print what a library scan would record and what the organizer would move, changing
    /// nothing. Use a copy of the database. Compares builds on the same library.
    ScanReport {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        database: PathBuf,
        /// Use this saved Transmission snapshot instead of asking Transmission.
        #[arg(long)]
        torrents: Option<PathBuf>,
        /// Save the Transmission snapshot used, for a later run.
        #[arg(long)]
        save_torrents: Option<PathBuf>,
    },
    /// Test directory enumeration and one file read in each media root.
    CheckAccess {
        #[arg(long)]
        media_root: PathBuf,
    },
    /// Verify configuration and live Transmission connectivity without changing torrents.
    CheckTransmission {
        #[arg(long)]
        config: PathBuf,
    },
    /// Call the same authenticated API used by native clients.
    Request {
        #[arg(value_parser = ["GET", "POST", "PATCH"])]
        method: String,
        path: String,
        #[arg(long, default_value = "{}")]
        data: String,
        #[arg(long, env = "TORRBABY_URL", default_value = "http://127.0.0.1:8765")]
        url: String,
        #[arg(
            long,
            env = "TORRBABY_TOKEN_FILE",
            default_value = "~/.config/torrbaby/api.token"
        )]
        token_file: String,
    },
}

fn check_folder(path: &Path) -> io::Result<(usize, bool)> {
    let mut count = 0;
    let mut read_file = false;
    let mut pending = vec![path.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                count += 1;
                if !read_file {
                    let mut byte = [0_u8; 1];
                    fs::File::open(entry.path())?.read(&mut byte)?;
                    read_file = true;
                }
            }
        }
    }
    Ok((count, read_file))
}

#[tokio::main]
async fn main() -> Result<()> {
    match Arguments::parse().command {
        Command::PackageMedia {
            input,
            output,
            helper,
            max_bytes,
        } => {
            let helper = helper
                .unwrap_or(std::env::current_exe()?.with_file_name("torrbaby-media-segmenter"));
            media_packaging::package(&input, &output, &helper, max_bytes).await?;
            println!("Media packaging complete");
        }
        Command::IdentifyMedia { metadata_dir, paths } => {
            let metadata = metadata::Metadata::new(&metadata_dir)?;
            for path in paths {
                let name = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                let folder = path.parent().and_then(Path::file_name).map(|f| f.to_string_lossy().into_owned());
                let mut found = metadata.identify(&name, folder.as_deref()).await
                    .unwrap_or_else(|error| serde_json::json!({"error":error.to_string()}));
                found["path"] = serde_json::json!(path);
                found["candidates"] = serde_json::json!(found["candidates"].as_array().map(Vec::len));
                println!("{}", serde_json::to_string(&found)?);
            }
        }
        Command::SearchTorrents {
            query,
            market,
            year,
            season,
            episode,
            sources,
        } => {
            let search = torrent_search::Search::new()?;
            let result = search
                .search(torrent_search::Query {
                    market: if market == "china" { torrent_search::Market::China } else { torrent_search::Market::World },
                    query,
                    year,
                    season,
                    episode,
                    sources,
                })
                .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::OcrSubtitles { input, track, output } => { ocr_subtitles::decode(&input, track, &output)?; }
        Command::IndexMatroska { input, output } => {
            // With the converter beside this binary, audio Apple devices cannot decode is converted as the server would.
            let sidecars = audio_transcode::ensure(&output.with_extension("data"), "index-matroska", &input, &subtitle_jobs::revision(&input)?);
            println!("{}", virtual_mkv::build(&input, &output, &sidecars)?);
        }
        Command::ServeIndexedMatroska { input, index, port } => {
            virtual_mkv::serve(&input, &index, port).await?;
        }
        Command::ServeCaptionedMedia { input, index, captions, port } => {
            let source=if let Some(index)=index { virtual_captions::Source::Indexed(std::sync::Arc::new(virtual_mkv::Reader::open(&input,&index)?)) } else { virtual_captions::Source::file(&input)? };
            let tracks: Vec<virtual_captions::Caption>=serde_json::from_slice(&std::fs::read(captions)?)?;
            virtual_captions::serve(virtual_captions::Reader::new(source,&tracks)?,port).await?;
        }
        Command::RemuxMatroska {
            input,
            output,
            max_bytes,
        } => {
            println!("{}", remux::run(&input, &output, max_bytes)?);
        }
        Command::InspectMatroska { input, frames } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&matroska::inspect(&input, frames)?)?
            );
        }
        Command::SniffContainer { inputs } => {
            for input in inputs {
                let container = container::sniff(&input).map(|c| c.name().to_owned());
                println!("{}\t{}", container.unwrap_or_else(|e| format!("error: {e}")), input.display());
            }
        }
        Command::MatroskaSubtitles {
            input,
            track,
            json_cues,
        } => {
            if let Some(track) = track {
                let cues = matroska::extract(&input, track)?;
                if json_cues {
                    println!("{}", serde_json::to_string(&cues)?);
                } else {
                    print!("{}", subtitles::webvtt(&cues));
                }
            } else {
                println!("{}", serde_json::to_string(&matroska::tracks(&input)?)?);
            }
        }
        Command::PrepareSubtitles {
            input,
            track,
            source,
            target,
            helper,
        } => {
            let mut cues = if let Some(track) = track {
                native_subtitles::extract(&input, track).await?
            } else {
                subtitles::read_srt_file(&input, None)?
            };
            if let (Some(source), Some(target)) = (source, target) {
                let helper = helper.unwrap_or(
                    std::env::current_exe()?.with_file_name("torrbaby-subtitle-translator"),
                );
                subtitles::translate_cues(&helper, &source, &target, &mut cues).await?;
            }
            print!("{}", subtitles::webvtt(&cues));
        }
        Command::TranslateSubtitles { helper } => {
            let helper = helper
                .unwrap_or(std::env::current_exe()?.with_file_name("torrbaby-subtitle-translator"));
            let mut input = Vec::new();
            io::stdin().take(1_048_577).read_to_end(&mut input)?;
            anyhow::ensure!(input.len() <= 1_048_576, "Subtitle batch too large");
            let request = serde_json::from_slice(&input)?;
            let response = subtitles::translate(&helper, request).await?;
            println!("{}", serde_json::to_string(&response)?);
        }
        Command::Serve { config } => {
            // Warnings, and what drives, moves and the organizer did, go to the server's log
            // (stderr, which launchd writes to rust-server.log).
            {
                use tracing_subscriber::{filter::{LevelFilter, Targets}, prelude::*};
                let targets = Targets::new().with_default(LevelFilter::WARN)
                    .with_target("torrbaby::drives", LevelFilter::INFO)
                    .with_target("torrbaby::drive_moves", LevelFilter::INFO)
                    .with_target("torrbaby::organizer", LevelFilter::INFO);
                let _ = tracing_subscriber::registry()
                    .with(tracing_subscriber::fmt::layer().with_ansi(false).with_writer(std::io::stderr))
                    .with(targets).try_init();
            }
            let settings = config::Config::load(&config)?;
            let database = config::expand(&settings.state_file)?.with_extension("sqlite3");
            // Loopback stays loopback (tests, previews); otherwise every network the Mac is
            // on. An address in the settings is never bound: DHCP or Tailscale may change it.
            let loopback = settings.listen_host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback()) || settings.listen_host == "localhost";
            let listen = if loopback { format!("{}:{}", settings.listen_host, settings.listen_port) } else { format!("[::]:{}", settings.listen_port) };
            api::serve(config, database, listen, false).await?;
        }
        Command::Preview {
            config,
            database,
            listen,
        } => api::serve(config, database, listen, true).await?,
        Command::ScanReport { config, database, torrents, save_torrents } => {
            let report = scan_report::run(&config, &database, torrents.as_deref(), save_torrents.as_deref()).await?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Command::CheckAccess { media_root } => {
            let mut failed = false;
            for folder in ["Downloads", "Movies", "TV"] {
                let path = media_root.join(folder);
                match check_folder(&path) {
                    Ok((count, read)) => println!(
                        "{}: enumeration OK; {count} files; {}",
                        path.display(),
                        if read {
                            "sample file read OK"
                        } else {
                            "no files available to test reads"
                        }
                    ),
                    Err(error) => {
                        eprintln!("{}: {error}", path.display());
                        failed = true;
                    }
                }
            }
            if failed {
                anyhow::bail!("Media access check failed");
            }
        }
        Command::CheckTransmission { config: path } => {
            let config = config::Config::load(&path)?;
            let rpc = transmission::Transmission::new(&config)?;
            let result = rpc.call("session_stats", serde_json::json!({})).await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Request {
            method,
            path,
            data,
            url,
            token_file,
        } => {
            let token = fs::read_to_string(config::expand(&token_file)?)?;
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(90))
                .redirect(reqwest::redirect::Policy::none())
                .build()?;
            let mut request = client
                .request(
                    method.parse()?,
                    format!(
                        "{}/{}",
                        url.trim_end_matches('/'),
                        path.trim_start_matches('/')
                    ),
                )
                .bearer_auth(token.trim());
            if method != "GET" {
                request = request.json(&serde_json::from_str::<serde_json::Value>(&data)?);
            }
            let response = request
                .send()
                .await
                .map_err(|_| anyhow::anyhow!("Cannot reach Torrbaby server"))?;
            let status = response.status();
            let body: serde_json::Value = response.json().await?;
            println!("{}", serde_json::to_string_pretty(&body)?);
            if !status.is_success() {
                anyhow::bail!("Server returned HTTP {}", status.as_u16());
            }
        }
    }
    Ok(())
}
