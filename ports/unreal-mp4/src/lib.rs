//! H.264 with optional AAC fragmented MP4. Original native mux data flow and deliberate
//! subset differences are documented in README.md. No codec or external process.
use anyhow::{bail, ensure, Context, Result};

const TRACK_ID: u32 = 1;
const TIMESCALE: u32 = 90_000;
const MAX_FRAME: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct AudioConfig {
    pub sample_rate: u32,
    pub channels: u16,
    pub decoder_config: Vec<u8>,
}
impl AudioConfig {
    fn validate(&self) -> Result<()> {
        ensure!(
            (2..=64).contains(&self.decoder_config.len()),
            "invalid AAC AudioSpecificConfig"
        );
        let asc = u16::from_be_bytes(self.decoder_config[..2].try_into().unwrap());
        let rates = [
            96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
        ];
        let index = ((asc >> 7) & 15) as usize;
        ensure!(
            asc >> 11 == 2 && rates.get(index) == Some(&self.sample_rate),
            "AAC-LC sample rate mismatch"
        );
        ensure!(
            (1..=2).contains(&self.channels) && ((asc >> 3) & 15) == self.channels,
            "unsupported AAC channels"
        );
        ensure!(
            self.sample_rate <= 65535 && asc & 7 == 0,
            "unsupported AAC frame configuration"
        );
        Ok(())
    }
}

pub struct Muxer {
    init: Vec<u8>,
    sps: Vec<u8>,
    pps: Vec<u8>,
    sequence: u32,
    last_dts: Option<u64>,
    audio: Option<AudioConfig>,
    last_audio_dts: Option<u64>,
}

impl Muxer {
    /// SPS/PPS include their NAL header, without Annex B or AVCC length prefixes.
    pub fn new(sps: &[u8], pps: &[u8], width: u32, height: u32) -> Result<Self> {
        Self::new_av(sps, pps, width, height, None)
    }

    pub fn new_av(
        sps: &[u8],
        pps: &[u8],
        width: u32,
        height: u32,
        audio: Option<AudioConfig>,
    ) -> Result<Self> {
        ensure!(sps.len() >= 4 && sps[0] & 0x9f == 7, "invalid SPS");
        ensure!(
            [66, 77, 88, 100, 110, 122, 144].contains(&sps[1]),
            "unsupported H264 profile"
        );
        ensure!(!pps.is_empty() && pps[0] & 0x9f == 8, "invalid PPS");
        ensure!(
            sps.len() <= 65535 && pps.len() <= 65535,
            "parameter set too large"
        );
        ensure!(
            (1..=65535).contains(&width) && (1..=65535).contains(&height),
            "invalid dimensions"
        );
        if let Some(a) = &audio {
            a.validate()?;
        }
        let init = initialization(sps, pps, width, height, audio.as_ref())?;
        Ok(Self {
            init,
            sps: sps.to_vec(),
            pps: pps.to_vec(),
            sequence: 1,
            last_dts: None,
            audio,
            last_audio_dts: None,
        })
    }

    pub fn init_segment(&self) -> &[u8] {
        &self.init
    }

    /// RFC6381 codec derived from the actual input SPS (not a fixed baseline claim).
    pub fn codec(&self) -> String {
        let video = format!(
            "avc1.{:02X}{:02X}{:02X}",
            self.sps[1], self.sps[2], self.sps[3]
        );
        if self.audio.is_some() {
            format!("{video},mp4a.40.2")
        } else {
            video
        }
    }

    /// `timestamp_90k` is caller-rebased DTS; this method does not rebase it.
    /// PTS=DTS: B-slices are explicitly unsupported and rejected.
    /// Each returned buffer is one complete moof+mdat; append init+buffers to
    /// record a fragmented MP4. First sample must be IDR.
    pub fn fragment(
        &mut self,
        avcc: &[u8],
        timestamp_90k: u64,
        duration_90k: u32,
        keyframe: bool,
    ) -> Result<Vec<u8>> {
        self.fragment_with_prefix(avcc, timestamp_90k, duration_90k, keyframe, 0)
    }

    /// Like `fragment`, but reserves a zero-filled transport prefix of at most
    /// 64 bytes in the same allocation. The caller may fill that prefix without
    /// copying the fragment. MP4 offsets remain relative to the following moof;
    /// omit the prefix when writing these bytes to an MP4 recording.
    pub fn fragment_with_prefix(
        &mut self,
        avcc: &[u8],
        timestamp_90k: u64,
        duration_90k: u32,
        keyframe: bool,
        prefix_len: usize,
    ) -> Result<Vec<u8>> {
        ensure!(prefix_len <= 64, "transport prefix exceeds 64 bytes");
        ensure!(
            !avcc.is_empty() && avcc.len() <= MAX_FRAME,
            "invalid access unit size"
        );
        ensure!(duration_90k > 0, "sample duration must be positive");
        ensure!(
            timestamp_90k.checked_add(duration_90k as u64).is_some(),
            "timestamp overflow"
        );
        if let Some(previous) = self.last_dts {
            ensure!(timestamp_90k > previous, "DTS must increase");
        }
        ensure!(self.sequence < u32::MAX, "fragment sequence exhausted");
        let mut cursor = 0;
        let mut idr = false;
        let mut vcl = false;
        while cursor < avcc.len() {
            let length_bytes = avcc
                .get(cursor..cursor + 4)
                .context("truncated AVCC length")?;
            let length = u32::from_be_bytes(length_bytes.try_into().unwrap()) as usize;
            cursor += 4;
            ensure!(
                length > 0 && length <= avcc.len() - cursor,
                "invalid AVCC NAL length"
            );
            let nal = &avcc[cursor..cursor + length];
            ensure!(nal[0] & 0x80 == 0, "forbidden NAL bit");
            match nal[0] & 31 {
                1 | 5 => {
                    vcl = true;
                    idr |= nal[0] & 31 == 5;
                    // Read just the slice prefix; do not unescape/copy a potentially
                    // multi-megabyte NAL to inspect two Exp-Golomb fields.
                    let mut bits = Bits::new(&nal[1..]);
                    bits.ue().context("truncated first_mb_in_slice")?;
                    let slice = bits.ue().context("truncated slice_type")?;
                    ensure!(
                        slice <= 9 && slice % 5 != 1,
                        "B-slice or unsupported slice type; DTS-only API"
                    );
                }
                7 => ensure!(
                    nal == self.sps,
                    "SPS changed; start a new muxer/init segment"
                ),
                8 => ensure!(
                    nal == self.pps,
                    "PPS changed; start a new muxer/init segment"
                ),
                6 | 9 | 10 | 11 | 12 => {}
                other => bail!("unsupported H264 NAL type {other}"),
            }
            cursor += length;
        }
        ensure!(vcl, "access unit contains no video slice");
        ensure!(keyframe == idr, "keyframe flag must match IDR presence");
        ensure!(self.last_dts.is_some() || idr, "first fragment must be IDR");

        // Rust adaptation of the original MSE adapter's size-first output
        // strategy (0x14009abf0), not an identical native MP4 serializer.
        // This API emits a fixed single-sample box layout: moof=100, mdat header=8.
        // Ownership is transferred to the caller, so buffers are never reused
        // while an asynchronous sender or recorder may still hold them.
        const MOOF_SIZE: u32 = 100;
        let mut output = Vec::with_capacity(prefix_len + MOOF_SIZE as usize + 8 + avcc.len());
        output.resize(prefix_len, 0);
        box_header(&mut output, *b"moof", MOOF_SIZE);
        full_header(&mut output, *b"mfhd", 16, 0, 0);
        put32(&mut output, self.sequence);
        box_header(&mut output, *b"traf", 76);
        full_header(&mut output, *b"tfhd", 16, 0, 0x020000);
        put32(&mut output, TRACK_ID);
        full_header(&mut output, *b"tfdt", 20, 1, 0);
        output.extend_from_slice(&timestamp_90k.to_be_bytes());
        full_header(&mut output, *b"trun", 32, 0, 0x000701);
        put32(&mut output, 1); // one sample
        put32(&mut output, MOOF_SIZE + 8); // signed data offset, relative to moof
        put32(&mut output, duration_90k);
        put32(&mut output, avcc.len() as u32);
        put32(&mut output, if keyframe { 0x02000000 } else { 0x01010000 });
        debug_assert_eq!(output.len(), prefix_len + MOOF_SIZE as usize);
        box_header(&mut output, *b"mdat", avcc.len() as u32 + 8);
        output.extend_from_slice(avcc);
        self.last_dts = Some(timestamp_90k);
        self.sequence += 1;
        Ok(output)
    }

    pub fn audio_fragment(
        &mut self,
        data: &[u8],
        timestamp: u64,
        duration: u32,
    ) -> Result<Vec<u8>> {
        self.audio_fragment_with_prefix(data, timestamp, duration, 0)
    }

    /// Raw AAC access unit (without ADTS), timestamp/duration in audio samples.
    pub fn audio_fragment_with_prefix(
        &mut self,
        data: &[u8],
        timestamp: u64,
        duration: u32,
        prefix_len: usize,
    ) -> Result<Vec<u8>> {
        ensure!(self.audio.is_some(), "AAC track not configured");
        ensure!(self.last_dts.is_some(), "video IDR must start the mux");
        ensure!(
            !data.is_empty() && data.len() <= 1024 * 1024 && prefix_len <= 64,
            "invalid AAC sample or prefix"
        );
        ensure!(
            duration == 1024 && timestamp.checked_add(u64::from(duration)).is_some(),
            "invalid AAC-LC sample duration"
        );
        ensure!(
            self.last_audio_dts.is_none_or(|last| timestamp > last),
            "audio DTS must increase"
        );
        ensure!(self.sequence < u32::MAX, "fragment sequence exhausted");
        let mut out = Vec::with_capacity(prefix_len + 108 + data.len());
        out.resize(prefix_len, 0);
        box_header(&mut out, *b"moof", 100);
        full_header(&mut out, *b"mfhd", 16, 0, 0);
        put32(&mut out, self.sequence);
        box_header(&mut out, *b"traf", 76);
        full_header(&mut out, *b"tfhd", 16, 0, 0x020000);
        put32(&mut out, 2);
        full_header(&mut out, *b"tfdt", 20, 1, 0);
        out.extend_from_slice(&timestamp.to_be_bytes());
        full_header(&mut out, *b"trun", 32, 0, 0x701);
        for value in [1, 108, duration, data.len() as u32, 0x02000000] {
            put32(&mut out, value);
        }
        box_header(&mut out, *b"mdat", data.len() as u32 + 8);
        out.extend_from_slice(data);
        self.sequence += 1;
        self.last_audio_dts = Some(timestamp);
        Ok(out)
    }
}

fn put16(out: &mut Vec<u8>, n: u16) {
    out.extend_from_slice(&n.to_be_bytes());
}
fn put32(out: &mut Vec<u8>, n: u32) {
    out.extend_from_slice(&n.to_be_bytes());
}
fn box_header(out: &mut Vec<u8>, kind: [u8; 4], size: u32) {
    put32(out, size);
    out.extend_from_slice(&kind);
}
fn full_header(out: &mut Vec<u8>, kind: [u8; 4], size: u32, version: u8, flags: u32) {
    box_header(out, kind, size);
    put32(out, ((version as u32) << 24) | flags);
}
fn boxed(kind: [u8; 4], data: Vec<u8>) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 8);
    put32(&mut out, (data.len() + 8) as u32);
    out.extend_from_slice(&kind);
    out.extend_from_slice(&data);
    out
}
fn full(kind: [u8; 4], version: u8, flags: u32, data: Vec<u8>) -> Vec<u8> {
    let mut body = vec![
        version,
        (flags >> 16) as u8,
        (flags >> 8) as u8,
        flags as u8,
    ];
    body.extend_from_slice(&data);
    boxed(kind, body)
}
fn matrix(out: &mut Vec<u8>) {
    for value in [0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x40000000] {
        put32(out, value);
    }
}

fn initialization(
    sps: &[u8],
    pps: &[u8],
    width: u32,
    height: u32,
    audio: Option<&AudioConfig>,
) -> Result<Vec<u8>> {
    let mut brands = b"isom".to_vec();
    put32(&mut brands, 0x200);
    brands.extend_from_slice(b"isomiso2iso5avc1mp41");
    let ftyp = boxed(*b"ftyp", brands);

    let mut mvhd = vec![0; 8];
    put32(&mut mvhd, 1000);
    put32(&mut mvhd, 0);
    put32(&mut mvhd, 0x10000);
    put16(&mut mvhd, 0x100);
    mvhd.extend_from_slice(&[0; 10]);
    matrix(&mut mvhd);
    mvhd.extend_from_slice(&[0; 24]);
    put32(&mut mvhd, if audio.is_some() { 3 } else { 2 });
    let mvhd = full(*b"mvhd", 0, 0, mvhd);

    let mut tkhd = vec![0; 8];
    put32(&mut tkhd, TRACK_ID);
    put32(&mut tkhd, 0);
    put32(&mut tkhd, 0);
    tkhd.extend_from_slice(&[0; 16]);
    matrix(&mut tkhd);
    put32(&mut tkhd, width << 16);
    put32(&mut tkhd, height << 16);
    let tkhd = full(*b"tkhd", 0, 7, tkhd);

    let mut mdhd = vec![0; 8];
    put32(&mut mdhd, TIMESCALE);
    put32(&mut mdhd, 0);
    put16(&mut mdhd, 0x55c4);
    put16(&mut mdhd, 0);
    let mdhd = full(*b"mdhd", 0, 0, mdhd);
    let mut hdlr = vec![0; 4];
    hdlr.extend_from_slice(b"vide");
    hdlr.extend_from_slice(&[0; 12]);
    hdlr.extend_from_slice(b"VideoHandler\0");
    let hdlr = full(*b"hdlr", 0, 0, hdlr);

    let mut avcc = vec![1, sps[1], sps[2], sps[3], 0xff, 0xe1];
    put16(&mut avcc, sps.len() as u16);
    avcc.extend_from_slice(sps);
    avcc.push(1);
    put16(&mut avcc, pps.len() as u16);
    avcc.extend_from_slice(pps);
    if [100, 110, 122, 144].contains(&sps[1]) {
        let mut bits = Bits::new(&sps[1..]);
        bits.read(24)?;
        bits.ue()?; // profile/compat/level, seq_parameter_set_id
        let chroma = bits.ue()?;
        ensure!(chroma <= 3, "invalid chroma format");
        if chroma == 3 {
            bits.read(1)?;
        }
        let luma_depth = bits.ue()?;
        let chroma_depth = bits.ue()?;
        ensure!(
            luma_depth <= 6 && chroma_depth <= 6,
            "unsupported bit depth"
        );
        avcc.extend_from_slice(&[
            0xfc | chroma as u8,
            0xf8 | luma_depth as u8,
            0xf8 | chroma_depth as u8,
            0,
        ]);
    }
    let mut avc1 = vec![0; 6];
    put16(&mut avc1, 1);
    avc1.extend_from_slice(&[0; 16]);
    put16(&mut avc1, width as u16);
    put16(&mut avc1, height as u16);
    put32(&mut avc1, 0x480000);
    put32(&mut avc1, 0x480000);
    put32(&mut avc1, 0);
    put16(&mut avc1, 1);
    avc1.extend_from_slice(&[0; 32]);
    put16(&mut avc1, 0x18);
    put16(&mut avc1, 0xffff);
    avc1.extend_from_slice(&boxed(*b"avcC", avcc));
    let mut stsd = 1u32.to_be_bytes().to_vec();
    stsd.extend_from_slice(&boxed(*b"avc1", avc1));
    let stbl = boxed(
        *b"stbl",
        [
            full(*b"stsd", 0, 0, stsd),
            full(*b"stts", 0, 0, vec![0; 4]),
            full(*b"stsc", 0, 0, vec![0; 4]),
            full(*b"stsz", 0, 0, vec![0; 8]),
            full(*b"stco", 0, 0, vec![0; 4]),
        ]
        .concat(),
    );
    let mut dref = 1u32.to_be_bytes().to_vec();
    dref.extend_from_slice(&full(*b"url ", 0, 1, vec![]));
    let dinf = boxed(*b"dinf", full(*b"dref", 0, 0, dref));
    let minf = boxed(
        *b"minf",
        [full(*b"vmhd", 0, 1, vec![0; 8]), dinf, stbl].concat(),
    );
    let mdia = boxed(*b"mdia", [mdhd, hdlr, minf].concat());
    let trak = boxed(*b"trak", [tkhd, mdia].concat());
    let mut trex = Vec::new();
    for value in [TRACK_ID, 1, 0, 0, 0x01010000] {
        put32(&mut trex, value);
    }
    let mut trexes = full(*b"trex", 0, 0, trex);
    let mut tracks = trak;
    if let Some(a) = audio {
        tracks.extend_from_slice(&audio_track(a));
        let mut trex = Vec::new();
        for value in [2, 1, 1024, 0, 0x02000000] {
            put32(&mut trex, value);
        }
        trexes.extend_from_slice(&full(*b"trex", 0, 0, trex));
    }
    let mvex = boxed(*b"mvex", trexes);
    Ok([ftyp, boxed(*b"moov", [mvhd, tracks, mvex].concat())].concat())
}

fn descriptor(tag: u8, data: Vec<u8>) -> Vec<u8> {
    // All generated descriptors fit in the one-byte ISO descriptor size form.
    debug_assert!(data.len() < 128);
    [vec![tag, data.len() as u8], data].concat()
}
fn audio_track(a: &AudioConfig) -> Vec<u8> {
    let mut tkhd = vec![0; 8];
    for value in [2, 0, 0] {
        put32(&mut tkhd, value);
    }
    tkhd.extend_from_slice(&[0; 12]);
    put16(&mut tkhd, 0x100);
    put16(&mut tkhd, 0);
    matrix(&mut tkhd);
    tkhd.extend_from_slice(&[0; 8]);
    let mut mdhd = vec![0; 8];
    put32(&mut mdhd, a.sample_rate);
    put32(&mut mdhd, 0);
    put16(&mut mdhd, 0x55c4);
    put16(&mut mdhd, 0);
    let mut handler = vec![0; 4];
    handler.extend_from_slice(b"soun");
    handler.extend_from_slice(&[0; 12]);
    handler.extend_from_slice(b"SoundHandler\0");
    let mut decoder = vec![0x40, 0x15, 0, 0, 0];
    decoder.extend_from_slice(&[0; 8]);
    decoder.extend_from_slice(&descriptor(5, a.decoder_config.clone()));
    let mut es = vec![0, 2, 0];
    es.extend_from_slice(&descriptor(4, decoder));
    es.extend_from_slice(&descriptor(6, vec![2]));
    let mut entry = vec![0; 6];
    put16(&mut entry, 1);
    entry.extend_from_slice(&[0; 8]);
    put16(&mut entry, a.channels);
    put16(&mut entry, 16);
    entry.extend_from_slice(&[0; 4]);
    put32(&mut entry, a.sample_rate << 16);
    entry.extend_from_slice(&full(*b"esds", 0, 0, descriptor(3, es)));
    let stsd = [1u32.to_be_bytes().to_vec(), boxed(*b"mp4a", entry)].concat();
    let stbl = boxed(
        *b"stbl",
        [
            full(*b"stsd", 0, 0, stsd),
            full(*b"stts", 0, 0, vec![0; 4]),
            full(*b"stsc", 0, 0, vec![0; 4]),
            full(*b"stsz", 0, 0, vec![0; 8]),
            full(*b"stco", 0, 0, vec![0; 4]),
        ]
        .concat(),
    );
    let dref = [1u32.to_be_bytes().to_vec(), full(*b"url ", 0, 1, vec![])].concat();
    let minf = boxed(
        *b"minf",
        [
            full(*b"smhd", 0, 0, vec![0; 4]),
            boxed(*b"dinf", full(*b"dref", 0, 0, dref)),
            stbl,
        ]
        .concat(),
    );
    let mdia = boxed(
        *b"mdia",
        [
            full(*b"mdhd", 0, 0, mdhd),
            full(*b"hdlr", 0, 0, handler),
            minf,
        ]
        .concat(),
    );
    boxed(*b"trak", [full(*b"tkhd", 0, 7, tkhd), mdia].concat())
}

/// Lazy EBSP reader with the same emulation-prevention removal as the former
/// full-NAL RBSP copy. At most 31 leading zeros are accepted per Exp-Golomb
/// field; parsing two slice fields therefore consumes only a bounded prefix.
struct Bits<'a> {
    bytes: &'a [u8],
    cursor: usize,
    current: u8,
    remaining: u8,
    zeros: u8,
}
impl<'a> Bits<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            cursor: 0,
            current: 0,
            remaining: 0,
            zeros: 0,
        }
    }
    fn read(&mut self, count: usize) -> Result<u32> {
        ensure!(count <= 32, "invalid H264 bit count");
        let mut value = 0;
        for _ in 0..count {
            if self.remaining == 0 {
                loop {
                    let byte = *self.bytes.get(self.cursor).context("truncated H264 bits")?;
                    self.cursor += 1;
                    if self.zeros >= 2 && byte == 3 {
                        self.zeros = 0;
                        continue;
                    }
                    self.zeros = if byte == 0 {
                        (self.zeros + 1).min(2)
                    } else {
                        0
                    };
                    self.current = byte;
                    self.remaining = 8;
                    break;
                }
            }
            self.remaining -= 1;
            value = (value << 1) | ((self.current >> self.remaining) & 1) as u32;
        }
        Ok(value)
    }
    fn ue(&mut self) -> Result<u32> {
        let mut zeros = 0;
        while self.read(1)? == 0 {
            zeros += 1;
            ensure!(zeros < 32, "exp-Golomb overflow");
        }
        Ok((1u32 << zeros) - 1 + self.read(zeros)?)
    }
}

#[cfg(test)]
mod optimization_tests {
    use super::*;

    fn unhex(value: &str) -> Vec<u8> {
        value
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    #[test]
    fn fragments_match_pre_optimization_layout() {
        // Frozen single-sample layout from the former nested-box writer. These
        // fixtures cover version-1 tfdt above 32 bits, sequence, offset and both
        // sample flags. They are Rust compatibility vectors, not native output.
        let mut mux = Muxer::new(&[0x67, 0x42, 0xe0, 0x1e], &[0x68, 0xce], 640, 360).unwrap();
        let expected_idr = unhex(concat!(
            "000000646d6f6f66",
            "000000106d6668640000000000000001",
            "0000004c74726166",
            "00000010746668640002000000000001",
            "0000001474666474010000000000000100000005",
            "000000207472756e00000701000000010000006c000017700000000602000000",
            "0000000e6d6461740000000265b0"
        ));
        let expected_p = unhex(concat!(
            "000000646d6f6f66",
            "000000106d6668640000000000000002",
            "0000004c74726166",
            "00000010746668640002000000000001",
            "0000001474666474010000000000000100001775",
            "000000207472756e00000701000000010000006c000017700000000601010000",
            "0000000e6d6461740000000261c0"
        ));
        assert_eq!(
            mux.fragment(&[0, 0, 0, 2, 0x65, 0xb0], 0x100000005, 6000, true)
                .unwrap(),
            expected_idr
        );
        assert_eq!(
            mux.fragment(&[0, 0, 0, 2, 0x61, 0xc0], 0x100001775, 6000, false)
                .unwrap(),
            expected_p
        );
    }

    #[test]
    fn transport_prefix_is_zeroed_and_preserves_fragment_offsets_and_state() {
        let create = || Muxer::new(&[0x67, 0x42, 0xe0, 0x1e], &[0x68, 0xce], 640, 360).unwrap();
        let avcc = [0, 0, 0, 2, 0x65, 0xb0];
        let expected = create().fragment(&avcc, 0x100000005, 6000, true).unwrap();
        for prefix_len in [0, 10, 64] {
            let mut mux = create();
            // Rejected prefix lengths must not advance DTS or sequence.
            assert!(mux
                .fragment_with_prefix(&avcc, 0x100000005, 6000, true, 65)
                .is_err());
            assert!(mux
                .fragment_with_prefix(&avcc, 0x100000005, 6000, true, usize::MAX)
                .is_err());
            let prefixed = mux
                .fragment_with_prefix(&avcc, 0x100000005, 6000, true, prefix_len)
                .unwrap();
            assert_eq!(prefixed.len(), prefix_len + expected.len());
            assert!(prefixed[..prefix_len].iter().all(|&byte| byte == 0));
            assert_eq!(&prefixed[prefix_len..], expected.as_slice());
            let offset = u32::from_be_bytes(
                prefixed[prefix_len + 84..prefix_len + 88]
                    .try_into()
                    .unwrap(),
            );
            assert_eq!(offset, 108);
            assert_eq!(&prefixed[prefix_len + offset as usize..], &avcc);
        }
    }

    #[test]
    fn lazy_slice_reader_handles_escapes_and_truncation() {
        // first_mb_in_slice=2^23-1, slice_type=2, with two escape bytes.
        let escaped = [0, 0, 3, 1, 0, 0, 3, 0, 0xc0];
        let mut bits = Bits::new(&escaped);
        assert_eq!(bits.ue().unwrap(), (1 << 23) - 1);
        assert_eq!(bits.ue().unwrap(), 2);
        for truncated in [&[][..], &[0][..], &[0, 0, 3][..], &[0, 0, 3, 0x80][..]] {
            assert!(Bits::new(truncated).ue().is_err());
        }
        assert!(Bits::new(&[0; 8]).ue().is_err()); // bounded overlong code
        let mut escaped_byte = Bits::new(&[0, 0, 3, 3]);
        assert_eq!(escaped_byte.read(24).unwrap(), 3); // only the first 03 is removed
        assert!(escaped_byte.read(1).is_err());
    }

    #[test]
    fn slice_validation_reads_only_the_prefix_of_a_large_nal() {
        let mut encoded = vec![0xff; 1024 * 1024];
        encoded[0] = 0xb0; // first_mb=0, slice_type=2
        let mut bits = Bits::new(&encoded);
        assert_eq!(bits.ue().unwrap(), 0);
        assert_eq!(bits.ue().unwrap(), 2);
        assert_eq!(bits.cursor, 1);
    }
}
