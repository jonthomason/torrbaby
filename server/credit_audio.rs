//! Experimental: closing credits found by sound. The episodes of a series end with the same
//! music (a Chinese drama's ending song, a Western show's closing theme), so the stretch of
//! audio an episode's last minutes share with its siblings' is where its credits begin. This
//! needs no on-screen text, which the picture scanner (credits.rs) reads only in Western
//! layouts, and it reads audio with ffmpeg, which opens files AVFoundation cannot.
//!
//! Each episode's tail is reduced to one 32-bit fingerprint every 64 ms (band-energy
//! differences, after Haitsma and Kalker); two episodes share audio where, at one time
//! offset, their fingerprints differ in few bits for a sustained run.
use anyhow::{Result, ensure};
use std::path::{Path, PathBuf};
use std::process::Stdio;

pub const RATE: u32 = 8000;
const FRAME: usize = 2048;
const HOP: usize = 512;
/// Seconds per fingerprint.
pub const STEP: f64 = HOP as f64 / RATE as f64;
const BANDS: usize = 33;
/// The part of a file searched: its last 10 minutes, and never its first 55%.
const TAIL_SECONDS: f64 = 600.0;
const TAIL_SHARE: f64 = 0.45;
/// Fingerprints are compared in windows this long (about 2 s)...
const WINDOW: usize = 31;
/// ...which match when fewer than this share of their bits differ (unrelated audio: 0.5).
const MATCH_ERROR: f64 = 0.33;
/// The shortest shared run that counts as closing music.
pub const MIN_RUN_SECONDS: f64 = 20.0;

/// One file's tail: where it starts in the file, and its fingerprints.
#[derive(Clone, Debug)]
pub struct Print { pub start: f64, pub duration: f64, pub hashes: Vec<u32> }

impl Print {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 + self.hashes.len() * 4);
        out.extend(self.start.to_le_bytes());
        out.extend(self.duration.to_le_bytes());
        for h in &self.hashes { out.extend(h.to_le_bytes()) }
        out
    }
    pub fn from_bytes(bytes: &[u8]) -> Option<Print> {
        if bytes.len() < 16 || (bytes.len() - 16) % 4 != 0 { return None }
        let start = f64::from_le_bytes(bytes[0..8].try_into().ok()?);
        let duration = f64::from_le_bytes(bytes[8..16].try_into().ok()?);
        let hashes = bytes[16..].chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        Some(Print { start, duration, hashes })
    }
}

/// The server's own ffmpeg, the only one it uses.
fn ffmpegs() -> Vec<PathBuf> {
    std::env::current_exe().ok().map(|p| p.with_file_name("torrbaby-ffmpeg")).into_iter()
        .chain(std::env::var_os("TORRBABY_FFMPEG").map(PathBuf::from)).filter(|p| p.is_file()).collect()
}

/// Decode the tail of `path` (of `duration` seconds) to 8 kHz mono and fingerprint it.
/// Gaps in the audio are filled with silence so times stay those of the file.
pub fn print(path: &Path, duration: f64) -> Result<Print> {
    ensure!(duration.is_finite() && duration > 120.0, "Too short to have closing credits");
    let length = TAIL_SECONDS.min(duration * TAIL_SHARE);
    let start = duration - length;
    let mut problem = "ffmpeg is not installed".to_owned();
    for helper in ffmpegs() {
        let output = std::process::Command::new("/usr/bin/nice").args(["-n", "15"]).arg(helper)
            .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-ss", &format!("{start:.3}"), "-i"]).arg(path)
            .args(["-map", "0:a:0", "-vn", "-sn", "-dn", "-af", "aresample=async=1000", "-ac", "1", "-ar", &RATE.to_string(),
                   "-t", &format!("{length:.3}"), "-f", "s16le", "-"])
            .stdin(Stdio::null()).stderr(Stdio::piped()).output()?;
        let samples: Vec<f32> = output.stdout.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect();
        if samples.len() as f64 >= length * RATE as f64 * 0.8 { return Ok(Print { start, duration, hashes: fingerprint(&samples) }) }
        problem = format!("Audio could not be read: {}", String::from_utf8_lossy(&output.stderr).lines().next().unwrap_or("too little decoded"));
    }
    anyhow::bail!(problem)
}

fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 { j ^= bit; bit >>= 1 }
        j |= bit;
        if i < j { re.swap(i, j); im.swap(i, j) }
    }
    let mut len = 2;
    while len <= n {
        let angle = -2.0 * std::f32::consts::PI / len as f32;
        let (wr, wi) = (angle.cos(), angle.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let (tr, ti) = (re[b] * cr - im[b] * ci, re[b] * ci + im[b] * cr);
                re[b] = re[a] - tr; im[b] = im[a] - ti;
                re[a] += tr; im[a] += ti;
                (cr, ci) = (cr * wr - ci * wi, cr * wi + ci * wr);
            }
        }
        len <<= 1;
    }
}

/// One 32-bit fingerprint per hop: the signs of band-energy differences across 33 bands
/// spaced logarithmically from 300 to 3000 Hz, and across consecutive frames.
pub fn fingerprint(samples: &[f32]) -> Vec<u32> {
    if samples.len() < FRAME { return Vec::new() }
    let window: Vec<f32> = (0..FRAME).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / FRAME as f32).cos()).collect();
    let bin = |hz: f64| ((hz / RATE as f64) * FRAME as f64).round() as usize;
    let edges: Vec<usize> = (0..=BANDS).map(|b| bin(300.0 * (10f64).powf(b as f64 / BANDS as f64))).collect();
    let mut previous = [0f32; BANDS];
    let mut out = Vec::with_capacity(samples.len() / HOP);
    let (mut re, mut im) = (vec![0f32; FRAME], vec![0f32; FRAME]);
    let mut first = true;
    for frame in (0..=samples.len() - FRAME).step_by(HOP) {
        for i in 0..FRAME { re[i] = samples[frame + i] * window[i]; im[i] = 0.0 }
        fft(&mut re, &mut im);
        let mut energy = [0f32; BANDS];
        for (b, e) in energy.iter_mut().enumerate() {
            let (lo, hi) = (edges[b], edges[b + 1].max(edges[b] + 1));
            *e = (lo..hi).map(|k| re[k] * re[k] + im[k] * im[k]).sum::<f32>();
        }
        if !first {
            let mut hash = 0u32;
            for m in 0..32 {
                if (energy[m] - energy[m + 1]) - (previous[m] - previous[m + 1]) > 0.0 { hash |= 1 << m }
            }
            out.push(hash);
        }
        first = false;
        previous = energy;
    }
    out
}

/// The longest run of audio two tails share: (seconds into `a`'s file, seconds into `b`'s, length in seconds).
pub fn shared(a: &Print, b: &Print) -> Option<(f64, f64, f64)> {
    let (n, m) = (a.hashes.len(), b.hashes.len());
    if n < WINDOW || m < WINDOW { return None }
    let limit = (MATCH_ERROR * 32.0 * WINDOW as f64) as u32;
    let mut errors = vec![0u32; n.max(m)];
    let mut best: Option<(usize, usize, usize)> = None;
    // `a`'s frame i against `b`'s frame i - offset.
    for offset in -(m as isize - WINDOW as isize)..=(n as isize - WINDOW as isize) {
        let first = offset.max(0) as usize;
        let last = (n as isize).min(m as isize + offset) as usize;
        if last - first < WINDOW { continue }
        for i in first..last { errors[i - first] = (a.hashes[i] ^ b.hashes[(i as isize - offset) as usize]).count_ones() }
        let count = last - first;
        let mut sum: u32 = errors[..WINDOW].iter().sum();
        let (mut run_start, mut in_run) = (0usize, false);
        for w in 0..=count - WINDOW {
            if w > 0 { sum = sum + errors[w + WINDOW - 1] - errors[w - 1] }
            let matched = sum <= limit;
            if matched && !in_run { run_start = w; in_run = true }
            if in_run && (!matched || w == count - WINDOW) {
                let end = if matched { w + WINDOW } else { w - 1 + WINDOW };
                let length = end - run_start;
                let i = first + run_start;
                if best.is_none_or(|(_, _, l)| length > l) { best = Some((i, (i as isize - offset) as usize, length)) }
                in_run = false;
            }
        }
    }
    let (i, j, length) = best?;
    Some((a.start + i as f64 * STEP, b.start + j as f64 * STEP, length as f64 * STEP))
}

/// What the siblings say about one episode's closing music.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Estimate { pub start: f64, pub agreeing: usize, pub compared: usize, pub runs: Vec<(f64, f64)> }

/// Where `print`'s closing music begins, from its runs shared with `peers` (other episodes of
/// the series): the most peers agreeing within 3 s, at least two when two or more were compared.
pub fn estimate(print: &Print, peers: &[&Print]) -> Option<Estimate> {
    let mut runs: Vec<(f64, f64)> = peers.iter().filter_map(|p| shared(print, p)).filter(|r| r.2 >= MIN_RUN_SECONDS).map(|r| (r.0, r.2)).collect();
    runs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut best: Option<(usize, f64)> = None;
    for (i, (start, _)) in runs.iter().enumerate() {
        let group: Vec<f64> = runs[i..].iter().take_while(|r| r.0 - start <= 3.0).map(|r| r.0).collect();
        if best.is_none_or(|(n, _)| group.len() > n) { best = Some((group.len(), group[group.len() / 2])) }
    }
    let (agreeing, start) = best?;
    let needed = if peers.len() >= 2 { 2 } else { 1 };
    (agreeing >= needed).then(|| Estimate { start, agreeing, compared: peers.len(), runs })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tone(seconds: f64, seed: u32) -> Vec<f32> {
        // A deterministic, music-like signal: a few drifting tones.
        let mut state = seed;
        let mut freqs = [440.0f32, 660.0, 990.0];
        (0..(seconds * RATE as f64) as usize).map(|i| {
            if i % 2000 == 0 { for f in freqs.iter_mut() { state = state.wrapping_mul(1664525).wrapping_add(1013904223); *f = 300.0 + (state >> 20) as f32 % 2500.0 } }
            freqs.iter().map(|f| (2.0 * std::f32::consts::PI * f * i as f32 / RATE as f32).sin()).sum::<f32>() / 3.0
        }).collect()
    }
    #[test]
    fn shared_music_is_found_at_its_offset() {
        let song = tone(60.0, 7);
        let mut a = tone(100.0, 1); a.extend(&song); a.extend(tone(20.0, 2));
        let mut b = tone(40.0, 3); b.extend(&song); b.extend(tone(50.0, 4));
        let pa = Print { start: 1000.0, duration: 1180.0, hashes: fingerprint(&a) };
        let pb = Print { start: 2000.0, duration: 2150.0, hashes: fingerprint(&b) };
        let (sa, sb, length) = shared(&pa, &pb).unwrap();
        assert!((sa - 1100.0).abs() < 1.5, "{sa}");
        assert!((sb - 2040.0).abs() < 1.5, "{sb}");
        assert!((length - 60.0).abs() < 3.0, "{length}");
        let unrelated = Print { start: 0.0, duration: 200.0, hashes: fingerprint(&tone(180.0, 9)) };
        assert!(shared(&pa, &unrelated).is_none_or(|r| r.2 < MIN_RUN_SECONDS));
        let back = Print::from_bytes(&pa.to_bytes()).unwrap();
        assert_eq!((back.start, back.hashes.len()), (1000.0, pa.hashes.len()));
    }
}
