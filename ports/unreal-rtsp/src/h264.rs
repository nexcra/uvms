use anyhow::{bail, ensure, Result};
struct Bits {
    bytes: Vec<u8>,
    at: usize,
}
impl Bits {
    fn new(nal: &[u8]) -> Result<Self> {
        ensure!(nal.len() > 3 && nal[0] & 31 == 7, "Invalid H264 SPS");
        let mut bytes = Vec::new();
        let mut zeros = 0;
        for &byte in &nal[1..] {
            if zeros >= 2 && byte == 3 {
                zeros = 0;
                continue;
            }
            bytes.push(byte);
            zeros = if byte == 0 { zeros + 1 } else { 0 };
        }
        Ok(Self { bytes, at: 0 })
    }
    fn read(&mut self, n: usize) -> Result<u32> {
        ensure!(
            n <= 32 && self.at + n <= self.bytes.len() * 8,
            "Truncated H264 SPS"
        );
        let mut out = 0;
        for _ in 0..n {
            out = (out << 1) | ((self.bytes[self.at / 8] >> (7 - self.at % 8)) & 1) as u32;
            self.at += 1;
        }
        Ok(out)
    }
    fn ue(&mut self) -> Result<u32> {
        let mut zeros = 0;
        while self.read(1)? == 0 {
            zeros += 1;
            ensure!(zeros < 31, "Oversized SPS code");
        }
        Ok(((1u32 << zeros) - 1) + self.read(zeros)?)
    }
    fn se(&mut self) -> Result<i32> {
        let v = self.ue()?;
        Ok(if v & 1 != 0 {
            (v / 2 + 1) as i32
        } else {
            -((v / 2) as i32)
        })
    }
}
pub fn dimensions(sps: &[u8]) -> Result<(u32, u32)> {
    let mut b = Bits::new(sps)?;
    let profile = b.read(8)?;
    b.read(16)?;
    b.ue()?;
    let mut chroma = 1;
    let mut separate = 0;
    if [100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135].contains(&profile) {
        chroma = b.ue()?;
        ensure!(chroma <= 3, "Invalid chroma format");
        if chroma == 3 {
            separate = b.read(1)?;
        }
        ensure!(b.ue()? <= 6 && b.ue()? <= 6, "Invalid bit depth");
        b.read(1)?;
        if b.read(1)? != 0 {
            for i in 0..if chroma == 3 { 12 } else { 8 } {
                if b.read(1)? != 0 {
                    let mut last = 8;
                    let mut next = 8;
                    for _ in 0..if i < 6 { 16 } else { 64 } {
                        if next != 0 {
                            next = (last + b.se()? + 256).rem_euclid(256);
                        }
                        if next != 0 {
                            last = next;
                        }
                    }
                }
            }
        }
    }
    ensure!(b.ue()? <= 12, "Invalid frame_num size");
    let order = b.ue()?;
    match order {
        0 => {
            ensure!(b.ue()? <= 12, "Invalid POC size");
        }
        1 => {
            b.read(1)?;
            b.se()?;
            b.se()?;
            let n = b.ue()?;
            ensure!(n <= 255, "Invalid POC cycle");
            for _ in 0..n {
                b.se()?;
            }
        }
        2 => {}
        _ => bail!("Invalid POC mode"),
    }
    b.ue()?;
    b.read(1)?;
    let w = b.ue()?;
    let h = b.ue()?;
    ensure!(w < 512 && h < 512, "H264 dimensions exceed 8192");
    let frame = b.read(1)?;
    if frame == 0 {
        b.read(1)?;
    }
    b.read(1)?;
    let (mut left, mut right, mut top, mut bottom) = (0, 0, 0, 0);
    if b.read(1)? != 0 {
        left = b.ue()?;
        right = b.ue()?;
        top = b.ue()?;
        bottom = b.ue()?;
    }
    let array = if separate != 0 { 0 } else { chroma };
    let crop_x = if array == 1 || array == 2 { 2 } else { 1 };
    let crop_y = if array == 1 { 2 } else { 1 } * (2 - frame);
    let width = (w as u64 + 1) * 16;
    let height = (h as u64 + 1) * 16 * (2 - frame) as u64;
    let cx = (left as u64 + right as u64) * crop_x as u64;
    let cy = (top as u64 + bottom as u64) * crop_y as u64;
    ensure!(
        width > cx && height > cy && width - cx <= 8192 && height - cy <= 8192,
        "Invalid H264 crop"
    );
    Ok(((width - cx) as u32, (height - cy) as u32))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn original_capture_sps() {
        let sps = [
            0x67, 0x42, 0xc0, 0x16, 0xda, 0x02, 0x80, 0xbf, 0xe5, 0xc0, 0x44, 0, 0, 3, 0, 4, 0, 0,
            3, 0, 0x7a, 0x3c, 0x58, 0xba, 0x80,
        ];
        assert_eq!(dimensions(&sps).unwrap(), (640, 360));
        for end in 0..9 {
            assert!(dimensions(&sps[..end]).is_err());
        }
    }
}
