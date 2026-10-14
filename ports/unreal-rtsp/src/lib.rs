//! Original-derived RTSP -> compressed H264/AVCC boundary.
//! See docs/original-rtsp-native.md: 0x18001e810/0x18001ea40 (depacketization),
//! 0x18000ff30 (timestamp grouping), 0x1800100b0 (AVCC), 0x180010250 (SPS/PPS).
//! Invalid/lost packets are dropped until an IDR; unsafe original bounds are not preserved.
mod audio;
mod auth;
pub use audio::{AudioCodec, AudioConfig, AudioFrame};
mod h264;
mod rtp;
mod sync;

use anyhow::{anyhow, bail, ensure, Context, Result};
use base64::Engine;
use rtp::Assembler;
use std::{
    collections::{HashMap, VecDeque},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::{timeout, Instant},
};
use url::Url;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoConfig {
    pub sps: Vec<u8>,
    pub pps: Vec<u8>,
    pub width: u32,
    pub height: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    H265,
}
pub fn h264_video_config(sps: Vec<u8>, pps: Vec<u8>) -> Result<VideoConfig> {
    ensure!(
        sps.first().is_some_and(|v| v & 31 == 7) && pps.first().is_some_and(|v| v & 31 == 8),
        "Invalid H264 parameter sets"
    );
    let (width, height) = h264::dimensions(&sps)?;
    Ok(VideoConfig {
        sps,
        pps,
        width,
        height,
    })
}
#[derive(Clone, Debug)]
pub struct Frame {
    pub avcc: Vec<u8>,
    pub timestamp_90k: u64,
    pub duration_90k: u32,
    pub keyframe: bool,
}

#[derive(Clone, Debug)]
pub enum MediaEvent {
    Video(Frame),
    Audio(AudioFrame),
}

/// One TCP-interleaved RTSP session. Dropping it closes the camera connection.
pub struct Client {
    codec: VideoCodec,
    socket: TcpStream,
    wire: Vec<u8>,
    cseq: u32,
    uri: String,
    session: Option<String>,
    credentials: Option<(String, String)>,
    auth: Option<auth::Challenge>,
    auth_count: u32,
    assembler: Option<Assembler>,
    pending: VecDeque<MediaEvent>,
    audio: Option<audio::Assembler>,
    audio_warning: Option<String>,
    audio_channel: Option<u8>,
    origin: Option<Instant>,
    video_anchor: Option<u64>,
    synchronization: sync::ClockSync,
    rtp_channel: u8,
    keepalive_period: Duration,
    keepalive_at: Instant,
    get_parameter: bool,
}

impl Client {
    pub async fn connect(address: &str) -> Result<Self> {
        let mut url = Url::parse(address).context("Invalid RTSP URL")?;
        ensure!(
            url.scheme() == "rtsp",
            "Only rtsp:// TCP transport is supported"
        );
        let host = url.host_str().context("RTSP URL has no host")?.to_owned();
        let port = url.port().unwrap_or(554);
        let credentials = if !url.username().is_empty() {
            Some((
                percent_encoding::percent_decode_str(url.username())
                    .decode_utf8()?
                    .into_owned(),
                percent_encoding::percent_decode_str(url.password().unwrap_or(""))
                    .decode_utf8()?
                    .into_owned(),
            ))
        } else {
            None
        };
        url.set_username("").map_err(|_| anyhow!("Invalid URL"))?;
        url.set_password(None).map_err(|_| anyhow!("Invalid URL"))?;
        let socket = timeout(
            Duration::from_secs(10),
            TcpStream::connect((host.as_str(), port)),
        )
        .await
        .context("RTSP connect timed out")??;
        socket.set_nodelay(true)?;
        let mut client = Self {
            codec: VideoCodec::H264,
            socket,
            wire: Vec::new(),
            cseq: 0,
            uri: url.to_string(),
            session: None,
            credentials,
            auth: None,
            auth_count: 0,
            assembler: None,
            pending: VecDeque::new(),
            audio: None,
            audio_warning: None,
            audio_channel: None,
            origin: None,
            video_anchor: None,
            synchronization: sync::ClockSync::default(),
            rtp_channel: 0,
            keepalive_period: Duration::from_secs(20),
            keepalive_at: Instant::now() + Duration::from_secs(20),
            get_parameter: false,
        };
        let uri = client.uri.clone();
        let options = client.request("OPTIONS", &uri, &[]).await?;
        client.get_parameter = options
            .headers
            .get("public")
            .is_some_and(|s| s.split(',').any(|s| s.trim() == "GET_PARAMETER"));
        let description = client
            .request("DESCRIBE", &uri, &[("Accept", "application/sdp")])
            .await?;
        ensure!(
            description.status == 200,
            "RTSP DESCRIBE failed: {}",
            description.status
        );
        let base = description
            .headers
            .get("content-base")
            .or_else(|| description.headers.get("content-location"))
            .cloned()
            .unwrap_or(uri.clone());
        let base = url.join(&base).context("Invalid SDP content base")?;
        let sdp = std::str::from_utf8(&description.body).context("SDP is not UTF8")?;
        let track = parse_sdp(sdp, &base)?;
        let audio_track = audio::parse_sdp(sdp, &base)?;
        if audio_track.is_none() && sdp.lines().any(|l| l.trim().starts_with("m=audio ")) {
            client.audio_warning=Some("Camera audio codec or RTP format is unsupported; supported: AAC-LC MPEG4-GENERIC, PCMU, PCMA, Opus".into());
        }
        client.codec = track.codec;
        client.assembler = Some(match track.codec {
            VideoCodec::H264 => Assembler::new(track.payload_type, track.video),
            VideoCodec::H265 => Assembler::new_hevc(track.payload_type, track.video, track.vps),
        });
        if let Some(control) = track.aggregate {
            client.uri = control;
        }
        let setup = client
            .request(
                "SETUP",
                &track.control,
                &[("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1")],
            )
            .await?;
        ensure!(setup.status == 200, "RTSP SETUP failed: {}", setup.status);
        let session = setup
            .headers
            .get("session")
            .context("SETUP omitted session")?;
        let mut fields = session.split(';');
        let id = fields.next().unwrap_or("").trim();
        ensure!(!id.is_empty() && id.len() < 1024, "Invalid session");
        client.session = Some(id.to_owned());
        for field in fields {
            if let Some(n) = field
                .trim()
                .strip_prefix("timeout=")
                .and_then(|v| v.parse::<u64>().ok())
            {
                client.keepalive_period = Duration::from_secs((n / 2).clamp(1, 30));
            }
        }
        let transport = setup
            .headers
            .get("transport")
            .context("SETUP omitted transport")?;
        ensure!(
            transport.to_ascii_uppercase().contains("RTP/AVP/TCP"),
            "Camera did not accept TCP interleaving"
        );
        if let Some(ch) = transport
            .split(';')
            .find_map(|s| s.trim().strip_prefix("interleaved="))
        {
            client.rtp_channel = ch
                .split('-')
                .next()
                .context("Invalid RTP channel")?
                .parse()
                .context("Invalid RTP channel")?;
        }
        if let Some(track) = audio_track {
            let setup = client
                .request(
                    "SETUP",
                    &track.control,
                    &[("Transport", "RTP/AVP/TCP;unicast;interleaved=2-3")],
                )
                .await?;
            ensure!(
                setup.status == 200,
                "RTSP audio SETUP failed: {}",
                setup.status
            );
            let transport = setup
                .headers
                .get("transport")
                .context("Audio SETUP omitted transport")?;
            ensure!(
                transport.to_ascii_uppercase().contains("RTP/AVP/TCP"),
                "Audio requires TCP interleaving"
            );
            let channel = transport
                .split(';')
                .find_map(|s| s.trim().strip_prefix("interleaved="))
                .and_then(|s| s.split('-').next())
                .context("Audio SETUP omitted channels")?
                .parse::<u8>()?;
            ensure!(
                channel < 255
                    && channel != client.rtp_channel
                    && channel != client.rtp_channel.saturating_add(1)
                    && channel.saturating_add(1) != client.rtp_channel,
                "Audio/video channels overlap"
            );
            client.audio_channel = Some(channel);
            client.audio = Some(audio::Assembler::new(track));
        }
        client.origin = None;
        let uri = client.uri.clone();
        let play = client
            .request("PLAY", &uri, &[("Range", "npt=0.000-")])
            .await?;
        ensure!(play.status == 200, "RTSP PLAY failed: {}", play.status);
        client.keepalive_at = Instant::now() + client.keepalive_period;
        // Like the original media-type detector fallback, accept in-band SPS/PPS.
        let deadline = Instant::now() + Duration::from_secs(10);
        while !client.assembler.as_ref().unwrap().ready() {
            let event = timeout(
                deadline.saturating_duration_since(Instant::now()),
                client.read_event(),
            )
            .await
            .context("No video parameter sets received")??;
            client.accept_event(event)?;
        }
        Ok(client)
    }
    pub fn video(&self) -> &VideoConfig {
        self.assembler
            .as_ref()
            .expect("initialized RTSP client")
            .video()
    }
    pub fn video_codec(&self) -> VideoCodec {
        self.codec
    }
    pub fn audio(&self) -> Option<&AudioConfig> {
        self.audio.as_ref().map(|a| &a.config)
    }
    pub fn audio_warning(&self) -> Option<&str> {
        self.audio_warning.as_deref()
    }
    /// Whether sender reports correlate both tracks, and current audio correction in 90-kHz ticks.
    pub fn audio_sync(&self) -> (bool, i64) {
        self.synchronization.status()
    }
    pub async fn next_frame(&mut self) -> Result<Frame> {
        loop {
            if let MediaEvent::Video(frame) = self.next_media().await? {
                return Ok(frame);
            }
        }
    }
    pub async fn next_media(&mut self) -> Result<MediaEvent> {
        let frame_deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(frame) = self.pending.pop_front() {
                return Ok(frame);
            }
            ensure!(
                Instant::now() < frame_deadline,
                "RTSP source produced no complete frame for 15 seconds"
            );
            if Instant::now() >= self.keepalive_at {
                let uri = self.uri.clone();
                let method = if self.get_parameter {
                    "GET_PARAMETER"
                } else {
                    "OPTIONS"
                };
                let response = self.request(method, &uri, &[]).await?;
                ensure!(
                    response.status == 200,
                    "RTSP keepalive failed: {}",
                    response.status
                );
                self.keepalive_at = Instant::now() + self.keepalive_period;
                continue;
            }
            let wait = self
                .keepalive_at
                .saturating_duration_since(Instant::now())
                .min(frame_deadline.saturating_duration_since(Instant::now()));
            match timeout(wait, self.read_event()).await {
                Ok(event) => self.accept_event(event?)?,
                Err(_) if Instant::now() >= self.keepalive_at => continue,
                Err(_) => bail!("RTSP source stalled for 15 seconds"),
            }
        }
    }
    fn accept_event(&mut self, event: Event) -> Result<()> {
        if let Event::Packet(channel, bytes) = event {
            if channel == self.rtp_channel.saturating_add(1) {
                self.synchronization.report(false, &bytes);
                return Ok(());
            }
            if self
                .audio_channel
                .is_some_and(|ch| channel == ch.saturating_add(1))
            {
                self.synchronization.report(true, &bytes);
                return Ok(());
            }
            if channel != self.rtp_channel && self.audio_channel != Some(channel) {
                return Ok(());
            }
            let Ok(packet) = rtp::packet(&bytes) else {
                return Ok(());
            };
            let arrival = (self
                .origin
                .get_or_insert_with(Instant::now)
                .elapsed()
                .as_nanos()
                * 90000
                / 1_000_000_000) as u64;
            if self.audio_channel == Some(channel) {
                if let Some(audio) = &mut self.audio {
                    if let Ok(mut frames) = audio.push(&bytes, arrival) {
                        self.synchronization.rtp(
                            true,
                            packet.ssrc,
                            packet.timestamp,
                            audio.presentation_clock(),
                            audio.config.clock_rate,
                        );
                        for frame in &mut frames {
                            frame.timestamp_90k = self.synchronization.adjust(
                                frame.timestamp_90k,
                                u64::from(frame.duration) * 90000
                                    / u64::from(audio.config.clock_rate),
                            );
                        }
                        ensure!(
                            self.pending.len() + frames.len() <= 256,
                            "RTSP pending queue overflow"
                        );
                        self.pending
                            .extend(frames.into_iter().map(MediaEvent::Audio));
                    }
                }
            }
            if channel == self.rtp_channel {
                let anchor = *self.video_anchor.get_or_insert(arrival);
                if let Some(assembler) = &mut self.assembler {
                    let previous = assembler.video().clone();
                    let frame = assembler.push(&bytes)?;
                    self.synchronization.rtp(
                        false,
                        packet.ssrc,
                        packet.timestamp,
                        anchor + assembler.clock(),
                        90000,
                    );
                    ensure!(previous.sps.is_empty() || previous.pps.is_empty() || &previous==assembler.video(),
                        "H264 parameter sets changed; reconnect required to refresh media configuration");
                    if let Some(mut frame) = frame {
                        frame.timestamp_90k = frame.timestamp_90k.saturating_add(anchor);
                        ensure!(
                            self.pending.len() < 128
                                && self
                                    .pending
                                    .iter()
                                    .map(|f| match f {
                                        MediaEvent::Video(f) => f.avcc.len(),
                                        MediaEvent::Audio(f) => f.data.len(),
                                    })
                                    .sum::<usize>()
                                    + frame.avcc.len()
                                    <= 32 * 1024 * 1024,
                            "RTSP pending frame queue overflow"
                        );
                        self.pending.push_back(MediaEvent::Video(frame));
                    }
                }
            }
        }
        Ok(())
    }
    async fn request(
        &mut self,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
    ) -> Result<Response> {
        ensure!(!uri.contains(['\r', '\n']), "Invalid RTSP request URI");
        for retry in 0..2 {
            self.cseq = self
                .cseq
                .checked_add(1)
                .context("RTSP sequence exhausted")?;
            let cseq = self.cseq;
            let mut text = format!(
                "{method} {uri} RTSP/1.0\r\nCSeq: {cseq}\r\nUser-Agent: uvms-original-port/1\r\n"
            );
            if let Some(session) = &self.session {
                text.push_str(&format!("Session: {session}\r\n"));
            }
            if let (Some(challenge), Some((user, password))) = (&self.auth, &self.credentials) {
                self.auth_count = self
                    .auth_count
                    .checked_add(1)
                    .context("Authentication sequence exhausted")?;
                let authorization =
                    challenge.authorization(user, password, method, uri, self.auth_count)?;
                text.push_str(&format!("Authorization: {authorization}\r\n"));
            }
            for (key, value) in headers {
                text.push_str(&format!("{key}: {value}\r\n"));
            }
            text.push_str("\r\n");
            timeout(
                Duration::from_secs(10),
                self.socket.write_all(text.as_bytes()),
            )
            .await
            .context("RTSP write timed out")??;
            let deadline = Instant::now() + Duration::from_secs(10);
            let response = loop {
                let event = timeout(
                    deadline.saturating_duration_since(Instant::now()),
                    self.read_event(),
                )
                .await
                .context("RTSP response timed out")??;
                if let Event::Response(response) = event {
                    ensure!(
                        response
                            .headers
                            .get("cseq")
                            .and_then(|n| n.parse::<u32>().ok())
                            == Some(cseq),
                        "RTSP response sequence mismatch"
                    );
                    break response;
                } else {
                    self.accept_event(event)?;
                }
            };
            if response.status == 401 && retry == 0 && self.credentials.is_some() {
                self.auth = Some(auth::Challenge::parse(
                    response
                        .headers
                        .get("www-authenticate")
                        .context("Missing authentication challenge")?,
                )?);
                continue;
            }
            return Ok(response);
        }
        bail!("RTSP authentication failed")
    }
    async fn read_event(&mut self) -> Result<Event> {
        loop {
            if let Some((event, used)) = parse_event(&self.wire)? {
                self.wire.drain(..used);
                return Ok(event);
            }
            ensure!(
                self.wire.len() < 2 * 1024 * 1024,
                "RTSP wire buffer exceeded limit"
            );
            let mut chunk = [0u8; 16384];
            let n = self.socket.read(&mut chunk).await?;
            ensure!(n > 0, "RTSP connection closed");
            self.wire.extend_from_slice(&chunk[..n]);
        }
    }
}

#[derive(Debug)]
struct Response {
    status: u16,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}
enum Event {
    Response(Response),
    Packet(u8, Vec<u8>),
}
fn parse_event(bytes: &[u8]) -> Result<Option<(Event, usize)>> {
    if bytes.is_empty() {
        return Ok(None);
    }
    if bytes[0] == b'$' {
        if bytes.len() < 4 {
            return Ok(None);
        }
        let n = u16::from_be_bytes([bytes[2], bytes[3]]) as usize;
        if bytes.len() < n + 4 {
            return Ok(None);
        }
        return Ok(Some((
            Event::Packet(bytes[1], bytes[4..4 + n].to_vec()),
            n + 4,
        )));
    }
    let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") else {
        ensure!(bytes.len() < 32768, "RTSP header too large");
        return Ok(None);
    };
    ensure!(end < 32768, "RTSP header too large");
    let text = std::str::from_utf8(&bytes[..end]).context("Invalid RTSP header encoding")?;
    let mut lines = text.split("\r\n");
    let mut status = lines
        .next()
        .context("Missing RTSP status")?
        .split_whitespace();
    ensure!(status.next() == Some("RTSP/1.0"), "Unexpected RTSP message");
    let status = status.next().context("Missing RTSP status")?.parse()?;
    let mut headers = HashMap::new();
    for line in lines {
        let (k, v) = line.split_once(':').context("Malformed RTSP header")?;
        headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_owned());
    }
    let length = headers
        .get("content-length")
        .map(|s| s.parse::<usize>())
        .transpose()?
        .unwrap_or(0);
    ensure!(length <= 1024 * 1024, "RTSP body too large");
    let used = end + 4 + length;
    if bytes.len() < used {
        return Ok(None);
    }
    Ok(Some((
        Event::Response(Response {
            status,
            headers,
            body: bytes[end + 4..used].to_vec(),
        }),
        used,
    )))
}

struct Track {
    codec: VideoCodec,
    vps: Vec<u8>,
    payload_type: u8,
    control: String,
    aggregate: Option<String>,
    video: VideoConfig,
}
fn parse_sdp(sdp: &str, base: &Url) -> Result<Track> {
    let mut sections: Vec<Vec<&str>> = vec![Vec::new()];
    for line in sdp.lines().map(str::trim) {
        if line.starts_with("m=") {
            sections.push(Vec::new());
        }
        sections.last_mut().unwrap().push(line);
    }
    let aggregate = sections[0]
        .iter()
        .find_map(|l| l.strip_prefix("a=control:"))
        .filter(|s| *s != "*")
        .map(|s| resolve_control(base, s))
        .transpose()?;
    for lines in sections.iter().skip(1) {
        if !lines.first().is_some_and(|l| l.starts_with("m=video ")) {
            continue;
        }
        for map in lines.iter().filter_map(|l| l.strip_prefix("a=rtpmap:")) {
            let Some((pt, codec)) = map.split_once(' ') else {
                continue;
            };
            let hevc = codec.trim().eq_ignore_ascii_case("H265/90000");
            if !hevc && !codec.trim().eq_ignore_ascii_case("H264/90000") {
                continue;
            }
            let payload_type: u8 = pt.parse()?;
            ensure!(payload_type < 128, "Invalid H264 payload type");
            let prefix = format!("a=fmtp:{pt} ");
            let mut sps = Vec::new();
            let mut pps = Vec::new();
            let mut vps = Vec::new();
            if let Some(fmtp) = lines.iter().find_map(|s| s.strip_prefix(&prefix)) {
                for field in fmtp.split(';').map(str::trim) {
                    if hevc {
                        if let Some(don) = field.strip_prefix("sprop-max-don-diff=") {
                            ensure!(don == "0", "HEVC DONL mode unsupported");
                        }
                        for (name, kind, dest) in [
                            ("sprop-vps=", 32, &mut vps),
                            ("sprop-sps=", 33, &mut sps),
                            ("sprop-pps=", 34, &mut pps),
                        ] {
                            if let Some(encoded) = field.strip_prefix(name) {
                                ensure!(
                                    !encoded.contains(','),
                                    "Multiple HEVC parameter sets unsupported"
                                );
                                let nal =
                                    base64::engine::general_purpose::STANDARD.decode(encoded)?;
                                ensure!(
                                    nal.len() >= 2
                                        && nal.len() <= 65535
                                        && (nal[0] >> 1) & 63 == kind
                                        && nal[0] & 0x81 == 0
                                        && nal[1] & 0xf8 == 0
                                        && nal[1] & 7 != 0,
                                    "Invalid HEVC parameter set"
                                );
                                *dest = nal;
                            }
                        }
                        continue;
                    }
                    if let Some(mode) = field.strip_prefix("packetization-mode=") {
                        ensure!(
                            mode == "0" || mode == "1",
                            "Interleaved H264 packetization mode 2 unsupported"
                        );
                    }
                    if let Some(value) = field.strip_prefix("sprop-parameter-sets=") {
                        for encoded in value.split(',') {
                            let nal =
                                base64::engine::general_purpose::STANDARD.decode(encoded.trim())?;
                            ensure!(
                                !nal.is_empty() && nal.len() <= 65535,
                                "Invalid parameter set"
                            );
                            match nal[0] & 31 {
                                7 => sps = nal,
                                8 => pps = nal,
                                _ => {}
                            }
                        }
                    }
                }
            }
            let (width, height) = if sps.is_empty() || hevc {
                (0, 0)
            } else {
                h264::dimensions(&sps)?
            };
            let control = lines
                .iter()
                .find_map(|s| s.strip_prefix("a=control:"))
                .context("H264 track lacks control URI")?;
            return Ok(Track {
                codec: if hevc {
                    VideoCodec::H265
                } else {
                    VideoCodec::H264
                },
                vps,
                payload_type,
                control: resolve_control(base, control)?,
                aggregate,
                video: VideoConfig {
                    sps,
                    pps,
                    width,
                    height,
                },
            });
        }
    }
    bail!("SDP has no H264/90000 or H265/90000 video track")
}
fn resolve_control(base: &Url, control: &str) -> Result<String> {
    if control == "*" {
        return Ok(base.to_string());
    }
    Ok(base
        .join(control)
        .context("Invalid SDP control URI")?
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hevc_sdp_parameter_sets_and_don_mode() {
        let s="v=0\nm=video 0 RTP/AVP 98\na=rtpmap:98 H265/90000\na=fmtp:98 sprop-vps=QAEBAQ==;sprop-sps=QgECAg==;sprop-pps=RAEDAw==;sprop-max-don-diff=0\na=control:trackID=0\n";
        let base = Url::parse("rtsp://localhost/camera/").unwrap();
        let track = parse_sdp(s, &base).unwrap();
        assert_eq!(track.codec, VideoCodec::H265);
        assert_eq!(track.vps, vec![64, 1, 1, 1]);
        assert_eq!(track.video.sps, vec![66, 1, 2, 2]);
        assert_eq!(track.payload_type, 98);
        assert!(parse_sdp(&s.replace("don-diff=0", "don-diff=1"), &base).is_err());
    }
    #[test]
    fn response_and_interleaved_fragmentation() {
        let data = b"RTSP/1.0 200 OK\r\nCSeq: 4\r\nContent-Length: 3\r\n\r\nabc$\x00\x00\x03xyz";
        assert!(parse_event(&data[..40]).unwrap().is_none());
        let (_, used) = parse_event(data).unwrap().unwrap();
        assert!(
            matches!(parse_event(&data[used..]).unwrap(),Some((Event::Packet(0,p),7)) if p==b"xyz")
        );
        assert!(parse_event(b"RTSP/1.0 200 OK\r\nContent-Length: 99999999\r\n\r\n").is_err());
    }
    #[test]
    fn sdp_selects_video_and_resolves_track() {
        let s="v=0\r\na=control:*\r\nm=audio 0 RTP/AVP 96\r\na=rtpmap:96 OPUS/48000/2\r\nm=video 0 RTP/AVP 97\r\na=rtpmap:97 H264/90000\r\na=fmtp:97 packetization-mode=1;sprop-parameter-sets=Z0LAFtoCgL/lwEQAAAMABAAAAwB6PFi6gA==,aM4PyA==\r\na=control:trackID=1\r\n";
        let t = parse_sdp(s, &Url::parse("rtsp://localhost/camera/").unwrap()).unwrap();
        assert_eq!(t.control, "rtsp://localhost/camera/trackID=1");
        assert_eq!((t.video.width, t.video.height), (640, 360));
    }
    #[tokio::test]
    async fn authenticated_session_and_keepalive_with_nonzero_rtp_channel() {
        for digest in [false, true] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut first = true;
                for expected in [
                    "OPTIONS",
                    "OPTIONS",
                    "DESCRIBE",
                    "SETUP",
                    "PLAY",
                    "GET_PARAMETER",
                ] {
                    let mut request = Vec::new();
                    while !request.ends_with(b"\r\n\r\n") {
                        request.push(socket.read_u8().await.unwrap());
                        assert!(request.len() < 8192);
                    }
                    let text = String::from_utf8(request).unwrap();
                    let mut words = text.lines().next().unwrap().split_whitespace();
                    let method = words.next().unwrap();
                    let uri = words.next().unwrap();
                    assert_eq!(method, expected);
                    assert!(!uri.contains("user:password"));
                    let cseq = text.lines().find_map(|l| l.strip_prefix("CSeq: ")).unwrap();
                    if first {
                        let challenge = if digest {
                            "Digest realm=\"camera\",nonce=\"testnonce\",algorithm=MD5,qop=\"auth\""
                        } else {
                            "Basic realm=\"camera\""
                        };
                        socket.write_all(format!("RTSP/1.0 401 Unauthorized\r\nCSeq: {cseq}\r\nWWW-Authenticate: {challenge}\r\n\r\n").as_bytes()).await.unwrap();
                        first = false;
                        continue;
                    }
                    let header = text
                        .lines()
                        .find_map(|l| l.strip_prefix("Authorization: "))
                        .unwrap();
                    if digest {
                        let auth::Challenge::Digest(fields) =
                            auth::Challenge::parse(header).unwrap()
                        else {
                            panic!("Wrong auth scheme")
                        };
                        let ha1 = format!("{:x}", md5::compute("user:camera:password"));
                        let ha2 = format!("{:x}", md5::compute(format!("{method}:{uri}")));
                        let response = format!(
                            "{:x}",
                            md5::compute(format!(
                                "{ha1}:testnonce:{}:{}:auth:{ha2}",
                                fields["nc"], fields["cnonce"]
                            ))
                        );
                        assert_eq!(fields["response"], response);
                    } else {
                        assert_eq!(header, "Basic dXNlcjpwYXNzd29yZA==");
                    }
                    let extra=match method {
                        "DESCRIBE"=>{
                            let sdp="v=0\r\na=control:*\r\nm=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\na=fmtp:96 packetization-mode=1;sprop-parameter-sets=Z0LAFtoCgL/lwEQAAAMABAAAAwB6PFi6gA==,aM4PyA==\r\na=control:trackID=0\r\n";
                            format!("Content-Base: rtsp://{addr}/camera/\r\nContent-Length: {}\r\n\r\n{sdp}",sdp.len())
                        },
                        "SETUP"=>"Session: test-session;timeout=2\r\nTransport: RTP/AVP/TCP;unicast;interleaved=4-5\r\n\r\n".to_owned(),
                        _=>"Public: OPTIONS, DESCRIBE, SETUP, PLAY, GET_PARAMETER\r\n\r\n".to_owned(),
                    };
                    if method == "PLAY" || method == "GET_PARAMETER" {
                        assert!(text.contains("Session: test-session\r\n"));
                    }
                    socket
                        .write_all(format!("RTSP/1.0 200 OK\r\nCSeq: {cseq}\r\n{extra}").as_bytes())
                        .await
                        .unwrap();
                    let packets = if method == "PLAY" {
                        vec![(1u16, 0u32, 0x65), (2, 3000, 0x41), (3, 6000, 0x41)]
                    } else if method == "GET_PARAMETER" {
                        vec![(4, 9000, 0x41)]
                    } else {
                        Vec::new()
                    };
                    for (seq, ts, nal) in packets {
                        let mut payload = vec![0x80, 96];
                        payload.extend(seq.to_be_bytes());
                        payload.extend(ts.to_be_bytes());
                        payload.extend(1u32.to_be_bytes());
                        payload.extend([nal, 1, 2]);
                        let mut wire = vec![b'$', 4];
                        wire.extend((payload.len() as u16).to_be_bytes());
                        wire.extend(payload);
                        socket.write_all(&wire).await.unwrap();
                    }
                }
            });
            let mut c = Client::connect(&format!("rtsp://user:password@{addr}/camera"))
                .await
                .unwrap();
            assert!(c.next_frame().await.unwrap().keyframe);
            assert!(!c.next_frame().await.unwrap().keyframe);
            tokio::time::sleep(Duration::from_millis(1100)).await;
            assert_eq!(c.next_frame().await.unwrap().timestamp_90k, 6000);
            server.await.unwrap();
        }
    }
}
