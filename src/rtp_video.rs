//! Shared immutable H264 RTP payloads; parameter-set state stays per viewer.
use anyhow::Result;
use bytes::Bytes;
use webrtc::rtp::{codecs::h264::H264Payloader, packetizer::Payloader};

enum Operation {
    Sps(Bytes),
    Pps(Bytes),
    Media(Vec<Bytes>),
}

pub(crate) struct PreparedFrame {
    operations: Vec<Operation>,
    capacity: usize,
    mtu: usize,
}

impl PreparedFrame {
    pub fn new(annex_b: Bytes, mtu: usize) -> Result<Self> {
        let mut operations = Vec::new();
        let mut capacity = 0;
        // Match the installed payloader's Annex B boundaries (including extra
        // zero bytes before a start code). Scan once, rather than per viewer.
        let mut start = None;
        let mut zeros = 0;
        let mut ranges = Vec::new();
        for (at, &byte) in annex_b.iter().enumerate() {
            if byte == 1 && zeros >= 2 {
                if let Some(from) = start {
                    ranges.push(from..at - zeros);
                }
                start = Some(at + 1);
            }
            zeros = if byte == 0 { zeros + 1 } else { 0 };
        }
        ranges.push(start.unwrap_or(0)..annex_b.len());
        for range in ranges {
            let nal = annex_b.slice(range);
            let Some(header) = nal.first() else { continue };
            match header & 31 {
                7 => operations.push(Operation::Sps(Bytes::copy_from_slice(&nal))),
                8 => operations.push(Operation::Pps(Bytes::copy_from_slice(&nal))),
                9 | 12 => {}
                _ => {
                    // Delegate FU-A and single-NAL formatting to the same pinned
                    // library as before. No parameter sets are pending here.
                    let packets = H264Payloader::default().payload(mtu, &nal)?;
                    capacity += packets.len() + 1; // possible per-viewer STAP-A
                    operations.push(Operation::Media(packets));
                }
            }
        }
        Ok(Self {
            operations,
            capacity,
            mtu,
        })
    }
}

#[derive(Default)]
pub(crate) struct Packetizer {
    sps: Option<Bytes>,
    pps: Option<Bytes>,
}

impl Packetizer {
    #[cfg(test)]
    pub fn payloads(&mut self, frame: &PreparedFrame) -> Vec<Bytes> {
        let mut output = Vec::with_capacity(frame.capacity);
        self.payloads_into(frame, &mut output);
        output
    }

    /// Caller drains the list after sending; reuse its allocation next frame.
    /// Large frames cannot grow the retained list without bound.
    pub fn payloads_into(&mut self, frame: &PreparedFrame, output: &mut Vec<Bytes>) {
        output.clear();
        if output.capacity() > 4096 && frame.capacity < 1024 {
            *output = Vec::with_capacity(frame.capacity);
        }
        output.reserve(frame.capacity);
        for operation in &frame.operations {
            match operation {
                Operation::Sps(sps) => self.sps = Some(sps.clone()),
                Operation::Pps(pps) => self.pps = Some(pps.clone()),
                Operation::Media(packets) => {
                    if self.sps.is_some() && self.pps.is_some() {
                        let sps = self.sps.take().unwrap();
                        let pps = self.pps.take().unwrap();
                        let size = 5 + sps.len() + pps.len();
                        // Preserve the previous library's STAP-A ordering and
                        // oversized-pair behavior, including state consumption.
                        if size <= frame.mtu {
                            let mut stap = Vec::with_capacity(size);
                            stap.push(0x78);
                            stap.extend_from_slice(&(sps.len() as u16).to_be_bytes());
                            stap.extend_from_slice(&sps);
                            stap.extend_from_slice(&(pps.len() as u16).to_be_bytes());
                            stap.extend_from_slice(&pps);
                            output.push(Bytes::from(stap));
                        }
                    }
                    output.extend(packets.iter().cloned());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn annex(nals: &[Vec<u8>]) -> Bytes {
        let mut out = Vec::new();
        for nal in nals {
            out.extend([0, 0, 0, 1]);
            out.extend(nal);
        }
        out.into()
    }

    #[test]
    fn drained_packet_list_reuses_storage_without_repeating_parameter_sets() {
        let frame = PreparedFrame::new(
            annex(&[vec![0x67, 1], vec![0x68, 2], vec![0x65; 9000]]),
            1100,
        )
        .unwrap();
        let mut packetizer = Packetizer::default();
        let mut list = Vec::new();
        packetizer.payloads_into(&frame, &mut list);
        let pointer = list.as_ptr();
        let expected = list.clone();
        for packet in list.drain(..) {
            drop(packet);
        }
        packetizer.payloads_into(&frame, &mut list);
        assert_eq!(list.as_ptr(), pointer);
        assert_eq!(list, expected);
        let video = PreparedFrame::new(annex(&[vec![0x61; 9000]]), 1100).unwrap();
        packetizer.payloads_into(&video, &mut list);
        assert_eq!(list.len() + 1, expected.len());
        assert_eq!(list.as_ptr(), pointer);
    }

    #[test]
    fn shared_payloads_match_existing_payloader_across_frame_state_and_rejoins() {
        let mtu = 1100;
        let mut original = H264Payloader::default();
        let mut shared = Packetizer::default();
        // Parameter-only frames, split pairs, AUD/filler, MTU boundaries,
        // oversized pairs, changed configuration and late viewer joins.
        let frames = vec![
            vec![vec![0x67, 1, 2]],
            vec![vec![0x09, 1], vec![0x68, 3, 4]],
            vec![vec![0x0c, 1], vec![0x65; 1100]],
            vec![vec![0x61; 1101]],
            vec![vec![0x65; 2197], vec![0x67, 5]],
            vec![vec![0x68, 6], vec![0x61; 2200]],
            vec![vec![0x67; 700], vec![0x68; 700], vec![0x65; 9000]],
            vec![vec![0x67, 7], vec![0x68, 8], vec![0x65; 16384]],
        ];
        for frame in frames {
            let bytes = annex(&frame);
            let prepared = PreparedFrame::new(bytes.clone(), mtu).unwrap();
            assert_eq!(
                shared.payloads(&prepared),
                original.payload(mtu, &bytes).unwrap()
            );
            assert_eq!(
                Packetizer::default().payloads(&prepared),
                H264Payloader::default().payload(mtu, &bytes).unwrap()
            );
        }
    }

    #[test]
    fn fragmentation_storage_is_shared_and_viewer_state_is_independent() {
        let parameters = PreparedFrame::new(annex(&[vec![0x67, 1], vec![0x68, 2]]), 1100).unwrap();
        let frame = PreparedFrame::new(annex(&[vec![0x65; 9000]]), 1100).unwrap();
        let mut one = Packetizer::default();
        let mut two = Packetizer::default();
        assert!(one.payloads(&parameters).is_empty());
        let a = one.payloads(&frame);
        let b = two.payloads(&frame);
        assert_eq!(a.len(), b.len() + 1);
        assert_eq!(a[0][0], 0x78);
        for (a, b) in a[1..].iter().zip(&b) {
            assert_eq!(a, b);
            assert_eq!(a.as_ptr(), b.as_ptr());
        }
        assert_eq!(one.payloads(&frame), b); // consumed SPS/PPS, no repeated STAP
    }

    #[test]
    fn annex_b_edge_cases_match_existing_payloader() {
        for bytes in [
            vec![],
            vec![0, 0, 0, 1],
            vec![0x65, 1, 2],
            vec![0, 0, 1, 0x65, 3, 0, 0, 0, 0, 1, 0x61, 4, 0, 0],
            vec![0, 0, 0, 1, 0, 0, 1, 0x65, 4],
        ] {
            let bytes = Bytes::from(bytes);
            let frame = PreparedFrame::new(bytes.clone(), 1100).unwrap();
            assert_eq!(
                Packetizer::default().payloads(&frame),
                H264Payloader::default().payload(1100, &bytes).unwrap()
            );
        }
    }

    #[test]
    #[ignore = "bounded release-mode payload microbenchmark, not a playback test"]
    fn benchmark_shared_packetization() {
        use std::{hint::black_box, time::Instant};
        const FRAMES: usize = 2000;
        const VIEWERS: usize = 16;
        let bytes = annex(&[vec![0x67, 1, 2], vec![0x68, 3, 4], vec![0x65; 32768]]);
        for round in 0..6 {
            // Alternate order; both paths use already normalized identical bytes.
            let mut results = [0u128; 2];
            let mut totals = [0usize; 2];
            for shared in if round % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            } {
                let mut original: Vec<_> = (0..VIEWERS).map(|_| H264Payloader::default()).collect();
                let mut consumers: Vec<_> = (0..VIEWERS).map(|_| Packetizer::default()).collect();
                let started = Instant::now();
                let mut total = 0;
                for _ in 0..FRAMES {
                    if shared {
                        let frame = PreparedFrame::new(black_box(bytes.clone()), 1100).unwrap();
                        for consumer in &mut consumers {
                            total += black_box(consumer.payloads(&frame)).len();
                        }
                    } else {
                        for consumer in &mut original {
                            total +=
                                black_box(consumer.payload(1100, black_box(&bytes)).unwrap()).len();
                        }
                    }
                }
                results[usize::from(shared)] = started.elapsed().as_micros();
                totals[usize::from(shared)] = total;
            }
            assert_eq!(totals[0], totals[1]);
            println!("{{\"round\":{round},\"frames\":{FRAMES},\"viewers\":{VIEWERS},\"before_us\":{},\"after_us\":{},\"packets\":{}}}", results[0], results[1], totals[0]);
        }
    }
}
