//! Content-based container detection. Release packs ship Matroska named `.mp4`
//! and adverts named `.mkv`/`.mp4`, so the extension cannot choose a demuxer.
use std::{
    collections::HashMap,
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Container {
    /// EBML header: Matroska or WebM.
    Matroska,
    /// ISO base media / QuickTime boxes (MP4, M4V, MOV).
    Mp4,
    /// Another recognised video container (AVI, MPEG-TS/PS, ASF, FLV, Ogg, RealMedia).
    OtherVideo,
    /// Cannot be decided from the head (empty, zero-filled or truncated). Treated
    /// as media so damaged or still-preallocated videos stay visible as failures.
    Unknown,
    /// Readable, non-zero head that matches no video container: not a video file.
    NotMedia,
}

impl Container {
    pub fn name(self) -> &'static str {
        match self {
            Container::Matroska => "matroska",
            Container::Mp4 => "mp4",
            Container::OtherVideo => "other",
            Container::Unknown => "unknown",
            Container::NotMedia => "not_media",
        }
    }
}

/// Bytes needed to confirm an MPEG transport stream's second sync byte (M2TS: 4+188+1).
const HEAD: usize = 512;

pub fn classify(head: &[u8]) -> Container {
    let at = |offset: usize, magic: &[u8]| head.get(offset..offset + magic.len()) == Some(magic);
    if at(0, &[0x1A, 0x45, 0xDF, 0xA3]) {
        return Container::Matroska;
    }
    if head.len() >= 8 {
        let size = u32::from_be_bytes([head[0], head[1], head[2], head[3]]);
        let kind = &head[4..8];
        const BOXES: [&[u8; 4]; 12] = [
            b"ftyp", b"moov", b"mdat", b"free", b"skip", b"wide", b"pnot", b"uuid", b"styp",
            b"sidx", b"moof", b"PICT",
        ];
        if (size == 0 || size == 1 || size >= 8) && BOXES.iter().any(|b| kind == *b) {
            return Container::Mp4;
        }
    }
    let ts = |offset: usize| {
        head.get(offset) == Some(&0x47)
            && head.get(offset + 188).is_none_or(|b| *b == 0x47)
    };
    if (at(0, b"RIFF") && (at(8, b"AVI ") || at(8, b"AVIX")))
        || at(0, &[0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11])
        || at(0, &[0x00, 0x00, 0x01, 0xBA])
        || at(0, &[0x00, 0x00, 0x01, 0xB3])
        || at(0, b"FLV\x01")
        || at(0, b"OggS")
        || at(0, b".RMF")
        || (head.len() > 188 && ts(0))
        || (head.len() > 192 && ts(4))
    {
        return Container::OtherVideo;
    }
    if head.len() < 8 || head.iter().all(|b| *b == 0) {
        return Container::Unknown;
    }
    Container::NotMedia
}

type Key = (u64, u64, u64, i64, i64);
static CACHE: LazyLock<Mutex<HashMap<PathBuf, (Key, Container)>>> = LazyLock::new(Default::default);

/// Sniff the first bytes, cached by inode, size and modification time so a
/// replaced or rewritten file is sniffed again.
pub fn sniff(path: &Path) -> std::io::Result<Container> {
    let mut file = std::fs::File::open(path)?;
    let metadata = file.metadata()?;
    let key = (metadata.dev(), metadata.ino(), metadata.len(), metadata.mtime(), metadata.mtime_nsec());
    if let Some((cached, container)) = CACHE.lock().ok().and_then(|c| c.get(path).copied()) {
        if cached == key {
            return Ok(container);
        }
    }
    let mut head = Vec::with_capacity(HEAD);
    file.by_ref().take(HEAD as u64).read_to_end(&mut head)?;
    let container = classify(&head);
    if let Ok(mut cache) = CACHE.lock() {
        if cache.len() >= 65_536 {
            cache.clear();
        }
        cache.insert(path.to_owned(), (key, container));
    }
    Ok(container)
}

fn extension(path: &Path, names: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| names.iter().any(|n| e.eq_ignore_ascii_case(n)))
}

/// Matroska by content; an unreadable file falls back to its extension (the
/// caller's own open then reports the real error).
pub fn is_matroska(path: &Path) -> bool {
    match sniff(path) {
        Ok(container) => container == Container::Matroska,
        Err(_) => extension(path, &["mkv", "webm"]),
    }
}

/// Containers the virtual MP4 path and credits analysis read: Matroska (indexed)
/// and MP4/QuickTime (served as is). WebM keeps its own playback path.
pub fn virtual_mp4_source(path: &Path) -> bool {
    if extension(path, &["webm"]) {
        return false;
    }
    match sniff(path) {
        Ok(container) => matches!(container, Container::Matroska | Container::Mp4),
        Err(_) => extension(path, &["mkv", "mp4", "m4v", "mov"]),
    }
}

/// Library asset whose scanned content is not a video (see scanner::inventory).
pub fn tagged_not_media(asset: &serde_json::Value) -> bool {
    asset["container"] == Container::NotMedia.name()
}

pub fn not_media(path: &Path) -> bool {
    matches!(sniff(path), Ok(Container::NotMedia))
}

/// HTTP media type of an original file, by content where the extension can lie.
pub fn media_type(path: &Path) -> Option<&'static str> {
    if !extension(path, &["mp4", "m4v", "mov", "mkv", "webm"]) {
        return None;
    }
    Some(match sniff(path).ok()? {
        Container::Matroska if extension(path, &["webm"]) => "video/webm",
        Container::Matroska => "video/x-matroska",
        Container::Mp4 if extension(path, &["mov"]) => "video/quicktime",
        Container::Mp4 => "video/mp4",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const EBML: &[u8] = &[0x1A, 0x45, 0xDF, 0xA3, 0xA3, 0x42, 0x86, 0x81, 0x01, 0x42, 0xF7, 0x81];
    const FTYP: &[u8] = b"\x00\x00\x00\x20ftypisom\x00\x00\x02\x00isomiso2avc1mp41";
    // Head of the release-group advert files observed with .MKV/.MP4 names.
    const ADVERT: &[u8] = &[
        0x10, 0, 0, 0, 0x71, 0x34, 0x6c, 0x0f, 0xbb, 0x7c, 0xd5, 0x01, 0x67, 0xa2, 0x3a, 0x01,
        0xbb, 0x7c, 0xd5, 0x01, 0x90, 0x01, 0, 0, 0x04, 0, 0, 0, 0x20, 0x04, 0, 0,
    ];

    #[test]
    fn classifies_container_heads() {
        assert_eq!(classify(EBML), Container::Matroska);
        assert_eq!(classify(FTYP), Container::Mp4);
        assert_eq!(classify(b"\x00\x00\x00\x08free\x00\x00\x10\x00moov"), Container::Mp4);
        assert_eq!(classify(b"\x00\x00\x00\x01mdat\x00\x00\x00\x00\x00\x00\x00\x10"), Container::Mp4);
        assert_eq!(classify(b"RIFF\x10\x00\x00\x00AVI LIST"), Container::OtherVideo);
        let mut ts = vec![0u8; 400];
        ts[0] = 0x47;
        ts[188] = 0x47;
        assert_eq!(classify(&ts), Container::OtherVideo);
        let mut m2ts = vec![0u8; 400];
        m2ts[4] = 0x47;
        m2ts[192] = 0x47;
        assert_eq!(classify(&m2ts), Container::OtherVideo);
        assert_eq!(classify(ADVERT), Container::NotMedia);
        assert_eq!(classify(b"<!DOCTYPE html><html><body>ads</body></html>"), Container::NotMedia);
        let random: Vec<u8> = (0..512u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8 | 1).collect();
        assert_eq!(classify(&random), Container::NotMedia);
        // A lone 0x47 at offset 0 without the next sync byte is not a stream.
        let mut fake_ts = random.clone();
        fake_ts[0] = 0x47;
        fake_ts[188] = 0x11;
        assert_eq!(classify(&fake_ts), Container::NotMedia);
    }

    #[test]
    fn undecidable_heads_stay_media() {
        assert_eq!(classify(&[]), Container::Unknown);
        assert_eq!(classify(&[0x1A, 0x45]), Container::Unknown);
        assert_eq!(classify(&[0u8; 512]), Container::Unknown);
    }

    #[test]
    fn sniffs_files_by_content_not_extension() -> anyhow::Result<()> {
        let root = std::env::temp_dir().join(format!("torrbaby-container-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        let mkv_named_mp4 = root.join("Dr.No.mp4");
        let mut body = EBML.to_vec();
        body.resize(4096, 0x55);
        std::fs::write(&mkv_named_mp4, &body)?;
        let mp4_named_mkv = root.join("movie.mkv");
        std::fs::write(&mp4_named_mkv, FTYP)?;
        let advert = root.join("advert.MKV");
        std::fs::write(&advert, ADVERT)?;
        let webm = root.join("clip.webm");
        std::fs::write(&webm, EBML)?;
        assert!(is_matroska(&mkv_named_mp4));
        assert!(virtual_mp4_source(&mkv_named_mp4));
        assert_eq!(media_type(&mkv_named_mp4), Some("video/x-matroska"));
        assert!(!is_matroska(&mp4_named_mkv));
        assert!(virtual_mp4_source(&mp4_named_mkv));
        assert_eq!(media_type(&mp4_named_mkv), Some("video/mp4"));
        assert!(not_media(&advert) && !virtual_mp4_source(&advert) && !is_matroska(&advert));
        assert_eq!(media_type(&advert), None);
        assert!(is_matroska(&webm) && !virtual_mp4_source(&webm));
        assert_eq!(media_type(&webm), Some("video/webm"));
        // Replacing the content invalidates the cached sniff.
        std::fs::write(&advert, &body)?;
        assert!(is_matroska(&advert) && !not_media(&advert));
        // Missing files fall back to the extension; their open reports the error.
        assert!(is_matroska(&root.join("missing.mkv")) && !not_media(&root.join("missing.mkv")));
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
}
