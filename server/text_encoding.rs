//! Decoding of user-supplied subtitle text (sidecar SRT files).
//!
//! Subtitle files from the wild are frequently not UTF-8: Windows-1252/Latin-1
//! for Western European text, UTF-16 with a byte-order mark from Windows tools,
//! and GBK/GB18030 or Big5 for Chinese. A decode never panics and never guesses
//! a single-byte Western encoding for text that looks like a double-byte CJK
//! encoding; when no candidate is convincing, it fails explicitly instead of
//! producing mojibake.
use anyhow::Result;
use encoding_rs::{
    BIG5, EUC_KR, Encoding, GB18030, SHIFT_JIS, UTF_16BE, UTF_16LE, WINDOWS_874, WINDOWS_1250,
    WINDOWS_1251, WINDOWS_1252, WINDOWS_1253, WINDOWS_1254, WINDOWS_1255, WINDOWS_1256, WINDOWS_1258,
};

/// Decode subtitle bytes. `language` is an optional tag (filename label or
/// container metadata) used only to choose among legacy encodings.
pub fn decode(bytes: &[u8], language: Option<&str>) -> Result<String> {
    if let Some((encoding, bom)) = Encoding::for_bom(bytes) {
        return strict(encoding, &bytes[bom..])
            .ok_or_else(|| anyhow::anyhow!("Subtitle text is not valid {}", encoding.name()));
    }
    if let Some(encoding) = bomless_utf16(bytes) {
        if let Some(text) = strict(encoding, bytes) {
            return Ok(text);
        }
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return Ok(text.to_owned());
    }
    if mostly_utf8(bytes) {
        return Ok(repair_mixed_utf8(bytes));
    }
    let language = language.map(str::to_ascii_lowercase);
    let language = language.as_deref().unwrap_or("");
    let primary = language.split(['-', '_']).next().unwrap_or("");
    let chinese = |preferred: &[&'static Encoding]| -> Result<String> {
        best_chinese(bytes, preferred)
            .ok_or_else(|| anyhow::anyhow!("Chinese subtitle text encoding could not be identified"))
    };
    match primary {
        "zh" | "zho" | "chi" | "chs" | "cht" | "sc" | "tc" => {
            let traditional = ["hant", "tw", "hk", "mo"].iter().any(|t| language.contains(t))
                || ["cht", "tc"].contains(&primary);
            let order = if traditional { [BIG5, GB18030] } else { [GB18030, BIG5] };
            return chinese(&order);
        }
        "ko" | "kor" => return strict(EUC_KR, bytes).ok_or_else(|| anyhow::anyhow!("Korean subtitle text encoding could not be identified")),
        "ja" | "jpn" => return strict(SHIFT_JIS, bytes).ok_or_else(|| anyhow::anyhow!("Japanese subtitle text encoding could not be identified")),
        _ => {}
    }
    if let Some(encoding) = single_byte(primary) {
        return Ok(encoding.decode_without_bom_handling(bytes).0.into_owned());
    }
    if looks_double_byte(bytes) {
        return chinese(&[GB18030, BIG5]);
    }
    // Untagged, undetermined or unrecognised tags with isolated high bytes.
    Ok(WINDOWS_1252.decode_without_bom_handling(bytes).0.into_owned())
}

/// The Windows code page conventionally used for legacy subtitles in a language.
/// encoding_rs treats ISO-8859-1 as Windows-1252, its common superset.
fn single_byte(primary: &str) -> Option<&'static Encoding> {
    Some(match primary {
        "en" | "eng" | "fr" | "fre" | "fra" | "de" | "ger" | "deu" | "es" | "spa" | "it" | "ita"
        | "pt" | "por" | "nl" | "dut" | "nld" | "sv" | "swe" | "da" | "dan" | "no" | "nor" | "nb"
        | "nob" | "fi" | "fin" | "is" | "ice" | "isl" | "ga" | "gle" | "ca" | "cat" | "eu"
        | "baq" | "eus" | "gl" | "glg" | "af" | "afr" | "id" | "ind" | "ms" | "may" | "msa"
        | "tl" | "tgl" | "sq" | "alb" | "sqi" => WINDOWS_1252,
        "pl" | "pol" | "cs" | "cze" | "ces" | "sk" | "slo" | "slk" | "hu" | "hun" | "ro" | "rum"
        | "ron" | "hr" | "hrv" | "sl" | "slv" | "bs" | "bos" => WINDOWS_1250,
        "ru" | "rus" | "uk" | "ukr" | "bg" | "bul" | "sr" | "srp" | "mk" | "mac" | "mkd" | "be"
        | "bel" => WINDOWS_1251,
        "el" | "gre" | "ell" => WINDOWS_1253,
        "tr" | "tur" => WINDOWS_1254,
        "he" | "heb" => WINDOWS_1255,
        "ar" | "ara" | "fa" | "per" | "fas" => WINDOWS_1256,
        "vi" | "vie" => WINDOWS_1258,
        "th" | "tha" => WINDOWS_874,
        _ => return None,
    })
}

fn strict(encoding: &'static Encoding, bytes: &[u8]) -> Option<String> {
    encoding
        .decode_without_bom_handling_and_without_replacement(bytes)
        .map(|text| text.into_owned())
}

/// UTF-16 without a BOM: NUL bytes consistently in one byte position.
fn bomless_utf16(bytes: &[u8]) -> Option<&'static Encoding> {
    if bytes.len() < 8 || bytes.len() % 2 != 0 {
        return None;
    }
    let pairs = bytes.len() / 2;
    let even = bytes.iter().step_by(2).filter(|b| **b == 0).count();
    let odd = bytes.iter().skip(1).step_by(2).filter(|b| **b == 0).count();
    if odd * 10 >= pairs * 4 && even * 20 <= pairs {
        Some(UTF_16LE)
    } else if even * 10 >= pairs * 4 && odd * 20 <= pairs {
        Some(UTF_16BE)
    } else {
        None
    }
}

/// Western single-byte text has a few accented letters among ASCII words
/// ("Garçon", "Kälteen"). In GBK/Big5 dialogue every character has a high lead
/// byte, so high bytes are a large share of all letters. (Big5 trail bytes can
/// be ASCII letters, so byte runs alone cannot tell the two apart.)
fn looks_double_byte(bytes: &[u8]) -> bool {
    let high = bytes.iter().filter(|b| **b >= 0x80).count();
    let letters = bytes.iter().filter(|b| b.is_ascii_alphabetic()).count();
    high >= 4 && high * 10 >= (high + letters) * 3
}

/// Mostly UTF-8 with a few stray legacy bytes, e.g. concatenated or edited files.
fn mostly_utf8(bytes: &[u8]) -> bool {
    let (mut multibyte, mut invalid) = (0usize, 0usize);
    for chunk in bytes.utf8_chunks() {
        multibyte += chunk.valid().chars().filter(|c| !c.is_ascii()).count();
        invalid += chunk.invalid().len();
    }
    multibyte > invalid
}

fn repair_mixed_utf8(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        text.push_str(chunk.valid());
        text.push_str(&WINDOWS_1252.decode_without_bom_handling(chunk.invalid()).0);
    }
    text
}

// Frequent Chinese characters in both simplified and traditional forms. Correctly
// decoded dialogue hits this set constantly; a wrong double-byte decode rarely does.
const COMMON_HANZI: &str = "的一是不了人我在有他这這个個们們中来來上大为為和国國地到以说說时時要就出会會可也你对對生能而子那得于於着著下自之年过過发發后後作里裡用道行所然家种種事成方多经經么麼去法学學如都同现現当當没沒动動面起看定天分还還进進好小部其些主样樣理心她本前开開但因只从從想实實日军軍者意无無力它与與长長把机機十民第公此已工使情明性知全三又关關点點正业業外将將两兩高间間由问問很最重并並物手应應战戰向头頭文体體政美相见見被利什二等产產或新己制身果加西斯月话話合回特代内內信表化老给給常此吗嗎呢吧啊哦嗯谁誰怎呀让讓走跟再快真谢謝请請先生太太叫做吃";

fn hanzi_score(text: &str) -> f64 {
    let (mut cjk, mut common) = (0usize, 0usize);
    for c in text.chars() {
        if ('\u{4e00}'..='\u{9fff}').contains(&c) || ('\u{3400}'..='\u{4dbf}').contains(&c) {
            cjk += 1;
            if COMMON_HANZI.contains(c) {
                common += 1;
            }
        }
    }
    if cjk < 2 { 0.0 } else { common as f64 / cjk as f64 }
}

fn best_chinese(bytes: &[u8], preferred: &[&'static Encoding]) -> Option<String> {
    let mut best: Option<(f64, String)> = None;
    for encoding in preferred {
        let Some(text) = strict(encoding, bytes) else { continue };
        let score = hanzi_score(&text);
        // Strictly better only: ties keep the language-preferred encoding.
        if best.as_ref().is_none_or(|(s, _)| score > *s + 0.02) {
            best = Some((score, text));
        }
    }
    best.filter(|(score, _)| *score >= 0.1).map(|(_, text)| text)
}

#[cfg(test)]
mod tests {
    use super::*;
    const SRT: &str = "1\r\n00:00:01,000 --> 00:00:02,000\r\n";

    fn with(text: &[u8]) -> Vec<u8> {
        [SRT.as_bytes(), text, b"\r\n"].concat()
    }

    #[test]
    fn utf8_and_utf8_bom_are_unchanged() {
        assert_eq!(decode("Garçon".as_bytes(), None).unwrap(), "Garçon");
        assert_eq!(decode(b"\xef\xbb\xbfHello", None).unwrap(), "Hello");
    }

    #[test]
    fn windows_1252_sidecars_decode_like_the_failing_library_files() {
        // Bytes from Casino Royale, Mean Girls and Pulp Fiction sidecars.
        let casino = with(b"Bollinger Grand Ann\xe9e...");
        assert!(decode(&casino, None).unwrap().contains("Bollinger Grand Année..."));
        let mean = with(b"What are K\xe4lteen bars?");
        assert!(decode(&mean, None).unwrap().contains("Kälteen"));
        let pulp = with(b"Gar\xe7on, coffee!\r\n- \xbfDonde esta el zapato? It\x92s \x93fine\x94\x85");
        let text = decode(&pulp, None).unwrap();
        assert!(text.contains("Garçon") && text.contains("¿Donde") && text.contains("It’s “fine”…"));
        assert_eq!(decode(b"Caf\xe9", Some("en")).unwrap(), "Café");
        assert_eq!(decode(b"Caf\xe9", Some("fre")).unwrap(), "Café");
    }

    #[test]
    fn utf16_with_and_without_bom() {
        let text = "1\n00:00:01,000 --> 00:00:02,000\n你好, café\n";
        let le: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let be: Vec<u8> = text.encode_utf16().flat_map(u16::to_be_bytes).collect();
        assert_eq!(decode(&[&[0xff, 0xfe][..], &le].concat(), None).unwrap(), text);
        assert_eq!(decode(&[&[0xfe, 0xff][..], &be].concat(), None).unwrap(), text);
        assert_eq!(decode(&le, None).unwrap(), text);
        assert_eq!(decode(&be, None).unwrap(), text);
        // An odd-length payload after a UTF-16 BOM is corrupt, not silently repaired.
        assert!(decode(&[0xff, 0xfe, b'a'], None).is_err());
    }

    #[test]
    fn chinese_legacy_encodings_are_identified_without_mojibake() {
        let simplified = "我们现在就去吧。你说的对，这个问题很重要。";
        let traditional = "我們現在就去吧。你說的對，這個問題很重要。";
        let gbk = GB18030.encode(simplified).0.into_owned();
        let big5 = BIG5.encode(traditional).0.into_owned();
        assert_eq!(decode(&with(&gbk), None).unwrap(), format!("{SRT}{simplified}\r\n"));
        assert_eq!(decode(&with(&big5), None).unwrap(), format!("{SRT}{traditional}\r\n"));
        // Tags only choose the order; content still wins over a wrong tag.
        assert_eq!(decode(&big5, Some("zh-Hans")).unwrap(), traditional);
        assert_eq!(decode(&gbk, Some("zh-Hant")).unwrap(), simplified);
        assert_eq!(decode(&gbk, Some("chi")).unwrap(), simplified);
        // Never decode double-byte text as Windows-1252 mojibake.
        let noise: Vec<u8> = (0..64).map(|i| 0x81 + (i * 37 % 0x7e) as u8).collect();
        assert!(decode(&noise, None).is_err());
        assert!(decode(&noise, Some("zh")).is_err());
    }

    #[test]
    fn hinted_non_latin_encodings_and_mixed_utf8() {
        let korean = "기생충 대사입니다";
        assert_eq!(decode(&EUC_KR.encode(korean).0, Some("ko")).unwrap(), korean);
        let japanese = "こんにちは、元気ですか";
        assert_eq!(decode(&SHIFT_JIS.encode(japanese).0, Some("jpn")).unwrap(), japanese);
        assert_eq!(decode(b"Za\xbf\xf3\xb3\xe6", Some("pol")).unwrap(), "Zażółć");
        let russian = "Привет, как дела?";
        assert_eq!(decode(&WINDOWS_1251.encode(russian).0, Some("ru")).unwrap(), russian);
        let mixed = [&"naïve café — ".as_bytes()[..], b"r\xe9sum\xe9"].concat();
        assert_eq!(decode(&mixed, None).unwrap(), "naïve café — résumé");
    }

    #[test]
    fn hostile_input_never_panics() {
        for bytes in [&b""[..], b"\xff", b"\xfe", b"\xef\xbb", b"\x00\x00\x00", b"\xc3", b"\x81\x81\x81\x81"] {
            for language in [None, Some("en"), Some("zh"), Some("ko"), Some("xx-YY")] {
                let _ = decode(bytes, language);
            }
        }
    }
}
