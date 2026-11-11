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
    /// Set with `pending` when the packet comes from a converted audio copy.
    pub pending_side: Option<usize>,
    /// [virtual offset, which copy, offset in the copy, length]
    pub side_extents: Vec<[u64; 4]>,
}
/// A converted audio copy the index reads packets from.
#[derive(Serialize, Deserialize, Clone)]
pub struct SideFile {
    pub path: std::path::PathBuf,
    pub bytes: u64,
}
#[derive(Serialize, Deserialize)]
pub struct Index {
    pub version: u32,
    pub source_revision: String,
    pub size: u64,
    pub extents: Vec<[u64; 3]>,
    pub metadata_extents: Vec<[u64; 3]>,
    /// Source audio codecs left out of the virtual MP4. Older indexes omit
    /// nothing, so the default keeps them valid.
    #[serde(default)]
    pub omitted_audio: Vec<String>,
    #[serde(default)]
    pub audio_tracks: Option<u64>,
    /// Converted audio copies (formats Apple devices cannot decode) and the
    /// packets served from them. Absent in indexes built without any.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub side_files: Vec<SideFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub side_extents: Vec<[u64; 4]>,
}
/// Small marker beside the index, so a missing audio copy is noticed without parsing the index.
pub fn side_files(directory: &Path) -> Vec<(std::path::PathBuf, u64)> {
    fs::read(directory.join("sidecars.json")).ok()
        .and_then(|bytes| serde_json::from_slice::<Vec<SideFile>>(&bytes).ok())
        .map(|files| files.into_iter().map(|f| (f.path, f.bytes)).collect())
        .unwrap_or_default()
}
pub fn build(input: &Path, output: &Path, sidecars: &[crate::audio_transcode::Sidecar]) -> Result<serde_json::Value> {
    let revision = crate::subtitle_jobs::revision(input)?;
    fs::create_dir(output).context("Index destination must be new")?;
    let result = (|| -> Result<serde_json::Value> {
        let mapping = Arc::new(Mutex::new(Recording::default()));
        let metadata = output.join("metadata.mp4");
        let before = std::time::Instant::now();
        let remuxed = crate::remux::run_internal(input, &metadata, 34_359_738_368, Some(mapping.clone()), sidecars)?;
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
        let side_files = sidecars.iter()
            .map(|s| Ok(SideFile { path: s.path.clone(), bytes: s.path.metadata()?.len() }))
            .collect::<Result<Vec<_>>>()?;
        let mut previous = 0;
        for [at, which, source, length] in &record.side_extents {
            ensure!(
                *length > 0
                    && *at >= previous
                    && at.checked_add(*length).is_some_and(|v| v <= output_size)
                    && side_files.get(*which as usize).is_some_and(|f| source.checked_add(*length).is_some_and(|v| v <= f.bytes)),
                "Invalid converted audio extent"
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
        let mut spans: Vec<[u64; 2]> = record.extents.iter().map(|e| [e[0], e[2]])
            .chain(record.side_extents.iter().map(|e| [e[0], e[3]])).collect();
        spans.sort_unstable();
        spans.push([output_size, 0]);
        for [start, len] in spans {
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
            omitted_audio: serde_json::from_value(remuxed["omitted_audio"].clone())?,
            audio_tracks: remuxed["audio_tracks"].as_u64(),
            side_files: side_files.clone(),
            side_extents: std::mem::take(&mut record.side_extents),
        };
        if !side_files.is_empty() { fs::write(output.join("sidecars.json"), serde_json::to_vec(&side_files)?)?; }
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
    sides: Vec<fs::File>,
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
        let mut sides = Vec::new();
        for side in &index.side_files {
            let file = fs::File::open(&side.path).context("Converted audio is missing")?;
            ensure!(file.metadata()?.len() == side.bytes, "Converted audio changed; rebuild index");
            sides.push(file);
        }
        let mut side_end = 0;
        for &[at, which, offset, len] in &index.side_extents {
            ensure!(
                len > 0 && at >= side_end
                    && index.side_files.get(which as usize).is_some_and(|f| offset.checked_add(len).is_some_and(|v| v <= f.bytes)),
                "Invalid converted audio extent"
            );
            side_end = at.checked_add(len).context("Converted audio extent overflow")?;
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
        ensure!(
            metadata_revision(&original.metadata()?) == index.source_revision,
            "Source replaced during open"
        );
        Ok(Self {
            source: original,
            metadata,
            sides,
            index,
            position: 0,
        })
    }
    pub fn validate_source(&self) -> std::io::Result<()> {
        if metadata_revision(&self.source.metadata()?) != self.index.source_revision {
            return Err(std::io::Error::other("Media changed during playback"));
        }
        Ok(())
    }
    pub fn len(&self) -> u64 {
        self.index.size
    }
    /// The source audio codec that kept this index from carrying any audio.
    pub fn missing_audio(&self) -> Option<&str> {
        match self.index.audio_tracks {
            Some(0) => self.index.omitted_audio.first().map(String::as_str),
            _ => None,
        }
    }
    pub fn read_at(&self, mut at: u64, mut output: &mut [u8]) -> std::io::Result<usize> {
        use std::os::unix::fs::FileExt;
        self.validate_source()?;
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
                _ if self.index.side_extents.get(self.index.side_extents.partition_point(|e| e[0] + e[3] <= at)).is_some_and(|e| e[0] <= at) => {
                    let [start, which, offset, len] = self.index.side_extents[self.index.side_extents.partition_point(|e| e[0] + e[3] <= at)];
                    (
                        &self.sides[which as usize],
                        offset + at - start,
                        (start + len - at).min(output.len() as u64) as usize,
                    )
                }
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

pub async fn serve(source: &Path, directory: &Path, port: u16) -> Result<()> {
    let source =
        crate::virtual_captions::Source::Indexed(Arc::new(Reader::open(source, directory)?));
    crate::virtual_captions::serve(crate::virtual_captions::Reader::new(source, &[])?, port).await
}

fn metadata_revision(m: &fs::Metadata) -> String {
    use std::os::unix::fs::MetadataExt;
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
