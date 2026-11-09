//! Experimental MKV packet index with packed MP4 metadata and original packet offsets.
//! The temporary sparse construction file is never a playable standalone movie.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::Path,
    sync::{Arc, Mutex},
};
#[derive(Default)]
pub(crate) struct Recording {
    pub pending: Option<(u64, Vec<u8>)>,
    pub extents: Vec<[u64; 3]>,
}
#[derive(Serialize, Deserialize)]
pub struct Index {
    pub version: u32,
    pub source_revision: String,
    pub size: u64,
    pub extents: Vec<[u64; 3]>,
    pub metadata_extents: Vec<[u64; 3]>,
}
pub fn build(input: &Path, output: &Path) -> Result<serde_json::Value> {
    let revision = crate::subtitle_jobs::revision(input)?;
    fs::create_dir(output).context("Index destination must be new")?;
    let result = (|| -> Result<serde_json::Value> {
        let mapping = Arc::new(Mutex::new(Recording::default()));
        let metadata = output.join("metadata.mp4");
        let before = std::time::Instant::now();
        crate::remux::run_internal(input, &metadata, 34_359_738_368, Some(mapping.clone()))?;
        let mut record = mapping
            .lock()
            .map_err(|_| anyhow::anyhow!("Mapping unavailable"))?;
        ensure!(record.pending.is_none(), "Incomplete mapping");
        let mut previous = 0;
        let source_size = input.metadata()?.len();
        let output_size = metadata.metadata()?.len();
        for [at, source, length] in &record.extents {
            ensure!(
                *length > 0
                    && *at >= previous
                    && at.checked_add(*length).is_some_and(|v| v <= output_size)
                    && source
                        .checked_add(*length)
                        .is_some_and(|v| v <= source_size),
                "Invalid media extent"
            );
            previous = at + length;
        }
        ensure!(
            revision == crate::subtitle_jobs::revision(input)?,
            "Source changed during indexing"
        );
        // Persist only metadata bytes, never filesystem-dependent sparse holes.
        use std::io::{Read, Seek, SeekFrom, Write};
        let mut sparse = fs::File::open(&metadata)?;
        let packed_path = output.join("metadata.bin");
        let mut packed = fs::File::create(&packed_path)?;
        let mut metadata_extents = Vec::new();
        let mut cursor = 0;
        let mut packed_offset = 0;
        for &[start, _, len] in record
            .extents
            .iter()
            .chain(std::iter::once(&[output_size, 0, 0]))
        {
            if start > cursor {
                let n = start - cursor;
                ensure!(packed_offset + n <= 128 * 1024 * 1024, "Metadata too large");
                sparse.seek(SeekFrom::Start(cursor))?;
                let copied = std::io::copy(&mut (&mut sparse).take(n), &mut packed)?;
                ensure!(copied == n, "Truncated metadata");
                metadata_extents.push([cursor, packed_offset, n]);
                packed_offset += n;
            }
            cursor = start + len;
        }
        packed.flush()?;
        packed.sync_all()?;
        drop(sparse);
        fs::remove_file(&metadata)?;
        let index = Index {
            version: 1,
            source_revision: revision,
            size: output_size,
            extents: std::mem::take(&mut record.extents),
            metadata_extents,
        };
        let json = serde_json::to_vec(&index)?;
        ensure!(json.len() <= 128 * 1024 * 1024, "Index too large");
        fs::write(output.join("index.json"), &json)?;

        fs::File::open(output.join("index.json"))?.sync_all()?;
        use std::os::unix::fs::MetadataExt;
        Ok(
            serde_json::json!({"virtual_bytes": output_size, "index_bytes":json.len(),
            "metadata_allocated_bytes":packed_path.metadata()?.blocks()*512,"packets":index.extents.len(),"seconds":before.elapsed().as_secs_f64()}),
        )
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(output);
    }
    result
}

/// Reads a virtual MP4 without ever returning the sparse packet holes.
/// Keep this reader tied to the opened original, not a repeatedly resolved path.
pub struct Reader {
    source: fs::File,
    metadata: fs::File,
    index: Index,
    position: u64,
}
impl Reader {
    pub fn open(source: &Path, directory: &Path) -> Result<Self> {
        let path = directory.join("index.json");
        ensure!(
            path.metadata()?.len() <= 128 * 1024 * 1024,
            "Index too large"
        );
        let index: Index = serde_json::from_slice(&fs::read(path)?)?;
        ensure!(index.version == 1, "Unsupported index version");
        let original = fs::File::open(source)?;
        ensure!(
            crate::subtitle_jobs::revision(source)? == index.source_revision,
            "Source changed; rebuild index"
        );
        let metadata = fs::File::open(directory.join("metadata.bin"))?;
        let mut spans: Vec<_> = index.extents.iter().map(|e| (e[0], e[2])).collect();
        let mut metadata_end = 0;
        for &[at, offset, len] in &index.metadata_extents {
            ensure!(at >= metadata_end, "Unsorted metadata extents");
            metadata_end = at.checked_add(len).context("Metadata extent overflow")?;
            ensure!(
                len > 0
                    && offset
                        .checked_add(len)
                        .is_some_and(|v| v <= metadata.metadata().map(|m| m.len()).unwrap_or(0)),
                "Invalid metadata extent"
            );
            spans.push((at, len));
        }
        spans.sort_unstable();
        let mut covered = 0u64;
        for (at, len) in spans {
            ensure!(at == covered, "Missing or overlapping extent");
            covered = covered.checked_add(len).context("Extent overflow")?;
        }
        ensure!(covered == index.size, "Incomplete index");
        let mut end = 0;
        let source_size = original.metadata()?.len();
        ensure!(!index.extents.is_empty(), "Empty packet index");
        for &[at, from, len] in &index.extents {
            ensure!(
                len > 0
                    && at >= end
                    && at.checked_add(len).is_some_and(|v| v <= index.size)
                    && from.checked_add(len).is_some_and(|v| v <= source_size),
                "Invalid packet index"
            );
            end = at + len;
        }
        Ok(Self {
            source: original,
            metadata,
            index,
            position: 0,
        })
    }
    pub fn len(&self) -> u64 {
        self.index.size
    }
    pub fn read_at(&self, mut at: u64, mut output: &mut [u8]) -> std::io::Result<usize> {
        use std::os::unix::fs::FileExt;
        let requested = output
            .len()
            .min(self.index.size.saturating_sub(at) as usize);
        output = &mut output[..requested];
        while !output.is_empty() {
            let i = self.index.extents.partition_point(|e| e[0] + e[2] <= at);
            let (file, offset, count) = match self.index.extents.get(i) {
                Some(&[start, source, len]) if start <= at => (
                    &self.source,
                    source + at - start,
                    (start + len - at).min(output.len() as u64) as usize,
                ),
                _ => {
                    let j = self
                        .index
                        .metadata_extents
                        .partition_point(|e| e[0] + e[2] <= at);
                    let &[start, offset, len] = self
                        .index
                        .metadata_extents
                        .get(j)
                        .ok_or_else(|| std::io::Error::other("Missing metadata"))?;
                    if start > at {
                        return Err(std::io::Error::other("Missing metadata"));
                    }
                    (
                        &self.metadata,
                        offset + at - start,
                        (start + len - at).min(output.len() as u64) as usize,
                    )
                }
            };
            file.read_exact_at(&mut output[..count], offset)?;
            at += count as u64;
            output = &mut output[count..];
        }
        Ok(requested)
    }
}
impl std::io::Read for Reader {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        let n = self.read_at(self.position, output)?;
        self.position += n as u64;
        Ok(n)
    }
}
impl std::io::Seek for Reader {
    fn seek(&mut self, from: std::io::SeekFrom) -> std::io::Result<u64> {
        use std::io::{Error, ErrorKind, SeekFrom};
        let at = match from {
            SeekFrom::Start(n) => n as i128,
            SeekFrom::Current(n) => self.position as i128 + n as i128,
            SeekFrom::End(n) => self.index.size as i128 + n as i128,
        };
        if !(0..=u64::MAX as i128).contains(&at) {
            return Err(Error::new(ErrorKind::InvalidInput, "Invalid seek"));
        }
        self.position = at as u64;
        Ok(self.position)
    }
}

/// Isolated, loopback-only diagnostic server. Never replaces the running daemon.
pub async fn serve(source: &Path, directory: &Path, port: u16) -> Result<()> {
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
    let reader = Arc::new(Reader::open(source, directory)?);
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
