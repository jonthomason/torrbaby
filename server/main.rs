//! Native server and CLI. Production cutover follows API and data parity checks.
mod api;
mod catalog;
mod config;
mod dolby;
mod hls;
mod lan;
mod matroska;
mod media_inventory;
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
mod storage;
mod subtitle_jobs;
mod subtitle_preferences;
mod subtitles;
mod telemetry;
mod torrent_search;
mod transmission;
mod video_metadata;
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
            println!("{}", virtual_mkv::build(&input, &output)?);
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
                let mut bytes = Vec::new();
                fs::File::open(input)?
                    .take(8_388_609)
                    .read_to_end(&mut bytes)?;
                anyhow::ensure!(bytes.len() <= 8_388_608, "Subtitle file too large");
                subtitles::parse_srt(std::str::from_utf8(&bytes)?)?
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
            let settings = config::Config::load(&config)?;
            let database = config::expand(&settings.state_file)?.with_extension("sqlite3");
            let listen = format!("{}:{}", settings.listen_host, settings.listen_port);
            api::serve(config, database, listen, false).await?;
        }
        Command::Preview {
            config,
            database,
            listen,
        } => api::serve(config, database, listen, true).await?,
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
