//! Native timed-text tracks appended virtually; audiovisual bytes remain in their source.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    os::unix::fs::{FileExt, MetadataExt},
    path::Path,
    sync::Arc,
};
#[derive(Clone, Serialize, Deserialize)]
pub struct Caption {
    pub id: String,
    pub language: String,
    pub text: String,
}
pub enum Source {
    Indexed(Arc<crate::virtual_mkv::Reader>),
    File(File, String),
}
fn revision(m: &std::fs::Metadata) -> String {
    format!(
        "{}:{}:{}:{}:{}:{}:{}",
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec()
    )
}
impl Source {
    pub fn file(path: &Path) -> Result<Self> {
        let f = File::open(path)?;
        let r = revision(&f.metadata()?);
        Ok(Self::File(f, r))
    }
    pub fn len(&self) -> Result<u64> {
        Ok(match self {
            Self::Indexed(r) => r.len(),
            Self::File(f, _) => f.metadata()?.len(),
        })
    }
    pub fn validate(&self) -> std::io::Result<()> {
        match self {
            Self::Indexed(r) => r.validate_source(),
            Self::File(f, r) => {
                if revision(&f.metadata()?) != *r {
                    return Err(std::io::Error::other("Media changed during playback"));
                }
                Ok(())
            }
        }
    }
    pub fn read_at(&self, at: u64, data: &mut [u8]) -> std::io::Result<()> {
        self.validate()?;
        match self {
            Self::Indexed(r) => {
                if r.read_at(at, data)? != data.len() {
                    return Err(std::io::Error::other("Truncated media"));
                }
                Ok(())
            }
            Self::File(f, _) => f.read_exact_at(data, at),
        }
    }
}
pub struct Reader {
    source: Source,
    base: u64,
    mask: Option<u64>,
    append: Vec<u8>,
}
fn n32(data: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(
        data.get(at..at + 4)
            .context("Truncated MP4 field")?
            .try_into()?,
    ))
}
fn n64(data: &[u8], at: usize) -> Result<u64> {
    Ok(u64::from_be_bytes(
        data.get(at..at + 8)
            .context("Truncated MP4 field")?
            .try_into()?,
    ))
}
fn atom(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(body.len() + 8);
    v.extend_from_slice(&((body.len() + 8) as u32).to_be_bytes());
    v.extend_from_slice(kind);
    v.extend_from_slice(body);
    v
}
fn children(data: &[u8]) -> Result<Vec<([u8; 4], &[u8])>> {
    let mut v = Vec::new();
    let mut p = 0;
    while p < data.len() {
        let n = n32(data, p)? as usize;
        ensure!(n >= 8 && n <= data.len() - p, "Invalid MP4 box");
        v.push((data[p + 4..p + 8].try_into()?, &data[p + 8..p + n]));
        p += n;
    }
    Ok(v)
}
fn find<'a>(data: &'a [u8], kind: &[u8; 4]) -> Result<&'a [u8]> {
    children(data)?
        .into_iter()
        .find(|(k, _)| k == kind)
        .map(|(_, b)| b)
        .context("Missing MP4 box")
}
fn moov(source: &Source) -> Result<(u64, Vec<u8>)> {
    let size = source.len()?;
    let mut at = 0;
    let mut result = None;
    let mut count = 0;
    while at < size {
        count += 1;
        ensure!(count < 10000 && size - at >= 8, "Invalid MP4 layout");
        let mut h = [0; 16];
        source.read_at(at, &mut h[..8])?;
        let mut n = n32(&h, 0)? as u64;
        let head = if n == 1 {
            ensure!(size - at >= 16, "Truncated box");
            source.read_at(at + 8, &mut h[8..])?;
            n = n64(&h, 8)?;
            16
        } else {
            8
        };
        ensure!(n >= head && n <= size - at, "Unsupported MP4 box size");
        ensure!(
            &h[4..8] != b"moof",
            "Fragmented MP4 requires another playback path"
        );
        if &h[4..8] == b"moov" {
            ensure!(
                result.is_none() && n - head <= 64 * 1024 * 1024,
                "Invalid movie metadata"
            );
            let mut bytes = vec![0; (n - head) as usize];
            source.read_at(at + head, &mut bytes)?;
            result = Some((at + 4, bytes));
        }
        at += n;
    }
    result.context("Missing movie metadata")
}
fn language(tag: &str) -> u16 {
    let tag = tag.to_ascii_lowercase();
    let code = if tag.starts_with("en") {
        b"eng"
    } else if tag.starts_with("zh") || tag == "chi" {
        b"zho"
    } else if tag.starts_with("ja") {
        b"jpn"
    } else {
        b"und"
    };
    ((code[0] - 0x60) as u16) << 10 | ((code[1] - 0x60) as u16) << 5 | (code[2] - 0x60) as u16
}
fn table(kind: &[u8; 4], entries: &[u8], count: u32) -> Vec<u8> {
    let mut b = vec![0; 4];
    b.extend(count.to_be_bytes());
    b.extend(entries);
    atom(kind, &b)
}
fn captions(text: &str) -> Result<Vec<crate::subtitles::TimedCue>> {
    ensure!(text.len() <= 16 * 1024 * 1024, "Subtitle file too large");
    // Artifacts are our own normalized WebVTT, using full HH:MM:SS.mmm clocks.
    let mut s = String::new();
    for block in text.split("\n\n") {
        if block.lines().any(|l| l.contains(" --> ")) {
            for line in block.lines() {
                if line.contains(" --> ") {
                    s.push_str(&line.replace('.', ","));
                } else {
                    s.push_str(line);
                }
                s.push('\n');
            }
            s.push('\n');
        }
    }
    crate::subtitles::parse_srt(&s)
}
fn track(
    c: &Caption,
    id: u32,
    scale: u32,
    duration: u64,
    dimensions: &[u8],
    offset: u64,
    payload: &mut Vec<u8>,
) -> Result<Vec<u8>> {
    let cues = captions(&c.text)?;
    ensure!(cues.len() <= 100000, "Too many subtitle cues");
    // Split overlaps into nonoverlapping timed samples so captions neither vanish nor linger.
    let mut events = std::collections::BTreeMap::<u64, Vec<(usize, bool)>>::new();
    events.entry(0).or_default();
    for (i, cue) in cues.iter().enumerate() {
        ensure!(cue.end_ms <= 172800000, "Subtitle duration too large");
        events.entry(cue.start_ms).or_default().push((i, true));
        events.entry(cue.end_ms).or_default().push((i, false));
    }
    let events: Vec<_> = events.into_iter().collect();
    let mut active = std::collections::BTreeSet::new();
    let mut sizes = Vec::new();
    let mut offsets = Vec::new();
    let mut times = Vec::new();
    for pair in events.windows(2) {
        let (start, changes) = &pair[0];
        for (i, on) in changes {
            if *on {
                active.insert(*i);
            } else {
                active.remove(i);
            }
        }
        let end = pair[1].0;
        let text = active
            .iter()
            .map(|i| cues[*i].text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        ensure!(text.len() <= u16::MAX as usize, "Caption too long");
        offsets.extend((offset + payload.len() as u64).to_be_bytes());
        sizes.extend(((text.len() + 2) as u32).to_be_bytes());
        times.extend(1u32.to_be_bytes());
        times.extend(u32::try_from(end - start)?.to_be_bytes());
        payload.extend((text.len() as u16).to_be_bytes());
        payload.extend(text.as_bytes());
        ensure!(
            payload.len() <= 64 * 1024 * 1024,
            "Caption payload too large"
        );
    }
    let count = (sizes.len() / 4) as u32;
    ensure!(count > 0, "Empty caption track");
    let mut tx = vec![0; 6];
    tx.extend(1u16.to_be_bytes());
    tx.extend(0u32.to_be_bytes());
    tx.extend([1, 255]);
    tx.extend([0; 4]);
    tx.extend([0; 8]);
    tx.extend([0; 4]);
    tx.extend(1u16.to_be_bytes());
    tx.extend([0, 24]);
    tx.extend([255; 4]);
    let mut fonts = vec![0, 1, 0, 1, 5];
    fonts.extend(b"Arial");
    tx.extend(atom(b"ftab", &fonts));
    let entry = atom(b"tx3g", &tx);
    let mut stbl = table(b"stsd", &entry, 1);
    stbl.extend(table(b"stts", &times, count));
    let mut chunks = Vec::new();
    for n in [1u32, 1, 1] {
        chunks.extend(n.to_be_bytes());
    }
    stbl.extend(table(b"stsc", &chunks, 1));
    let mut sz = vec![0; 8];
    sz.extend(count.to_be_bytes());
    sz.extend(sizes);
    stbl.extend(atom(b"stsz", &sz));
    stbl.extend(table(b"co64", &offsets, count));
    let url = atom(b"url ", &[0, 0, 0, 1]);
    let dref = table(b"dref", &url, 1);
    let mut minf = atom(b"nmhd", &[0; 4]);
    minf.extend(atom(b"dinf", &dref));
    minf.extend(atom(b"stbl", &stbl));
    let mut mdhd = vec![0; 12];
    mdhd.extend(1000u32.to_be_bytes());
    mdhd.extend(u32::try_from(events.last().context("Empty captions")?.0)?.to_be_bytes());
    mdhd.extend(language(&c.language).to_be_bytes());
    mdhd.extend([0; 2]);
    let mut handler = vec![0; 8];
    handler.extend(b"sbtl");
    handler.extend([0; 12]);
    handler.extend(if c.language.starts_with("zh") {
        b"Chinese (Simplified)\0".as_slice()
    } else if c.language.starts_with("en") {
        b"English\0".as_slice()
    } else {
        b"Subtitles\0".as_slice()
    });
    let mut mdia = atom(b"mdhd", &mdhd);
    let mut extended_language=vec![0;4];
    extended_language.extend(c.language.as_bytes());extended_language.push(0);
    mdia.extend(atom(b"elng", &extended_language));
    mdia.extend(atom(b"hdlr", &handler));
    mdia.extend(atom(b"minf", &minf));
    let mut tk = vec![0, 0, 0, 2];
    tk.extend([0; 8]);
    tk.extend(id.to_be_bytes());
    tk.extend([0; 4]);
    tk.extend(u32::try_from(duration)?.to_be_bytes());
    tk.extend([0; 8]);
    tk.extend([0; 2]);
    tk.extend(32760u16.to_be_bytes());
    tk.extend([0; 4]);
    for n in [65536u32, 0, 0, 0, 65536, 0, 0, 0, 1073741824] {
        tk.extend(n.to_be_bytes());
    }
    tk.extend(dimensions);
    ensure!(scale > 0, "Invalid movie timescale");
    let mut result = atom(b"tkhd", &tk);
    result.extend(atom(b"mdia", &mdia));
    Ok(atom(b"trak", &result))
}
impl Reader {
    pub fn new(source: Source, tracks: &[Caption]) -> Result<Self> {
        let base = source.len()?;
        if tracks.is_empty() {
            return Ok(Self {
                source,
                base,
                mask: None,
                append: Vec::new(),
            });
        }
        ensure!(tracks.len() <= 8, "Too many subtitle tracks");
        let (mask, movie) = moov(&source)?;
        let boxes = children(&movie)?;
        ensure!(
            !boxes.iter().any(|(k, _)| k == b"mvex"),
            "Fragmented movie unsupported"
        );
        let head = find(&movie, b"mvhd")?;
        let version = *head.first().context("Missing movie version")?;
        ensure!(version <= 1, "Invalid movie version");
        let scale = n32(head, if version == 1 { 20 } else { 12 })?;
        let duration = if version == 1 {
            n64(head, 24)?
        } else {
            n32(head, 16)? as u64
        };
        let mut next = 1u32;
        let mut dims = None;
        for (kind, data) in &boxes {
            if kind == b"trak" {
                let tk = find(data, b"tkhd")?;
                let id = n32(tk, if tk[0] == 1 { 20 } else { 12 })?;
                next = next.max(id.checked_add(1).context("Track ID overflow")?);
                let mdia = find(data, b"mdia")?;
                let h = find(mdia, b"hdlr")?;
                if h.get(8..12) == Some(b"vide") {
                    ensure!(tk.len() >= 8, "Truncated video track");
                    dims = Some(tk[tk.len() - 8..].to_vec());
                }
            }
        }
        let dims = dims.context("Missing video dimensions")?;
        let mut payload = Vec::new();
        let mut added = Vec::new();
        for c in tracks {
            added.extend(track(
                c,
                next,
                scale,
                duration,
                &dims,
                base + 8,
                &mut payload,
            )?);
            next = next.checked_add(1).context("Track ID overflow")?;
        }
        let mut merged = Vec::new();
        for (kind, data) in boxes {
            let mut data = data.to_vec();
            if &kind == b"mvhd" {
                ensure!(data.len() >= 4, "Invalid movie header");
                let n = data.len();
                data[n - 4..].copy_from_slice(&next.to_be_bytes());
            }
            merged.extend(atom(&kind, &data));
        }
        merged.extend(added);
        ensure!(merged.len() <= 96 * 1024 * 1024, "Movie metadata too large");
        let mut append = atom(b"mdat", &payload);
        append.extend(atom(b"moov", &merged));
        Ok(Self {
            source,
            base,
            mask: Some(mask),
            append,
        })
    }
    pub fn len(&self) -> u64 {
        self.base + self.append.len() as u64
    }
    pub fn validate_source(&self) -> std::io::Result<()> {
        self.source.validate()
    }
    pub fn read_at(&self, at: u64, out: &mut [u8]) -> std::io::Result<usize> {
        self.source.validate()?;
        let n = out.len().min(self.len().saturating_sub(at) as usize);
        let out = &mut out[..n];
        let original = (self.base.saturating_sub(at) as usize).min(n);
        if original > 0 {
            self.source.read_at(at, &mut out[..original])?;
            if let Some(mask) = self.mask {
                let a = at.max(mask);
                let b = (at + original as u64).min(mask + 4);
                if a < b {
                    out[(a - at) as usize..(b - at) as usize]
                        .copy_from_slice(&b"free"[(a - mask) as usize..(b - mask) as usize]);
                }
            }
        }
        if original < n {
            let start = (at + original as u64 - self.base) as usize;
            out[original..].copy_from_slice(&self.append[start..start + n - original]);
        }
        Ok(n)
    }
}

/// Isolated, loopback-only diagnostic server. Never replaces the running daemon.
pub async fn serve(reader: Reader, port: u16) -> Result<()> {
    use axum::{
        Router,
        body::Body,
        extract::State,
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
        routing::get,
    };
    async fn response(
        State(reader): State<Arc<Reader>>,
        headers: HeaderMap,
    ) -> axum::response::Response {
        let size = reader.len();
        let range = headers.get("range").and_then(|v| v.to_str().ok());
        let parsed = (|| -> Option<(u64, u64)> {
            let Some(value) = range else {
                return Some((0, size.checked_sub(1)?));
            };
            let (a, b) = value.strip_prefix("bytes=")?.split_once('-')?;
            if a.is_empty() {
                let n = b.parse::<u64>().ok()?;
                return (n > 0).then_some((size.saturating_sub(n), size - 1));
            }
            let start = a.parse::<u64>().ok()?;
            let end = if b.is_empty() {
                size - 1
            } else {
                b.parse::<u64>().ok()?.min(size - 1)
            };
            (start <= end && start < size).then_some((start, end))
        })();
        let Some((start, end)) = parsed else {
            return (
                StatusCode::RANGE_NOT_SATISFIABLE,
                [("content-range", format!("bytes */{size}"))],
            )
                .into_response();
        };
        let stream = futures_util::stream::try_unfold(
            (reader, start, end + 1),
            |(reader, at, end)| async move {
                if at == end {
                    return Ok::<_, std::io::Error>(None);
                }
                let count = (end - at).min(256 * 1024) as usize;
                let next_reader = reader.clone();
                let bytes = tokio::task::spawn_blocking(move || {
                    let mut bytes = vec![0; count];
                    let n = next_reader.read_at(at, &mut bytes)?;
                    if n != count {
                        return Err(std::io::Error::other("Truncated virtual media"));
                    }
                    Ok::<_, std::io::Error>(bytes)
                })
                .await
                .map_err(std::io::Error::other)??;
                Ok(Some((bytes, (reader, at + count as u64, end))))
            },
        );
        let mut result = axum::http::Response::builder()
            .status(if range.is_some() { 206 } else { 200 })
            .header("content-type", "video/mp4")
            .header("accept-ranges", "bytes")
            .header("content-length", (end - start + 1).to_string());
        if range.is_some() {
            result = result.header("content-range", format!("bytes {start}-{end}/{size}"));
        }
        result.body(Body::from_stream(stream)).unwrap()
    }
    let reader = Arc::new(reader);
    let app = Router::new()
        .route("/movie.mp4", get(response))
        .with_state(reader);
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    println!(
        "Virtual MP4 diagnostic server: http://127.0.0.1:{}/movie.mp4",
        listener.local_addr()?.port()
    );
    axum::serve(listener, app).await?;
    Ok(())
}
