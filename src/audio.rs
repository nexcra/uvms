//! Camera AAC-LC to WebRTC Opus, decoded once per shared camera feed.
//! Recording/HLS/RTSP retain the original encoded audio. No external encoder process.
use anyhow::{ensure, Result};
use symphonia::core::{
    audio::SampleBuffer,
    codecs::{CodecParameters, Decoder, DecoderOptions, CODEC_TYPE_AAC},
    formats::Packet,
};
use unreal_rtsp::{AudioCodec, AudioConfig, AudioFrame};

pub struct RtcAudio {
    config: AudioConfig,
    decoder: Option<Box<dyn Decoder>>,
    pending: Vec<f32>,
    opus_encoder: Option<opus::Encoder>,
    sum: [f64; 2],
    weight: u32,
    timestamp: Option<u64>,
    expected_input: Option<u64>,
    source_clock_offset: Option<i128>,
    clock_adjust: i64,
    failed: bool,
}
impl RtcAudio {
    pub fn new(config: &AudioConfig) -> Result<Self> {
        let decoder = if config.codec == AudioCodec::Aac {
            let mut params = CodecParameters::new();
            params
                .for_codec(CODEC_TYPE_AAC)
                .with_sample_rate(config.clock_rate)
                .with_extra_data(config.config.clone().into_boxed_slice());
            Some(symphonia::default::get_codecs().make(&params, &DecoderOptions::default())?)
        } else {
            None
        };
        let opus_encoder = if config.codec == AudioCodec::Aac {
            Some(opus::Encoder::new(
                48000,
                if config.channels == 1 {
                    opus::Channels::Mono
                } else {
                    opus::Channels::Stereo
                },
                opus::Application::Audio,
            )?)
        } else {
            None
        };
        Ok(Self {
            config: config.clone(),
            decoder,
            pending: Vec::with_capacity(1920),
            opus_encoder,
            sum: [0.0; 2],
            weight: 0,
            timestamp: None,
            expected_input: None,
            source_clock_offset: None,
            clock_adjust: 0,
            failed: false,
        })
    }
    pub fn push(&mut self, frame: &AudioFrame) -> Result<Vec<AudioFrame>> {
        let result = self.push_inner(frame);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn push_inner(&mut self, frame: &AudioFrame) -> Result<Vec<AudioFrame>> {
        let Some(decoder) = &mut self.decoder else {
            return Ok(vec![frame.clone()]);
        };
        if self.failed || self.expected_input.is_some_and(|t| t != frame.timestamp) {
            decoder.reset();
            self.pending.clear();
            self.sum = [0.0; 2];
            self.opus_encoder.as_mut().unwrap().reset_state()?;
            self.weight = 0;
            self.timestamp = None;
            self.source_clock_offset = None;
            self.clock_adjust = 0;
            self.failed = false;
        }
        self.expected_input = Some(frame.timestamp + u64::from(frame.duration));
        let source_offset = i128::from(frame.timestamp_90k)
            - i128::from(frame.timestamp) * 90000 / i128::from(self.config.clock_rate);
        let first_offset = *self.source_clock_offset.get_or_insert(source_offset);
        self.clock_adjust =
            (source_offset - first_offset).clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64;
        let packet =
            Packet::new_from_slice(0, frame.timestamp, u64::from(frame.duration), &frame.data);
        let decoded = decoder.decode(&packet)?;
        ensure!(
            decoded.spec().rate == self.config.clock_rate,
            "AAC sample rate changed"
        );
        let channels = decoded.spec().channels.count();
        ensure!((1..=2).contains(&channels), "Unsupported AAC channels");
        let mut samples = SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
        samples.copy_interleaved_ref(decoded);
        self.timestamp
            .get_or_insert(frame.timestamp_90k * 48000 / 90000);
        let mut output = Vec::new();
        // Rational sample-rate conversion into 48-kHz Opus. At 48 kHz this is
        // a direct copy; channel separation is preserved for stereo cameras.
        for chunk in samples.samples().chunks_exact(channels) {
            let mut left = 48000;
            while left > 0 {
                let take = left.min(self.config.clock_rate - self.weight);
                for (index, sample) in chunk.iter().enumerate() {
                    self.sum[index] += f64::from(*sample) * f64::from(take);
                }
                self.weight += take;
                left -= take;
                if self.weight == self.config.clock_rate {
                    for index in 0..channels {
                        self.pending
                            .push((self.sum[index] / f64::from(self.config.clock_rate)) as f32);
                    }
                    self.sum = [0.0; 2];
                    self.weight = 0;
                    if self.pending.len() == 960 * channels {
                        let mut data = vec![0u8; 4000];
                        let size = self
                            .opus_encoder
                            .as_mut()
                            .unwrap()
                            .encode_float(&self.pending, &mut data)?;
                        data.truncate(size);
                        self.pending.clear();
                        let timestamp = self.timestamp.unwrap();
                        self.timestamp = Some(timestamp + 960);
                        output.push(AudioFrame {
                            data: data.into(),
                            timestamp,
                            duration: 960,
                            timestamp_90k: (i128::from(timestamp) * 90000 / 48000
                                + i128::from(self.clock_adjust))
                            .clamp(0, i128::from(u64::MAX))
                                as u64,
                        });
                    }
                }
            }
        }
        Ok(output)
    }
}

pub fn rtc_config(config: &AudioConfig) -> AudioConfig {
    if config.codec == AudioCodec::Aac {
        AudioConfig {
            codec: AudioCodec::Opus,
            clock_rate: 48000,
            channels: 2,
            config: vec![],
        }
    } else {
        config.clone()
    }
}
#[cfg(test)]
fn linear_to_mulaw(sample: i16) -> u8 {
    let mut value = i32::from(sample);
    let sign = if value < 0 {
        value = -value;
        0x80
    } else {
        0
    };
    value = value.min(32635) + 132;
    let exponent = (7 - (value.leading_zeros() as i32 - 17)).clamp(0, 7) as u8;
    let mantissa = ((value >> (exponent + 3)) & 15) as u8;
    !(sign | (exponent << 4) | mantissa)
}

/// Shared AAC-LC archive conversion. AAC cameras are bit-exact passthrough;
/// G.711 and Opus cameras use the standalone FDK encoder, never FFmpeg.
pub struct ArchiveAudio {
    input: AudioConfig,
    output: AudioConfig,
    encoder: Option<fdk_aac::enc::Encoder>,
    opus: Option<opus::Decoder>,
    pending: Vec<i16>,
    expected: Option<u64>,
    origin: Option<u64>,
    emitted: u64,
    delay: u32,
    input_start: Option<u64>,
    failed: bool,
}
fn aac_encoder(config: &AudioConfig) -> Result<fdk_aac::enc::Encoder> {
    use fdk_aac::enc::*;
    Encoder::new(EncoderParams {
        bit_rate: BitRate::Cbr(if config.clock_rate == 8000 {
            24000
        } else {
            96000
        }),
        sample_rate: config.clock_rate,
        transport: Transport::Raw,
        channels: if config.channels == 1 {
            ChannelMode::Mono
        } else {
            ChannelMode::Stereo
        },
        audio_object_type: AudioObjectType::Mpeg4LowComplexity,
    })
    .map_err(|e| anyhow::anyhow!("AAC encoder initialization: {e}"))
}
impl ArchiveAudio {
    pub fn new(input: &AudioConfig) -> Result<Self> {
        let encoder = if input.codec == AudioCodec::Aac {
            None
        } else {
            Some(aac_encoder(input)?)
        };
        let (output, delay) = if let Some(encoder) = &encoder {
            let info = encoder
                .info()
                .map_err(|e| anyhow::anyhow!("AAC encoder info: {e}"))?;
            ensure!(
                info.frameLength == 1024 && info.confSize <= 64,
                "Unsupported AAC encoder configuration"
            );
            (
                AudioConfig {
                    codec: AudioCodec::Aac,
                    clock_rate: input.clock_rate,
                    channels: input.channels,
                    config: info.confBuf[..info.confSize as usize].to_vec(),
                },
                info.nDelay,
            )
        } else {
            (input.clone(), 0)
        };
        let opus = if input.codec == AudioCodec::Opus {
            Some(opus::Decoder::new(
                input.clock_rate,
                opus::Channels::Stereo,
            )?)
        } else {
            None
        };
        Ok(Self {
            input: input.clone(),
            output,
            encoder,
            opus,
            pending: Vec::new(),
            expected: None,
            origin: None,
            emitted: 0,
            delay,
            input_start: None,
            failed: false,
        })
    }
    pub fn config(&self) -> AudioConfig {
        self.output.clone()
    }
    pub fn push(&mut self, frame: &AudioFrame) -> Result<Vec<AudioFrame>> {
        let result = self.push_inner(frame);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn push_inner(&mut self, frame: &AudioFrame) -> Result<Vec<AudioFrame>> {
        if self.encoder.is_none() {
            return Ok(vec![frame.clone()]);
        }
        if self.failed || self.expected.is_some_and(|ts| ts != frame.timestamp) {
            self.encoder = Some(aac_encoder(&self.input)?);
            if let Some(opus) = &mut self.opus {
                opus.reset_state()?;
            }
            self.pending.clear();
            self.origin = None;
            self.emitted = 0;
            self.input_start = None;
            self.failed = false;
        }
        self.expected = Some(frame.timestamp + u64::from(frame.duration));
        let start = *self.input_start.get_or_insert(frame.timestamp);
        let origin = (i128::from(frame.timestamp_90k)
            - i128::from(frame.timestamp - start) * 90000 / i128::from(self.output.clock_rate))
        .clamp(0, i128::from(u64::MAX)) as u64;
        self.origin = Some(origin);
        match self.input.codec {
            AudioCodec::Pcmu => self
                .pending
                .extend(frame.data.iter().map(|b| mulaw_to_linear(*b))),
            AudioCodec::Pcma => self
                .pending
                .extend(frame.data.iter().map(|b| alaw_to_linear(*b))),
            AudioCodec::Opus => {
                let mut pcm = vec![0i16; 5760 * 2];
                let count = self
                    .opus
                    .as_mut()
                    .unwrap()
                    .decode(&frame.data, &mut pcm, false)?;
                ensure!(
                    count == frame.duration as usize,
                    "Opus packet duration mismatch"
                );
                self.pending.extend_from_slice(&pcm[..count * 2]);
            }
            AudioCodec::Aac => unreachable!(),
        }
        ensure!(self.pending.len() <= 16384, "AAC PCM queue overflow");
        let needed = 1024 * usize::from(self.input.channels);
        let mut output = Vec::new();
        while self.pending.len() >= needed {
            let mut data = vec![0u8; 8192];
            let result = self
                .encoder
                .as_ref()
                .unwrap()
                .encode(&self.pending[..needed], &mut data)
                .map_err(|e| anyhow::anyhow!("AAC encode: {e}"))?;
            ensure!(
                result.input_consumed == needed,
                "AAC encoder did not consume a complete PCM frame"
            );
            self.pending.drain(..needed);
            if result.output_size == 0 {
                continue;
            }
            data.truncate(result.output_size);
            let position = self.emitted;
            self.emitted += 1024;
            // Suppress encoder priming frames, and correct the remaining timeline
            // by the encoder-reported delay instead of shifting audio behind video.
            if position < u64::from(self.delay) {
                continue;
            }
            let timestamp = position - u64::from(self.delay);
            output.push(AudioFrame {
                data: data.into(),
                timestamp,
                duration: 1024,
                timestamp_90k: origin + timestamp * 90000 / u64::from(self.output.clock_rate),
            });
        }
        Ok(output)
    }
}
fn mulaw_to_linear(byte: u8) -> i16 {
    let u = !byte;
    let magnitude = (((i32::from(u & 15) << 3) + 132) << ((u >> 4) & 7)) - 132;
    (if u & 128 != 0 { -magnitude } else { magnitude }) as i16
}
fn alaw_to_linear(byte: u8) -> i16 {
    let a = byte ^ 0x55;
    let exponent = (a >> 4) & 7;
    let mut value = i32::from(a & 15) << 4;
    value += if exponent == 0 { 8 } else { 0x108 };
    if exponent > 1 {
        value <<= exponent - 1;
    }
    (if a & 128 != 0 { value } else { -value }) as i16
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn g711_reference_values() {
        assert_eq!(linear_to_mulaw(0), 0xff);
        assert_eq!(linear_to_mulaw(1), 0xff);
        assert_eq!(linear_to_mulaw(-1), 0x7f);
        assert_eq!(linear_to_mulaw(32767), 0x80);
        assert_eq!(linear_to_mulaw(-32768), 0);
        assert_eq!(linear_to_mulaw(1000), 0xce);
    }
    #[test]
    fn passthrough_preserves_packets() {
        let config = AudioConfig {
            codec: AudioCodec::Opus,
            clock_rate: 48000,
            channels: 2,
            config: vec![],
        };
        let mut converter = RtcAudio::new(&config).unwrap();
        let frame = AudioFrame {
            data: vec![0xf8, 0xff, 0xfe].into(),
            timestamp: 120,
            duration: 960,
            timestamp_90k: 225,
        };
        let output = converter.push(&frame).unwrap();
        assert_eq!(output[0].data, frame.data);
        assert_eq!(output[0].data.as_ptr(), frame.data.as_ptr());
        assert_eq!(output[0].timestamp_90k, 225);
        assert_eq!(rtc_config(&config), config);
    }
    #[test]
    fn g711_and_opus_archive_aac_decode_and_encoder_delay() {
        for codec in [AudioCodec::Pcmu, AudioCodec::Pcma, AudioCodec::Opus] {
            let rate = if codec == AudioCodec::Opus {
                48000
            } else {
                8000
            };
            let channels = if codec == AudioCodec::Opus { 2 } else { 1 };
            let config = AudioConfig {
                codec,
                clock_rate: rate,
                channels,
                config: vec![],
            };
            let mut converter = ArchiveAudio::new(&config).unwrap();
            let archive = converter.config();
            assert_eq!(archive.codec, AudioCodec::Aac);
            let mut decoder = RtcAudio::new(&archive).unwrap();
            let mut encoder =
                opus::Encoder::new(48000, opus::Channels::Stereo, opus::Application::Audio)
                    .unwrap();
            let count = rate / 50;
            let mut compressed = Vec::new();
            for index in 0..100u64 {
                let pcm = (0..count)
                    .map(|n| {
                        (((index * u64::from(count) + u64::from(n)) as f32
                            * 440.0
                            * std::f32::consts::TAU
                            / rate as f32)
                            .sin()
                            * 12000.0) as i16
                    })
                    .collect::<Vec<_>>();
                let data = match codec {
                    AudioCodec::Pcmu => pcm.iter().map(|v| linear_to_mulaw(*v)).collect(),
                    // A-law reference alternating nonzero samples exercises decoder path.
                    AudioCodec::Pcma => (0..count)
                        .map(|n| if n % 18 < 9 { 0x9a } else { 0x1a })
                        .collect(),
                    AudioCodec::Opus => {
                        let stereo = pcm.iter().flat_map(|v| [*v, *v]).collect::<Vec<_>>();
                        let mut data = vec![0; 4000];
                        let n = encoder.encode(&stereo, &mut data).unwrap();
                        data.truncate(n);
                        data
                    }
                    _ => unreachable!(),
                };
                compressed.extend(
                    converter
                        .push(&AudioFrame {
                            data: data.into(),
                            timestamp: index * u64::from(count),
                            duration: count,
                            timestamp_90k: 9000 + index * 1800,
                        })
                        .unwrap(),
                );
            }
            assert!(compressed.len() > 8, "{codec:?} yielded insufficient AAC");
            let delay = u64::from(converter.delay);
            let expected_first =
                9000 + (delay.div_ceil(1024) * 1024 - delay) * 90000 / u64::from(rate);
            assert_eq!(
                compressed[0].timestamp_90k, expected_first,
                "{codec:?} priming delay"
            );
            let mut output = Vec::new();
            for frame in &compressed {
                output.extend(decoder.push(frame).unwrap());
            }
            assert!(output.len() > 20, "{codec:?} decode empty");
            let mut opus_decoder = opus::Decoder::new(48000, opus::Channels::Stereo).unwrap();
            let mut nonzero = false;
            for packet in &output {
                let mut pcm = vec![0; 1920];
                let count = opus_decoder.decode(&packet.data, &mut pcm, false).unwrap();
                nonzero |= pcm[..count * 2].iter().any(|v| v.abs() > 100);
            }
            assert!(nonzero, "{codec:?} decoded silent");
            assert!(compressed
                .windows(2)
                .all(|w| w[1].timestamp == w[0].timestamp + 1024));
        }
    }
}
