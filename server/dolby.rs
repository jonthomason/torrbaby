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
