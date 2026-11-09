//! Native server and CLI. Production cutover follows API and data parity checks.
mod api;
mod catalog;
mod config;
mod metadata;
mod organizer;
mod pairing;
mod scanner;
mod storage;
mod subtitles;
mod telemetry;
mod transmission;
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
    /// Convert UTF-8 SRT subtitles to WebVTT, optionally translating their text.
    PrepareSubtitles {
        #[arg(long)]
        input: PathBuf,
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
        Command::PrepareSubtitles {
            input,
            source,
            target,
            helper,
        } => {
            let mut bytes = Vec::new();
            fs::File::open(input)?
                .take(8_388_609)
                .read_to_end(&mut bytes)?;
            anyhow::ensure!(bytes.len() <= 8_388_608, "Subtitle file too large");
            let mut cues = subtitles::parse_srt(std::str::from_utf8(&bytes)?)?;
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
