//! AC-3 framing and MP4 configuration from ETSI TS 102 366, sections 4 and F.4.
//! Parses headers only; compressed audio is never decoded or modified here.
use anyhow::{Result, ensure};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ac3 {
    pub dac3: [u8; 3],
    pub sample_rate: u32,
    pub channels: u16,
    pub frame_bytes: usize,
}
struct Bits<'a> {
    data: &'a [u8],
    position: usize,
}
impl Bits<'_> {
    fn read(&mut self, count: usize) -> Result<u32> {
        ensure!(
            count <= 24 && self.position + count <= self.data.len() * 8,
            "Truncated AC-3 header"
        );
        let mut value = 0;
        for _ in 0..count {
            value = (value << 1)
                | ((self.data[self.position / 8] >> (7 - self.position % 8)) & 1) as u32;
            self.position += 1;
        }
        Ok(value)
    }
}
pub fn ac3(packet: &[u8]) -> Result<Ac3> {
    ensure!(
        (7..=3840).contains(&packet.len()),
        "Invalid AC-3 packet length"
    );
    let mut b = Bits {
        data: packet,
        position: 0,
    };
    ensure!(b.read(16)? == 0x0b77, "Invalid AC-3 syncword");
    b.read(16)?; // CRC1; the native decoder validates compressed payload integrity.
    let fscod = b.read(2)?;
    let size_code = b.read(6)?;
    ensure!(
        fscod < 3 && size_code < 38,
        "Reserved AC-3 sample rate/frame size"
    );
    let bsid = b.read(5)?;
    ensure!(bsid <= 8, "AC-3 bitstream version needs additional support");
    let bsmod = b.read(3)?;
    let acmod = b.read(3)?;
    if acmod & 1 != 0 && acmod != 1 {
        b.read(2)?;
    }
    if acmod & 4 != 0 {
        b.read(2)?;
    }
    if acmod == 2 {
        b.read(2)?;
    }
    let lfe = b.read(1)?;
    let rate = [48000, 44100, 32000][fscod as usize];
    let bit_rate_code = size_code / 2;
    let kbps = [
        32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 448, 512, 576, 640,
    ][bit_rate_code as usize];
    let words_44100 = [
        69, 87, 104, 121, 139, 174, 208, 243, 278, 348, 417, 487, 557, 696, 835, 975, 1114, 1253,
        1393,
    ];
    let frame_bytes = match fscod {
        0 => kbps * 4,
        1 => (words_44100[bit_rate_code as usize] + size_code % 2) * 2,
        _ => kbps * 6,
    } as usize;
    ensure!(
        packet.len() == frame_bytes,
        "AC-3 packet must contain exactly one complete syncframe"
    );
    let fields = (fscod << 22)
        | (bsid << 17)
        | (bsmod << 14)
        | (acmod << 11)
        | (lfe << 10)
        | (bit_rate_code << 5);
    let bytes = fields.to_be_bytes();
    Ok(Ac3 {
        dac3: [bytes[1], bytes[2], bytes[3]],
        sample_rate: rate,
        channels: [2, 1, 2, 3, 3, 4, 4, 5][acmod as usize] + lfe as u16,
        frame_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn standard_48khz_192k_51_header() {
        let mut frame = vec![0; 768];
        frame[..7].copy_from_slice(&[0x0b, 0x77, 0, 0, 0x14, 0x40, 0xe1]);
        let parsed = ac3(&frame).unwrap();
        assert_eq!(parsed.dac3, [0x10, 0x3d, 0x40]);
        assert_eq!(
            (parsed.sample_rate, parsed.channels, parsed.frame_bytes),
            (48000, 6, 768)
        );
        assert!(ac3(&frame[..767]).is_err());
        frame[5] = 0x80; // E-AC-3 is a different header, not AC-3 with a new label.
        assert!(ac3(&frame).is_err());
    }
}

/// MP4 configuration for a standalone six-block E-AC-3 independent substream.
/// Dolby Atmos JOC is signalled; dependent/extended layouts are rejected.
/// This is intentionally not a decoder: all original packet bytes are retained.
pub fn configuration(codec: &str, packet: &[u8]) -> Result<(Vec<u8>, u32, u16)> {
    if codec == "ac3" {
        let c = ac3(packet)?;
        return Ok((c.dac3.to_vec(), c.sample_rate, c.channels));
    }
    ensure!(codec == "eac3", "Unknown Dolby codec");
    ensure!(
        (7..=4096).contains(&packet.len()),
        "Invalid E-AC-3 packet length"
    );
    let mut b = Bits {
        data: packet,
        position: 0,
    };
    ensure!(b.read(16)? == 0x0b77, "Invalid E-AC-3 syncword");
    ensure!(
        b.read(2)? == 0 && b.read(3)? == 0,
        "E-AC-3 dependent/converted/multiple substreams need additional support"
    );
    let frame_bytes = (b.read(11)? as usize + 1) * 2;
    ensure!(
        frame_bytes == packet.len(),
        "E-AC-3 packet must contain exactly one complete syncframe"
    );
    let fscod = b.read(2)?;
    ensure!(fscod < 3, "Half-rate E-AC-3 needs additional support");
    ensure!(
        b.read(2)? == 3,
        "Short-block E-AC-3 needs access-unit assembly"
    );
    let acmod = b.read(3)?;
    let lfe = b.read(1)?;
    let bsid = b.read(5)?;
    ensure!(bsid == 16, "Unsupported E-AC-3 bitstream version");
    b.read(5)?; // dialnorm
    if b.read(1)? != 0 {
        b.read(8)?;
    }
    if acmod == 0 {
        b.read(5)?;
        if b.read(1)? != 0 {
            b.read(8)?;
        }
    }
    if b.read(1)? != 0 {
        // mixing metadata, ETSI E.1.2.2
        if acmod > 2 {
            b.read(2)?;
        }
        if acmod & 1 != 0 && acmod > 2 {
            b.read(6)?;
        }
        if acmod & 4 != 0 {
            b.read(6)?;
        }
        if lfe != 0 && b.read(1)? != 0 {
            b.read(5)?;
        }
        if b.read(1)? != 0 {
            b.read(6)?;
        }
        if acmod == 0 && b.read(1)? != 0 {
            b.read(6)?;
        }
        if b.read(1)? != 0 {
            b.read(6)?;
        }
        match b.read(2)? {
            0 => (),
            1 => {
                b.read(5)?;
            }
            2 => {
                b.read(12)?;
            }
            _ => anyhow::bail!("Extended E-AC-3 mixing metadata needs additional support"),
        }
        if acmod < 2 {
            if b.read(1)? != 0 {
                b.read(14)?;
            }
            if acmod == 0 && b.read(1)? != 0 {
                b.read(14)?;
            }
        }
        if b.read(1)? != 0 {
            for _ in 0..6 {
                if b.read(1)? != 0 {
                    b.read(5)?;
                }
            }
        }
    }
    let bsmod = if b.read(1)? != 0 {
        let mode = b.read(3)?;
        b.read(2)?;
        if acmod == 2 {
            b.read(4)?;
        }
        if acmod >= 6 {
            b.read(2)?;
        }
        if b.read(1)? != 0 {
            b.read(8)?;
        }
        if acmod == 0 && b.read(1)? != 0 {
            b.read(8)?;
        }
        b.read(1)?;
        mode
    } else {
        0
    };
    // Additional bitstream info carries Dolby Atmos JOC signalling (ETSI TS
    // 103 420): flag_ec3_extension_type_a, then complexity_index_type_a. The
    // core channels decode without it; dec3 repeats it so players see Atmos.
    let joc = if b.read(1)? != 0 {
        let bytes = b.read(6)? + 1;
        // The flag is the least significant bit of the first byte (as in FFmpeg).
        b.read(7)?;
        if bytes >= 2 && b.read(1)? != 0 { Some(b.read(8)? as u8) } else { None }
    } else {
        None
    };
    let rate = [48000, 44100, 32000][fscod as usize];
    let kbps = (packet.len() as u64 * 8 * rate as u64).div_ceil(1536 * 1000);
    ensure!(kbps < 8192, "E-AC-3 bitrate exceeds MP4 field");
    // data_rate:13, num_ind_sub:3; fscod:2, bsid:5, reserved:1,
    // asvc:1 (main service), bsmod:3, acmod:3, lfe:1, reserved:3,
    // num_dep_sub:4 (none), reserved:1.
    let fields = (fscod << 22) | (bsid << 17) | (bsmod << 12) | (acmod << 9) | (lfe << 8);
    let bits = fields.to_be_bytes();
    let rate_bits = ((kbps as u16) << 3).to_be_bytes();
    let mut config = vec![rate_bits[0], rate_bits[1], bits[1], bits[2], bits[3]];
    // reserved:7, flag_ec3_extension_type_a:1, complexity_index_type_a:8.
    if let Some(complexity) = joc {
        config.extend([1, complexity]);
    }
    Ok((
        config,
        rate,
        [2, 1, 2, 3, 3, 4, 4, 5][acmod as usize] + lfe as u16,
    ))
}
