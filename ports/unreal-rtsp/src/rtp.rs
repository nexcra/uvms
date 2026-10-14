use crate::{h264, Frame, VideoConfig};
use anyhow::{ensure, Result};
const MAX_FRAME: usize = 8 * 1024 * 1024;
pub(crate) struct Assembler {
    hevc: bool,
    vps: Vec<u8>,
    configuration_changed: bool,
    pt: u8,
    video: VideoConfig,
    seq: Option<u16>,
    ssrc: Option<u32>,
    timestamp: Option<u32>,
    clock: u64,
    nals: Vec<Vec<u8>>,
    bytes: usize,
    fu: Option<Vec<u8>>,
    damaged: bool,
    need_idr: bool,
}
pub(crate) struct Packet<'a> {
    pub(crate) seq: u16,
    pub(crate) timestamp: u32,
    pub(crate) ssrc: u32,
    pub(crate) payload: &'a [u8],
    pub(crate) pt: u8,
}
pub(crate) fn packet(bytes: &[u8]) -> Result<Packet<'_>> {
    ensure!(
        bytes.len() >= 12 && bytes[0] >> 6 == 2,
        "Invalid RTP header"
    );
    let mut at = 12 + 4 * (bytes[0] & 15) as usize;
    ensure!(at <= bytes.len(), "Truncated RTP CSRC");
    if bytes[0] & 16 != 0 {
        ensure!(at + 4 <= bytes.len(), "Truncated RTP extension");
        let words = u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]) as usize;
        at += 4 + words * 4;
        ensure!(at <= bytes.len(), "Truncated RTP extension body");
    }
    let mut end = bytes.len();
    if bytes[0] & 32 != 0 {
        let padding = bytes[end - 1] as usize;
        ensure!(padding > 0 && padding <= end - at, "Invalid RTP padding");
        end -= padding;
    }
    ensure!(at < end, "Empty RTP H264 payload");
    Ok(Packet {
        seq: u16::from_be_bytes([bytes[2], bytes[3]]),
        timestamp: u32::from_be_bytes(bytes[4..8].try_into().unwrap()),
        ssrc: u32::from_be_bytes(bytes[8..12].try_into().unwrap()),
        payload: &bytes[at..end],
        pt: bytes[1] & 127,
    })
}
impl Assembler {
    pub fn new(pt: u8, video: VideoConfig) -> Self {
        Self {
            hevc: false,
            vps: Vec::new(),
            configuration_changed: false,
            pt,
            video,
            seq: None,
            ssrc: None,
            timestamp: None,
            clock: 0,
            nals: Vec::new(),
            bytes: 0,
            fu: None,
            damaged: false,
            need_idr: true,
        }
    }
    pub fn video(&self) -> &VideoConfig {
        &self.video
    }
    pub fn new_hevc(pt: u8, video: VideoConfig, vps: Vec<u8>) -> Self {
        let mut result = Self::new(pt, video);
        result.hevc = true;
        result.vps = vps;
        result
    }
    pub fn ready(&self) -> bool {
        !self.video.sps.is_empty()
            && !self.video.pps.is_empty()
            && (!self.hevc || !self.vps.is_empty())
    }
    pub fn clock(&self) -> u64 {
        self.clock
    }
    fn damage(&mut self) {
        self.damaged = true;
        self.need_idr = true;
        self.fu = None;
        self.nals.clear();
        self.bytes = 0;
    }
    pub fn push(&mut self, bytes: &[u8]) -> Result<Option<Frame>> {
        let p = match packet(bytes) {
            Ok(p) => p,
            Err(_) => {
                self.damage();
                return Ok(None);
            }
        };
        if p.pt != self.pt {
            return Ok(None);
        }
        if self.ssrc.is_some_and(|ssrc| ssrc != p.ssrc) {
            self.damage();
            self.seq = None;
            self.timestamp = None;
        }
        self.ssrc = Some(p.ssrc);
        let mut gap = false;
        if let Some(last) = self.seq {
            let delta = p.seq.wrapping_sub(last);
            if delta == 0 || delta > 0x8000 {
                return Ok(None);
            } // TCP does not reorder; discard duplicates/late packets.
            gap = delta != 1;
        }
        self.seq = Some(p.seq);
        let mut completed = None;
        if let Some(last) = self.timestamp {
            if p.timestamp != last {
                let delta = p.timestamp.wrapping_sub(last);
                ensure!(!self.hevc || delta<=90000*10,"HEVC reordered timestamps/B-frames unsupported; configure low-delay IP camera output");
                if delta == 0 || delta > 90000 * 10 {
                    self.damage();
                    self.clock = self.clock.saturating_add(3000);
                } else {
                    // Any gap straddling the boundary could be a missing final NAL of the previous frame.
                    if gap {
                        self.damage();
                    }
                    completed = self.finish(delta);
                    self.clock = self.clock.saturating_add(delta as u64);
                }
                self.nals.clear();
                self.bytes = 0;
                self.fu = None;
                self.damaged = false;
            } else if gap {
                self.damage();
            }
        }
        self.timestamp = Some(p.timestamp);
        if !self.damaged && self.payload(p.payload).is_err() {
            self.damage();
        }
        ensure!(
            !self.configuration_changed,
            "HEVC parameter sets changed; reconnect required"
        );
        Ok(completed)
    }
    fn finish(&mut self, duration: u32) -> Option<Frame> {
        if self.damaged || self.fu.is_some() || self.nals.is_empty() {
            self.need_idr = true;
            return None;
        }
        let key = self.nals.iter().any(|n| {
            if self.hevc {
                matches!((n[0] >> 1) & 63, 16..=21)
            } else {
                n[0] & 31 == 5
            }
        });
        let has_slice = self.nals.iter().any(|n| {
            if self.hevc {
                (n[0] >> 1) & 63 <= 31
            } else {
                matches!(n[0] & 31, 1..=5)
            }
        });
        if !has_slice || (self.need_idr && !key) || !self.ready() {
            return None;
        }
        self.need_idr = false;
        let mut avcc = Vec::with_capacity(self.bytes + 128);
        // Native0x180010250 inserts the pair if both were not already seen.
        if key && self.hevc {
            append(&mut avcc, &self.vps);
            append(&mut avcc, &self.video.sps);
            append(&mut avcc, &self.video.pps);
        }
        if key
            && !self.hevc
            && !(self.nals.iter().any(|n| n[0] & 31 == 7)
                && self.nals.iter().any(|n| n[0] & 31 == 8))
        {
            append(&mut avcc, &self.video.sps);
            append(&mut avcc, &self.video.pps);
        }
        for nal in &self.nals {
            append(&mut avcc, nal);
        }
        if avcc.len() > MAX_FRAME {
            self.need_idr = true;
            return None;
        }
        Some(Frame {
            avcc,
            timestamp_90k: self.clock,
            duration_90k: duration,
            keyframe: key,
        })
    }
    fn add(&mut self, nal: Vec<u8>) -> Result<()> {
        if self.hevc {
            return self.add_hevc(nal);
        }
        ensure!(
            !nal.is_empty() && nal[0] & 0x80 == 0 && matches!(nal[0] & 31, 1..=23),
            "Invalid H264 NAL"
        );
        ensure!(
            self.bytes + nal.len() + 4 <= MAX_FRAME && self.nals.len() < 256,
            "H264 access unit too large"
        );
        match nal[0] & 31 {
            7 => {
                ensure!(nal.len() <= 65535, "SPS too large");
                let (width, height) = h264::dimensions(&nal)?;
                if !self.video.sps.is_empty() && self.video.sps != nal {
                    self.need_idr = true;
                }
                self.video.sps = nal.clone();
                self.video.width = width;
                self.video.height = height;
            }
            8 => {
                ensure!(nal.len() <= 65535, "PPS too large");
                if !self.video.pps.is_empty() && self.video.pps != nal {
                    self.need_idr = true;
                }
                self.video.pps = nal.clone();
            }
            _ => {}
        }
        self.bytes += nal.len() + 4;
        self.nals.push(nal);
        Ok(())
    }
    fn payload(&mut self, p: &[u8]) -> Result<()> {
        if self.hevc {
            return self.payload_hevc(p);
        }
        ensure!(p[0] & 0x80 == 0, "Forbidden H264 bit");
        match p[0] & 31 {
            1..=23 => {
                ensure!(self.fu.is_none(), "Incomplete fragmented NAL");
                self.add(p.to_vec())?;
            }
            24 => {
                ensure!(self.fu.is_none(), "Incomplete fragmented NAL");
                let mut at = 1;
                while at < p.len() {
                    ensure!(at + 2 <= p.len(), "Truncated STAP-A length");
                    let n = u16::from_be_bytes([p[at], p[at + 1]]) as usize;
                    at += 2;
                    ensure!(n > 0 && at + n <= p.len(), "Invalid STAP-A NAL size");
                    self.add(p[at..at + n].to_vec())?;
                    at += n;
                }
                ensure!(at > 1, "Empty STAP-A");
            }
            28 => {
                ensure!(
                    p.len() > 2 && p[1] & 32 == 0 && matches!(p[1] & 31, 1..=23),
                    "Invalid FU-A"
                );
                let start = p[1] & 128 != 0;
                let end = p[1] & 64 != 0;
                ensure!(!(start && end), "Invalid FU-A flags");
                let header = (p[0] & 0xe0) | (p[1] & 31);
                if start {
                    ensure!(self.fu.is_none(), "Overlapping FU-A");
                    let mut nal = vec![header];
                    nal.extend_from_slice(&p[2..]);
                    self.fu = Some(nal);
                } else {
                    let nal = self
                        .fu
                        .as_mut()
                        .ok_or_else(|| anyhow::anyhow!("FU-A continuation without start"))?;
                    ensure!(
                        nal[0] == header && nal.len() + p.len() - 2 + self.bytes <= MAX_FRAME,
                        "FU-A mismatch/overflow"
                    );
                    nal.extend_from_slice(&p[2..]);
                    if end {
                        let nal = self.fu.take().unwrap();
                        self.add(nal)?;
                    }
                }
            }
            _ => anyhow::bail!("Unsupported H264 packetization type"),
        }
        Ok(())
    }
    fn add_hevc(&mut self, nal: Vec<u8>) -> Result<()> {
        ensure!(
            nal.len() >= 2 && nal[0] & 0x81 == 0 && nal[1] & 0xf8 == 0 && nal[1] & 7 != 0,
            "Invalid HEVC NAL/layer header"
        );
        let kind = (nal[0] >> 1) & 63;
        ensure!(
            kind < 48 && self.bytes + nal.len() + 4 <= MAX_FRAME && self.nals.len() < 256,
            "HEVC NAL size/type limit"
        );
        if matches!(kind, 32..=34) {
            ensure!(nal.len() <= 65535, "HEVC parameter set too large");
            let dest = match kind {
                32 => &mut self.vps,
                33 => &mut self.video.sps,
                _ => &mut self.video.pps,
            };
            if !dest.is_empty() && *dest != nal {
                self.configuration_changed = true;
            }
            *dest = nal.clone();
        }
        self.bytes += nal.len() + 4;
        self.nals.push(nal);
        Ok(())
    }
    fn payload_hevc(&mut self, p: &[u8]) -> Result<()> {
        ensure!(
            p.len() >= 2 && p[0] & 0x81 == 0 && p[1] & 0xf8 == 0 && p[1] & 7 != 0,
            "Invalid HEVC RTP header"
        );
        match (p[0] >> 1) & 63 {
            0..=47 => {
                ensure!(self.fu.is_none(), "Incomplete HEVC FU");
                self.add_hevc(p.to_vec())?;
            }
            48 => {
                ensure!(self.fu.is_none(), "Incomplete HEVC FU");
                let mut at = 2;
                let mut nals = Vec::new();
                while at < p.len() {
                    ensure!(at + 2 <= p.len(), "Truncated HEVC AP length");
                    let n = u16::from_be_bytes([p[at], p[at + 1]]) as usize;
                    at += 2;
                    ensure!(
                        n >= 2 && at + n <= p.len() && nals.len() < 256,
                        "Invalid HEVC AP NAL size"
                    );
                    nals.push(p[at..at + n].to_vec());
                    at += n;
                }
                ensure!(!nals.is_empty(), "Empty HEVC AP");
                for nal in nals {
                    self.add_hevc(nal)?;
                }
            }
            49 => {
                ensure!(p.len() > 3 && p[2] & 63 < 48, "Invalid HEVC FU");
                let start = p[2] & 128 != 0;
                let end = p[2] & 64 != 0;
                ensure!(!(start && end), "Invalid HEVC FU flags");
                let header = [(p[0] & 0x81) | ((p[2] & 63) << 1), p[1]];
                if start {
                    ensure!(self.fu.is_none(), "Overlapping HEVC FU");
                    let mut n = header.to_vec();
                    n.extend_from_slice(&p[3..]);
                    self.fu = Some(n);
                } else {
                    let n = self
                        .fu
                        .as_mut()
                        .ok_or_else(|| anyhow::anyhow!("HEVC FU without start"))?;
                    ensure!(
                        n[..2] == header && n.len() + p.len() - 3 + self.bytes <= MAX_FRAME,
                        "HEVC FU mismatch/overflow"
                    );
                    n.extend_from_slice(&p[3..]);
                    if end {
                        let n = self.fu.take().unwrap();
                        self.add_hevc(n)?;
                    }
                }
            }
            _ => anyhow::bail!("HEVC PACI/DON unsupported"),
        }
        Ok(())
    }
}
fn append(out: &mut Vec<u8>, nal: &[u8]) {
    out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
    out.extend_from_slice(nal);
}
#[cfg(test)]
mod tests {
    use super::*;
    fn hevc() -> Assembler {
        Assembler::new_hevc(
            96,
            VideoConfig {
                sps: vec![66, 1, 2],
                pps: vec![68, 1, 3],
                width: 0,
                height: 0,
            },
            vec![64, 1, 1],
        )
    }
    #[test]
    fn hevc_fu_reassembles_idr_with_parameter_sets_and_wrap() {
        let mut a = hevc();
        a.push(&rtp(65535, u32::MAX - 2999, &[98, 1, 128 | 19, 10, 20]))
            .unwrap();
        a.push(&rtp(0, u32::MAX - 2999, &[98, 1, 64 | 19, 30, 40]))
            .unwrap();
        let f = a.push(&rtp(1, 0, &[2, 1, 9])).unwrap().unwrap();
        assert!(f.keyframe);
        assert_eq!(f.duration_90k, 3000);
        assert!(f.avcc.starts_with(&[0, 0, 0, 3, 64, 1, 1]));
        assert!(f.avcc.ends_with(&[0, 0, 0, 6, 38, 1, 10, 20, 30, 40]));
    }
    #[test]
    fn hevc_ap_loss_malformed_and_format_change() {
        let mut a = hevc();
        a.push(&rtp(1, 0, &[96, 1, 0, 3, 38, 1, 9, 0, 3, 38, 1, 8]))
            .unwrap();
        let f = a.push(&rtp(2, 3000, &[2, 1, 7])).unwrap().unwrap();
        assert!(f.keyframe);
        a.push(&rtp(3, 6000, &[98, 1, 128 | 19, 1])).unwrap();
        a.push(&rtp(5, 6000, &[98, 1, 64 | 19, 2])).unwrap();
        assert!(a.push(&rtp(6, 9000, &[2, 1, 7])).unwrap().is_none());
        a.push(&rtp(7, 12000, &[96, 1, 0, 3, 38, 1, 9, 0, 7, 1]))
            .unwrap();
        assert!(a.push(&rtp(8, 15000, &[2, 1, 7])).unwrap().is_none());
        assert!(a.push(&rtp(9, 18000, &[64, 1, 99])).is_err());
    }
    #[test]
    fn hevc_reordered_pts_rejected_explicitly() {
        let mut a = hevc();
        a.push(&rtp(1, 6000, &[38, 1, 7])).unwrap();
        assert!(a.push(&rtp(2, 3000, &[2, 1, 7])).is_err());
    }
    fn config() -> VideoConfig {
        VideoConfig {
            sps: vec![0x67, 1],
            pps: vec![0x68, 2],
            width: 640,
            height: 360,
        }
    }
    fn rtp(seq: u16, ts: u32, p: &[u8]) -> Vec<u8> {
        let mut b = vec![0x80, 96];
        b.extend(seq.to_be_bytes());
        b.extend(ts.to_be_bytes());
        b.extend(1u32.to_be_bytes());
        b.extend(p);
        b
    }
    #[test]
    fn fu_assembly_and_native_parameter_insertion() {
        let mut a = Assembler::new(96, config());
        assert!(a.push(&rtp(1, 100, &[0x7c, 0x85, 1, 2])).unwrap().is_none());
        a.push(&rtp(2, 100, &[0x7c, 0x45, 3, 4])).unwrap();
        let f = a.push(&rtp(3, 3100, &[0x41, 9])).unwrap().unwrap();
        assert!(f.keyframe);
        assert_eq!(f.duration_90k, 3000);
        assert!(f.avcc.ends_with(&[0, 0, 0, 5, 0x65, 1, 2, 3, 4]));
    }
    #[test]
    fn lost_fragment_discards_until_idr() {
        let mut a = Assembler::new(96, config());
        a.push(&rtp(1, 0, &[0x7c, 0x85, 1])).unwrap();
        a.push(&rtp(3, 0, &[0x7c, 0x45, 2])).unwrap();
        assert!(a.push(&rtp(4, 3000, &[0x41, 3])).unwrap().is_none());
        assert!(a.push(&rtp(5, 6000, &[0x65, 4])).unwrap().is_none());
        let f = a.push(&rtp(6, 9000, &[0x41, 5])).unwrap().unwrap();
        assert_eq!(f.timestamp_90k, 6000);
        assert!(f.keyframe);
    }
    #[test]
    fn stap_and_wrap() {
        let mut a = Assembler::new(96, config());
        a.push(&rtp(
            65535,
            u32::MAX - 2999,
            &[24, 0, 2, 0x65, 9, 0, 2, 0x65, 10],
        ))
        .unwrap();
        let f = a.push(&rtp(0, 0, &[0x41, 1])).unwrap().unwrap();
        assert_eq!(f.duration_90k, 3000);
        assert!(f
            .avcc
            .ends_with(&[0, 0, 0, 2, 0x65, 9, 0, 0, 0, 2, 0x65, 10]));
    }
    #[test]
    fn malformed_stap_never_partial_frame() {
        let mut a = Assembler::new(96, config());
        a.push(&rtp(1, 0, &[24, 0, 2, 0x65, 9, 0, 8, 0x65]))
            .unwrap();
        assert!(a.push(&rtp(2, 3000, &[0x41, 1])).unwrap().is_none());
    }
    #[test]
    fn truncated_headers_and_padding() {
        for n in 0..12 {
            assert!(packet(&vec![0; n]).is_err());
        }
        let mut p = rtp(1, 0, &[0x65, 1]);
        p[0] |= 32;
        p.push(255);
        assert!(packet(&p).is_err());
    }
    #[test]
    fn original_dll_corpus_roundtrips_single_stap_and_fu_byte_exactly() {
        let idr = include_bytes!("../tests/fixtures/original-idr.avcc");
        let pframe = include_bytes!("../tests/fixtures/original-p.avcc");
        let split = |bytes: &[u8]| {
            let mut at = 0;
            let mut nals = Vec::new();
            while at < bytes.len() {
                let n = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
                at += 4;
                nals.push(bytes[at..at + n].to_vec());
                at += n;
            }
            nals
        };
        let nals = split(idr);
        let mut a = Assembler::new(
            96,
            VideoConfig {
                sps: nals[0].clone(),
                pps: nals[1].clone(),
                width: 640,
                height: 360,
            },
        );
        let mut seq = 1u16;
        let mut stap = vec![24];
        for nal in &nals[..2] {
            stap.extend((nal.len() as u16).to_be_bytes());
            stap.extend(nal);
        }
        a.push(&rtp(seq, 0, &stap)).unwrap();
        seq += 1;
        for nal in &nals[2..] {
            let chunks = nal[1..].chunks(137).collect::<Vec<_>>();
            for (i, chunk) in chunks.iter().enumerate() {
                let flags = if i == 0 {
                    128
                } else if i + 1 == chunks.len() {
                    64
                } else {
                    0
                };
                let mut fu = vec![(nal[0] & 0xe0) | 28, (nal[0] & 31) | flags];
                fu.extend_from_slice(chunk);
                a.push(&rtp(seq, 0, &fu)).unwrap();
                seq += 1;
            }
        }
        let pnals = split(pframe);
        let restored = a.push(&rtp(seq, 6000, &pnals[0])).unwrap().unwrap();
        seq += 1;
        assert_eq!(restored.avcc, idr);
        for nal in &pnals[1..] {
            a.push(&rtp(seq, 6000, nal)).unwrap();
            seq += 1;
        }
        let restored = a.push(&rtp(seq, 12000, &[0x41, 1])).unwrap().unwrap();
        assert_eq!(restored.avcc, pframe);
    }
}
