//! RFC 3551 G.711, RFC 3640 AAC-hbr and RFC 7587 Opus input.
//! This standards-based extension is not a recovered native audio implementation.
use anyhow::{ensure, Context, Result};
use url::Url;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioCodec {
    Aac,
    Pcmu,
    Pcma,
    Opus,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioConfig {
    pub codec: AudioCodec,
    pub clock_rate: u32,
    pub channels: u16,
    /// MPEG-4 AudioSpecificConfig for AAC; empty for RTP-native codecs.
    pub config: Vec<u8>,
}
#[derive(Clone, Debug)]
pub struct AudioFrame {
    pub data: bytes::Bytes,
    /// Extended sample clock, starting at zero for this audio track.
    pub timestamp: u64,
    pub duration: u32,
    /// Common presentation timeline, using first RTP arrival as the A/V anchor.
    /// Cameras without sender reports have network-arrival synchronization accuracy.
    pub timestamp_90k: u64,
}
pub(crate) struct Track {
    pub pt: u8,
    pub control: String,
    pub config: AudioConfig,
}

pub(crate) fn parse_sdp(sdp: &str, base: &Url) -> Result<Option<Track>> {
    for section in sdp.split("m=").skip(1) {
        let lines: Vec<_> = section.lines().map(str::trim).collect();
        let Some(first) = lines.first() else { continue };
        if !first.starts_with("audio ") {
            continue;
        }
        let offered: Vec<u8> = first
            .split_whitespace()
            .skip(3)
            .filter_map(|s| s.parse().ok())
            .collect();
        for pt in offered.into_iter().filter(|p| *p < 128) {
            let prefix = format!("a=rtpmap:{pt} ");
            let map = lines
                .iter()
                .find_map(|l| l.strip_prefix(&prefix))
                .unwrap_or(match pt {
                    0 => "PCMU/8000",
                    8 => "PCMA/8000",
                    _ => "",
                });
            let parts: Vec<_> = map.split('/').collect();
            if parts.len() < 2 {
                continue;
            }
            let codec = match parts[0].to_ascii_uppercase().as_str() {
                "MPEG4-GENERIC" => AudioCodec::Aac,
                "PCMU" => AudioCodec::Pcmu,
                "PCMA" => AudioCodec::Pcma,
                "OPUS" => AudioCodec::Opus,
                _ => continue,
            };
            let clock_rate = parts[1].parse::<u32>()?;
            let channels = parts.get(2).unwrap_or(&"1").parse::<u16>()?;
            if !(1..=2).contains(&channels) || !(8000..=96000).contains(&clock_rate) {
                continue;
            }
            if matches!(codec, AudioCodec::Pcmu | AudioCodec::Pcma)
                && (clock_rate != 8000 || channels != 1)
            {
                continue;
            }
            if codec == AudioCodec::Opus && (clock_rate != 48000 || channels != 2) {
                continue;
            }
            let mut config = Vec::new();
            if codec == AudioCodec::Aac {
                let prefix = format!("a=fmtp:{pt} ");
                let Some(fmtp) = lines.iter().find_map(|l| l.strip_prefix(&prefix)) else {
                    continue;
                };
                let fields: std::collections::HashMap<_, _> = fmtp
                    .split(';')
                    .filter_map(|f| f.trim().split_once('='))
                    .map(|(k, v)| (k.to_ascii_lowercase(), v.trim()))
                    .collect();
                if !fields
                    .get("mode")
                    .is_some_and(|v| v.eq_ignore_ascii_case("AAC-hbr"))
                    || fields.get("sizelength") != Some(&"13")
                    || fields.get("indexlength") != Some(&"3")
                    || fields.get("indexdeltalength") != Some(&"3")
                {
                    continue;
                }
                let Some(hex) = fields.get("config") else {
                    continue;
                };
                if hex.len() < 4 || hex.len() > 32 || hex.len() % 2 != 0 {
                    continue;
                }
                let decoded = hex
                    .as_bytes()
                    .chunks_exact(2)
                    .map(|p| {
                        std::str::from_utf8(p)
                            .ok()
                            .and_then(|s| u8::from_str_radix(s, 16).ok())
                    })
                    .collect::<Option<Vec<_>>>();
                let Some(decoded) = decoded else { continue };
                // AAC-LC only. HE-AAC/SBR/PS and 960-sample frames need different timing.
                let object = decoded[0] >> 3;
                let rate_index = ((decoded[0] & 7) << 1) | (decoded[1] >> 7);
                let rates = [
                    96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025,
                    8000, 7350,
                ];
                let asc_channels = (decoded[1] >> 3) & 15;
                if object != 2
                    || rates.get(rate_index as usize) != Some(&clock_rate)
                    || asc_channels as u16 != channels
                    || decoded[1] & 7 != 0
                {
                    continue;
                }
                config = decoded;
            }
            let control = lines
                .iter()
                .find_map(|l| l.strip_prefix("a=control:"))
                .context("Audio track lacks control URI")?;
            return Ok(Some(Track {
                pt,
                control: super::resolve_control(base, control)?,
                config: AudioConfig {
                    codec,
                    clock_rate,
                    channels,
                    config,
                },
            }));
        }
    }
    Ok(None)
}

pub(crate) struct Assembler {
    pt: u8,
    pub config: AudioConfig,
    last_seq: Option<u16>,
    ssrc: Option<u32>,
    last_ts: Option<u32>,
    clock: u64,
    anchor: Option<u64>,
    fragment: Option<(u32, usize, Vec<u8>)>,
}
impl Assembler {
    pub fn presentation_clock(&self) -> u64 {
        self.anchor.unwrap_or(0) + self.clock * 90000 / u64::from(self.config.clock_rate)
    }
    pub fn new(track: Track) -> Self {
        Self {
            pt: track.pt,
            config: track.config,
            last_seq: None,
            ssrc: None,
            last_ts: None,
            clock: 0,
            anchor: None,
            fragment: None,
        }
    }
    pub fn push(&mut self, bytes: &[u8], arrival_90k: u64) -> Result<Vec<AudioFrame>> {
        let p = match super::rtp::packet(bytes) {
            Ok(p) => p,
            Err(_) => {
                self.fragment = None;
                return Ok(vec![]);
            }
        };
        if p.pt != self.pt {
            return Ok(vec![]);
        }
        if self.ssrc.is_some_and(|s| s != p.ssrc) {
            self.last_seq = None;
            self.last_ts = None;
            self.anchor = None;
            self.clock = 0;
            self.fragment = None;
        }
        self.ssrc = Some(p.ssrc);
        if let Some(seq) = self.last_seq {
            let delta = p.seq.wrapping_sub(seq);
            if delta == 0 || delta > 0x8000 {
                return Ok(vec![]);
            }
            if delta != 1 {
                self.fragment = None;
            }
        }
        self.last_seq = Some(p.seq);
        if let Some(ts) = self.last_ts {
            let delta = p.timestamp.wrapping_sub(ts);
            if delta > self.config.clock_rate * 10 {
                self.anchor = Some(arrival_90k);
                self.clock = 0;
                self.fragment = None;
            } else {
                self.clock += u64::from(delta);
            }
        }
        self.last_ts = Some(p.timestamp);
        let anchor = *self.anchor.get_or_insert(arrival_90k);
        let mut units: Vec<(Vec<u8>, u32)> = Vec::new();
        match self.config.codec {
            AudioCodec::Pcmu | AudioCodec::Pcma => {
                ensure!(p.payload.len() <= 8000, "G711 packet too large");
                units.push((p.payload.to_vec(), p.payload.len() as u32));
            }
            AudioCodec::Opus => {
                let duration = opus_duration(p.payload)?;
                units.push((p.payload.to_vec(), duration));
            }
            AudioCodec::Aac => {
                ensure!(p.payload.len() >= 4, "Truncated AAC AU header");
                let bits = u16::from_be_bytes([p.payload[0], p.payload[1]]) as usize;
                ensure!(
                    bits >= 16 && bits % 16 == 0 && bits <= 1024 && 2 + bits / 8 <= p.payload.len(),
                    "Invalid AAC AU headers"
                );
                let mut at = 2 + bits / 8;
                for (index, h) in p.payload[2..at].chunks_exact(2).enumerate() {
                    let raw = u16::from_be_bytes([h[0], h[1]]);
                    let size = (raw >> 3) as usize;
                    ensure!(size > 0 && raw & 7 == 0, "Unsupported AAC AU index");
                    if index == 0 && bits == 16 && at + size > p.payload.len() {
                        let fragment = self.fragment.get_or_insert((
                            p.timestamp,
                            size,
                            Vec::with_capacity(size),
                        ));
                        if fragment.0 != p.timestamp || fragment.1 != size {
                            self.fragment = None;
                            return Ok(vec![]);
                        }
                        fragment.2.extend_from_slice(&p.payload[at..]);
                        if fragment.2.len() > size {
                            self.fragment = None;
                            return Ok(vec![]);
                        }
                        if fragment.2.len() == size {
                            units.push((self.fragment.take().unwrap().2, 1024));
                        }
                        at = p.payload.len();
                        break;
                    }
                    ensure!(at + size <= p.payload.len(), "Truncated AAC AU");
                    self.fragment = None;
                    units.push((p.payload[at..at + size].to_vec(), 1024));
                    at += size;
                }
                ensure!(at == p.payload.len(), "Unexpected AAC payload suffix");
            }
        }
        let mut timestamp = self.clock;
        Ok(units
            .into_iter()
            .map(|(data, duration)| {
                let frame = AudioFrame {
                    data: data.into(),
                    timestamp,
                    duration,
                    timestamp_90k: anchor + timestamp * 90000 / u64::from(self.config.clock_rate),
                };
                timestamp += u64::from(duration);
                frame
            })
            .collect())
    }
}
fn opus_duration(data: &[u8]) -> Result<u32> {
    ensure!(
        !data.is_empty() && data.len() <= 1275 * 6,
        "Invalid Opus payload"
    );
    let toc = data[0];
    let config = toc >> 3;
    let per = if config >= 16 {
        120u32 << (config & 3)
    } else if config >= 12 {
        480u32 << (config & 1)
    } else {
        [480, 960, 1920, 2880][(config & 3) as usize]
    };
    let count = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => {
            ensure!(data.len() > 1, "Truncated Opus count");
            u32::from(data[1] & 63)
        }
    };
    ensure!(count > 0 && per * count <= 5760, "Invalid Opus duration");
    Ok(per * count)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn packet(seq: u16, ts: u32, payload: &[u8]) -> Vec<u8> {
        let mut p = vec![0x80, 97];
        p.extend(seq.to_be_bytes());
        p.extend(ts.to_be_bytes());
        p.extend(1u32.to_be_bytes());
        p.extend(payload);
        p
    }
    fn aac() -> Assembler {
        Assembler::new(Track {
            pt: 97,
            control: String::new(),
            config: AudioConfig {
                codec: AudioCodec::Aac,
                clock_rate: 48000,
                channels: 2,
                config: vec![0x11, 0x90],
            },
        })
    }
    #[test]
    fn independent_epoch_wrap_and_arrival_anchor() {
        let mut a = aac();
        let p = [0, 16, 0, 16, 1, 2];
        let one = a.push(&packet(1, u32::MAX - 1023, &p), 4500).unwrap();
        let two = a.push(&packet(2, 0, &p), 7000).unwrap();
        assert_eq!(one[0].timestamp_90k, 4500);
        assert_eq!(two[0].timestamp, 1024);
        assert_eq!(two[0].timestamp_90k, 6420);
        assert!(a.push(&packet(2, 0, &p), 8000).unwrap().is_empty());
    }
    #[test]
    fn aac_multiple_and_fragment_loss() {
        let mut a = aac();
        let frames = a
            .push(&packet(1, 100, &[0, 32, 0, 16, 0, 8, 1, 2, 3]), 0)
            .unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[1].timestamp, 1024);
        assert!(a
            .push(&packet(2, 2148, &[0, 16, 0, 32, 1, 2]), 100)
            .unwrap()
            .is_empty());
        assert!(a
            .push(&packet(4, 2148, &[0, 16, 0, 32, 3, 4]), 100)
            .unwrap()
            .is_empty());
    }
    #[test]
    fn sdp_rejects_unsupported_aac_shape() {
        let base = Url::parse("rtsp://host/camera/").unwrap();
        let sdp="m=audio 0 RTP/AVP 97\na=rtpmap:97 MPEG4-GENERIC/48000/2\na=fmtp:97 mode=AAC-hbr;sizeLength=13;indexLength=3;indexDeltaLength=3;config=1190\na=control:track2\n";
        assert_eq!(
            parse_sdp(sdp, &base).unwrap().unwrap().config.codec,
            AudioCodec::Aac
        );
        assert!(parse_sdp(&sdp.replace("1190", "2990"), &base)
            .unwrap()
            .is_none());
    }
}
