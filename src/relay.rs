//! RTSP 1.0 TCP-interleaved retransmission of shared compressed camera frames.
//! Standard delivery reconstruction, not a verbatim original RTSP server port.
use crate::{
    ingest::{Feed, SharedFrame},
    App,
};
use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{tcp::OwnedReadHalf, TcpListener, TcpStream},
    sync::{broadcast, mpsc},
    task::JoinSet,
    time::{timeout, Instant},
};
use unreal_rtsp::{AudioCodec, AudioConfig, AudioFrame, Frame, VideoConfig};

const HEADER_LIMIT: usize = 16 * 1024;
const BODY_LIMIT: usize = 4096;
const PUBLIC: &str = "OPTIONS, DESCRIBE, SETUP, PLAY, TEARDOWN, GET_PARAMETER";

pub async fn run(listener: TcpListener, app: Arc<App>) {
    let mut clients = JoinSet::new();
    loop {
        tokio::select! {
            _=app.shutdown.cancelled()=>break,
            result=listener.accept()=>match result {
                Ok((mut socket,_))=>{
                    let Ok(permit)=app.slots.clone().try_acquire_owned() else {
                        let _=timeout(Duration::from_millis(200),socket.write_all(b"RTSP/1.0 503 Service Unavailable\r\nCSeq: 0\r\nContent-Length: 0\r\n\r\n")).await;
                        continue;
                    };
                    let app=app.clone();
                    clients.spawn(async move {let _permit=permit;let _=session(socket,app).await;});
                },
                Err(_)=>break,
            },
            _=clients.join_next(), if !clients.is_empty()=>{},
        }
    }
    clients.abort_all();
    while clients.join_next().await.is_some() {}
}

#[derive(Debug)]
struct Request {
    method: String,
    uri: String,
    cseq: u32,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

/// Drop guard ensures the connection reader is not detached when a viewer task
/// is cancelled or a write times out.
struct ReaderTask(tokio::task::JoinHandle<()>);
impl Drop for ReaderTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn session(socket: TcpStream, app: Arc<App>) -> Result<()> {
    socket.set_nodelay(true)?;
    let (read, mut write) = socket.into_split();
    let (requests_tx, mut requests) = mpsc::channel(8);
    let _reader = ReaderTask(tokio::spawn(async move {
        let mut read = read;
        let mut wire = Vec::new();
        loop {
            let request =
                timeout(Duration::from_secs(60), read_request(&mut read, &mut wire)).await;
            match request {
                Ok(Ok(request)) => {
                    if requests_tx.send(request).await.is_err() {
                        break;
                    }
                }
                _ => break,
            }
        }
    }));
    let session_id = uuid::Uuid::new_v4().simple().to_string();
    let random = *uuid::Uuid::new_v4().as_bytes();
    let mut packetizer = Packetizer::new(
        u32::from_be_bytes(random[..4].try_into()?),
        u16::from_be_bytes(random[4..6].try_into()?),
    );
    let mut audio_packetizer = Packetizer::new(
        u32::from_be_bytes(random[8..12].try_into()?),
        u16::from_be_bytes(random[12..14].try_into()?),
    );
    audio_packetizer.payload_type = 97;
    let mut audio_channels = None;
    let mut audio_frames: Option<broadcast::Receiver<Arc<AudioFrame>>> = None;
    let mut stream_id: Option<String> = None;
    let mut feed: Option<Arc<Feed>> = None;
    let mut frames: Option<broadcast::Receiver<Arc<SharedFrame>>> = None;
    let mut channels = None;
    let mut waiting_idr = true;
    let mut report = tokio::time::interval(Duration::from_secs(5));
    report.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _=app.shutdown.cancelled()=>break,
            _=async {match &feed {Some(f)=>f.stopped.cancelled().await,None=>std::future::pending().await}}=>break,
            request=requests.recv()=>{
                let Some(request)=request else {break};
                if request.headers.contains_key("require") || request.headers.contains_key("proxy-require") {
                    send_response(&mut write,request.cseq,551,"Option Not Supported",&[],&[]).await?;continue;
                }
                if request.method=="OPTIONS" {
                    send_response(&mut write,request.cseq,200,"OK",&[("Public",PUBLIC.into())],&[]).await?;continue;
                }
                let Ok((id,base,track))=resource(&request.uri) else {
                    send_response(&mut write,request.cseq,400,"Bad Request",&[],&[]).await?;continue;
                };
                if !app.config.streams.contains_key(&id) {
                    send_response(&mut write,request.cseq,404,"Not Found",&[],&[]).await?;continue;
                }
                if stream_id.as_ref().is_some_and(|existing|existing!=&id) {
                    send_response(&mut write,request.cseq,455,"Method Not Valid In This State",&[],&[]).await?;continue;
                }
                match request.method.as_str() {
                    "DESCRIBE"=>{
                        if track {send_response(&mut write,request.cseq,460,"Only Aggregate Operation Allowed",&[],&[]).await?;continue;}
                        if feed.is_none() {
                            match app.hub.acquire(&id,&app.config,app.shutdown.clone()).await {
                                Ok(f)=>{feed=Some(f);stream_id=Some(id);},
                                Err(_)=>{send_response(&mut write,request.cseq,503,"Service Unavailable",&[],&[]).await?;continue;}
                            }
                        }
                        let video=feed.as_ref().unwrap().video();
                        let mut sdp=sdp(&video,&session_id)?;
                        if let Some(audio)=feed.as_ref().unwrap().audio(){sdp.push_str(&audio_sdp(&audio));}
                        send_response(&mut write,request.cseq,200,"OK",&[("Content-Type","application/sdp".into()),("Content-Base",base)],sdp.as_bytes()).await?;
                    }
                    "SETUP"=>{
                        if !track {send_response(&mut write,request.cseq,459,"Aggregate Operation Not Allowed",&[],&[]).await?;continue;}
                        if frames.is_some() || audio_frames.is_some() {send_response(&mut write,request.cseq,455,"Method Not Valid In This State",&[],&[]).await?;continue;}
                        let Some(pair)=request.headers.get("transport").and_then(|t|transport(t)) else {
                            send_response(&mut write,request.cseq,461,"Unsupported Transport",&[],&[]).await?;continue;
                        };
                        if (channels.is_some()||audio_channels.is_some()) && !valid_session(&request,&session_id) {
                            send_response(&mut write,request.cseq,454,"Session Not Found",&[],&[]).await?;continue;
                        }
                        if feed.is_none() {
                            match app.hub.acquire(&id,&app.config,app.shutdown.clone()).await {
                                Ok(f)=>{feed=Some(f);stream_id=Some(id);},
                                Err(_)=>{send_response(&mut write,request.cseq,503,"Service Unavailable",&[],&[]).await?;continue;}
                            }
                        }
                        let audio_track=request.uri.ends_with("/trackID=1");
                        if audio_track && feed.as_ref().unwrap().audio().is_none() {send_response(&mut write,request.cseq,404,"Not Found",&[],&[]).await?;continue;}
                        let other=if audio_track {channels}else{audio_channels};
                        if other.is_some_and(|other:(u8,u8)|other.0==pair.0||other.1==pair.0||other.0==pair.1||other.1==pair.1) {send_response(&mut write,request.cseq,461,"Unsupported Transport",&[],&[]).await?;continue;}
                        if audio_track {audio_channels=Some(pair);audio_packetizer.clock_rate=feed.as_ref().unwrap().audio().unwrap().clock_rate;}else{channels=Some(pair);}
                        send_response(&mut write,request.cseq,200,"OK",&[("Session",format!("{session_id};timeout=60")),("Transport",format!("RTP/AVP/TCP;unicast;interleaved={}-{};ssrc={:08X}",pair.0,pair.1,if audio_track {audio_packetizer.ssrc}else{packetizer.ssrc}))],&[]).await?;
                    }
                    "PLAY"|"GET_PARAMETER"|"TEARDOWN"=>{
                        if (channels.is_none()&&audio_channels.is_none())||!valid_session(&request,&session_id) {
                            send_response(&mut write,request.cseq,454,"Session Not Found",&[],&[]).await?;continue;
                        }
                        if track {send_response(&mut write,request.cseq,460,"Only Aggregate Operation Allowed",&[],&[]).await?;continue;}
                        if request.method=="PLAY" {
                            if request.headers.get("range").is_some_and(|v|!matches!(v.as_str(),"npt=0-"|"npt=0.000-"|"npt=now-")) {
                                send_response(&mut write,request.cseq,457,"Invalid Range",&[],&[]).await?;continue;
                            }
                            if frames.is_none() && channels.is_some() {frames=Some(feed.as_ref().unwrap().subscribe());waiting_idr=true;}
                            if audio_frames.is_none() && audio_channels.is_some() {audio_frames=Some(feed.as_ref().unwrap().subscribe_audio());}
                        } else if request.method=="GET_PARAMETER" && !request.body.is_empty() {
                            send_response(&mut write,request.cseq,451,"Parameter Not Understood",&[],&[]).await?;continue;
                        }
                        send_response(&mut write,request.cseq,200,"OK",&[("Session",session_id.clone())],&[]).await?;
                        if request.method=="TEARDOWN" {break;}
                    }
                    _=>{send_response(&mut write,request.cseq,405,"Method Not Allowed",&[("Allow",PUBLIC.into())],&[]).await?;}
                }
            },
            frame=async {match &mut frames {Some(rx)=>rx.recv().await,None=>std::future::pending().await}}=>{
                let frame=match frame {
                    Ok(frame)=>frame,
                    Err(broadcast::error::RecvError::Lagged(_))=>{waiting_idr=true;continue;},
                    Err(broadcast::error::RecvError::Closed)=>break,
                };
                if waiting_idr && !frame.keyframe {continue;}
                let wire=packetizer.frame(&frame,channels.unwrap().0)?;
                bounded_write(&mut write,&wire).await?;
                waiting_idr=false;
            },
            audio=async {match &mut audio_frames {Some(rx)=>rx.recv().await,None=>std::future::pending().await}}=>{
                let frame=match audio {Ok(frame)=>frame,Err(broadcast::error::RecvError::Lagged(_))=>continue,Err(_)=>break};
                if channels.is_some() && waiting_idr {continue;}
                let config=feed.as_ref().unwrap().audio().unwrap();
                let wire=audio_packetizer.audio(&config,&frame,audio_channels.unwrap().0)?;
                bounded_write(&mut write,&wire).await?;
            },
            _=report.tick(), if (frames.is_some() && !waiting_idr) || audio_packetizer.last.is_some()=>{
                if let Some(pair)=channels {if packetizer.last.is_some(){bounded_write(&mut write,&packetizer.report(pair.1)?).await?;}}
                if let Some(pair)=audio_channels {if audio_packetizer.last.is_some(){bounded_write(&mut write,&audio_packetizer.report(pair.1)?).await?;}}
            }
        }
    }
    Ok(())
}

fn valid_session(request: &Request, expected: &str) -> bool {
    request
        .headers
        .get("session")
        .is_some_and(|s| s.split(';').next().unwrap_or("").trim() == expected)
}
fn resource(uri: &str) -> Result<(String, String, bool)> {
    let mut url = url::Url::parse(uri).context("invalid RTSP URI")?;
    ensure!(
        url.scheme() == "rtsp"
            && url.host_str().is_some()
            && url.query().is_none()
            && url.fragment().is_none(),
        "invalid RTSP URI"
    );
    let parts: Vec<_> = url.path().trim_matches('/').split('/').collect();
    ensure!(
        (1..=2).contains(&parts.len()) && crate::config::valid_id(parts[0]),
        "invalid stream"
    );
    let track = parts.len() == 2;
    ensure!(
        !track || matches!(parts[1], "trackID=0" | "trackID=1"),
        "unknown track"
    );
    let id = parts[0].to_owned();
    url.set_username("")
        .map_err(|_| anyhow::anyhow!("invalid URI"))?;
    url.set_password(None)
        .map_err(|_| anyhow::anyhow!("invalid URI"))?;
    url.set_path(&format!("/{id}/"));
    Ok((id, url.to_string(), track))
}
fn transport(value: &str) -> Option<(u8, u8)> {
    for alternative in value.split(',') {
        let mut fields = alternative.split(';').map(str::trim);
        if !fields.next()?.eq_ignore_ascii_case("RTP/AVP/TCP") {
            continue;
        }
        let mut pair = None;
        let mut multicast = false;
        let mut invalid = false;
        for field in fields {
            if field.eq_ignore_ascii_case("multicast") {
                multicast = true;
            }
            if let Some((key, value)) = field.split_once('=') {
                if key.eq_ignore_ascii_case("interleaved") {
                    if pair.is_some() {
                        invalid = true;
                        break;
                    }
                    pair = value
                        .split_once('-')
                        .and_then(|(a, b)| Some((a.parse::<u8>().ok()?, b.parse::<u8>().ok()?)));
                } else if key.eq_ignore_ascii_case("mode")
                    && !value.trim_matches('"').eq_ignore_ascii_case("PLAY")
                {
                    invalid = true;
                }
            }
        }
        if !multicast && !invalid {
            if let Some((a, b)) = pair {
                if a != b {
                    return Some((a, b));
                }
            }
        }
    }
    None
}
fn sdp(video: &VideoConfig, session: &str) -> Result<String> {
    ensure!(
        video.sps.len() >= 4 && !video.pps.is_empty(),
        "camera has no H264 configuration"
    );
    let profile = format!(
        "{:02X}{:02X}{:02X}",
        video.sps[1], video.sps[2], video.sps[3]
    );
    let id = u64::from_str_radix(&session[..16], 16)?;
    Ok(format!("v=0\r\no=- {id} 1 IN IP4 127.0.0.1\r\ns=UVMS H264 relay\r\nc=IN IP4 0.0.0.0\r\nt=0 0\r\na=control:*\r\na=range:npt=now-\r\nm=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\na=fmtp:96 packetization-mode=1;profile-level-id={profile};sprop-parameter-sets={},{}\r\na=control:trackID=0\r\n",STANDARD.encode(&video.sps),STANDARD.encode(&video.pps)))
}

fn audio_sdp(audio: &AudioConfig) -> String {
    let codec = match audio.codec {
        AudioCodec::Aac => "MPEG4-GENERIC",
        AudioCodec::Pcmu => "PCMU",
        AudioCodec::Pcma => "PCMA",
        AudioCodec::Opus => "opus",
    };
    let mut s = format!(
        "m=audio 0 RTP/AVP 97\r\na=rtpmap:97 {codec}/{}/{}\r\n",
        audio.clock_rate, audio.channels
    );
    if audio.codec == AudioCodec::Aac {
        let config = audio
            .config
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        s.push_str(&format!("a=fmtp:97 streamtype=5;profile-level-id=1;mode=AAC-hbr;config={config};sizeLength=13;indexLength=3;indexDeltaLength=3\r\n"));
    }
    s.push_str("a=control:trackID=1\r\n");
    s
}

async fn read_request(read: &mut OwnedReadHalf, wire: &mut Vec<u8>) -> Result<Request> {
    'request: loop {
        if wire.first() == Some(&b'$') {
            while wire.len() < 4 {
                read_more(read, wire).await?;
            }
            let length = u16::from_be_bytes([wire[2], wire[3]]) as usize;
            ensure!(length <= 8192, "client interleaved packet too large");
            while wire.len() < 4 + length {
                read_more(read, wire).await?;
            }
            wire.drain(..4 + length); // consume RR/SDES without reflecting or logging data
            continue;
        }
        let end = loop {
            if wire.first() == Some(&b'$') {
                continue 'request;
            }
            if let Some(at) = wire.windows(4).position(|w| w == b"\r\n\r\n") {
                ensure!(at + 4 <= HEADER_LIMIT, "header too large");
                break at + 4;
            }
            ensure!(wire.len() < HEADER_LIMIT, "header too large");
            read_more(read, wire).await?;
        };
        let text = std::str::from_utf8(&wire[..end])?;
        let mut lines = text.split("\r\n");
        let start: Vec<_> = lines.next().unwrap_or("").split_whitespace().collect();
        ensure!(
            start.len() == 3 && start[2] == "RTSP/1.0" && start[0].len() <= 32,
            "invalid request line"
        );
        let method = start[0].to_owned();
        let uri = start[1].to_owned();
        let mut headers = HashMap::new();
        for line in lines.filter(|line| !line.is_empty()) {
            let (key, value) = line.split_once(':').context("invalid header")?;
            ensure!(
                !key.is_empty() && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
                "invalid header name"
            );
            ensure!(
                headers
                    .insert(key.to_ascii_lowercase(), value.trim().to_owned())
                    .is_none(),
                "duplicate header"
            );
        }
        let cseq = headers
            .get("cseq")
            .context("missing CSeq")?
            .parse::<u32>()?;
        let length = headers
            .get("content-length")
            .map(|v| v.parse::<usize>())
            .transpose()?
            .unwrap_or(0);
        ensure!(length <= BODY_LIMIT, "body too large");
        while wire.len() < end + length {
            read_more(read, wire).await?;
        }
        let body = wire[end..end + length].to_vec();
        wire.drain(..end + length);
        return Ok(Request {
            method,
            uri,
            cseq,
            headers,
            body,
        });
    }
}
async fn read_more(read: &mut OwnedReadHalf, wire: &mut Vec<u8>) -> Result<()> {
    ensure!(
        wire.len() < HEADER_LIMIT + BODY_LIMIT + 8192,
        "read buffer too large"
    );
    let mut buffer = [0u8; 4096];
    let count = read.read(&mut buffer).await?;
    ensure!(count != 0, "client disconnected");
    wire.extend_from_slice(&buffer[..count]);
    Ok(())
}
async fn bounded_write(write: &mut tokio::net::tcp::OwnedWriteHalf, bytes: &[u8]) -> Result<()> {
    timeout(Duration::from_secs(3), write.write_all(bytes))
        .await
        .context("RTSP viewer write timeout")??;
    Ok(())
}
async fn send_response(
    write: &mut tokio::net::tcp::OwnedWriteHalf,
    cseq: u32,
    status: u16,
    reason: &str,
    headers: &[(&str, String)],
    body: &[u8],
) -> Result<()> {
    let mut response = format!(
        "RTSP/1.0 {status} {reason}\r\nCSeq: {cseq}\r\nServer: UVMS-Rust\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (name, value) in headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str("\r\n");
    let mut response = response.into_bytes();
    response.extend_from_slice(body);
    bounded_write(write, &response).await
}

struct Packetizer {
    payload_type: u8,
    clock_rate: u32,
    ssrc: u32,
    sequence: u16,
    packets: u32,
    octets: u32,
    last: Option<(u32, Instant)>,
}
impl Packetizer {
    fn new(ssrc: u32, sequence: u16) -> Self {
        Self {
            payload_type: 96,
            clock_rate: 90000,
            ssrc,
            sequence,
            packets: 0,
            octets: 0,
            last: None,
        }
    }
    fn packet(
        &mut self,
        wire: &mut Vec<u8>,
        prefix: &[u8],
        payload: &[u8],
        timestamp: u32,
        marker: bool,
        channel: u8,
    ) {
        let length = 12 + prefix.len() + payload.len();
        wire.extend_from_slice(&[b'$', channel]);
        wire.extend_from_slice(&(length as u16).to_be_bytes());
        wire.extend_from_slice(&[0x80, self.payload_type | if marker { 0x80 } else { 0 }]);
        wire.extend_from_slice(&self.sequence.to_be_bytes());
        wire.extend_from_slice(&timestamp.to_be_bytes());
        wire.extend_from_slice(&self.ssrc.to_be_bytes());
        wire.extend_from_slice(prefix);
        wire.extend_from_slice(payload);
        self.sequence = self.sequence.wrapping_add(1);
        self.packets = self.packets.wrapping_add(1);
        self.octets = self
            .octets
            .wrapping_add((prefix.len() + payload.len()) as u32);
    }
    fn frame(&mut self, frame: &Frame, channel: u8) -> Result<Vec<u8>> {
        ensure!(
            !frame.avcc.is_empty() && frame.avcc.len() <= 32 * 1024 * 1024,
            "invalid relay frame size"
        );
        let mut nals = Vec::new();
        let mut cursor = 0;
        while cursor < frame.avcc.len() {
            let prefix = frame
                .avcc
                .get(cursor..cursor + 4)
                .context("truncated AVCC")?;
            let length = u32::from_be_bytes(prefix.try_into()?) as usize;
            cursor += 4;
            ensure!(
                length > 0 && length <= frame.avcc.len() - cursor,
                "invalid NAL length"
            );
            let nal = &frame.avcc[cursor..cursor + length];
            ensure!(
                nal[0] & 0x80 == 0 && (1..=23).contains(&(nal[0] & 31)),
                "invalid H264 NAL"
            );
            nals.push(nal);
            cursor += length;
        }
        let mut wire = Vec::with_capacity(frame.avcc.len() + 1024);
        for (index, nal) in nals.iter().enumerate() {
            let last = index + 1 == nals.len();
            if nal.len() <= 1200 {
                self.packet(
                    &mut wire,
                    &[],
                    nal,
                    frame.timestamp_90k as u32,
                    last,
                    channel,
                );
            } else {
                let chunks = nal[1..].chunks(1198);
                let count = chunks.len();
                for (part, chunk) in chunks.enumerate() {
                    let prefix = [
                        (nal[0] & 0xe0) | 28,
                        (nal[0] & 31)
                            | if part == 0 { 0x80 } else { 0 }
                            | if part + 1 == count { 0x40 } else { 0 },
                    ];
                    self.packet(
                        &mut wire,
                        &prefix,
                        chunk,
                        frame.timestamp_90k as u32,
                        last && part + 1 == count,
                        channel,
                    );
                }
            }
        }
        self.last = Some((frame.timestamp_90k as u32, Instant::now()));
        Ok(wire)
    }
    fn audio(&mut self, config: &AudioConfig, frame: &AudioFrame, channel: u8) -> Result<Vec<u8>> {
        ensure!(
            !frame.data.is_empty() && frame.data.len() <= 8191,
            "Invalid relay audio size"
        );
        let mut prefix = Vec::new();
        if config.codec == AudioCodec::Aac {
            prefix.extend_from_slice(&16u16.to_be_bytes());
            prefix.extend_from_slice(&((frame.data.len() as u16) << 3).to_be_bytes());
        }
        let timestamp = (frame.timestamp_90k * u64::from(config.clock_rate) / 90000) as u32;
        let mut wire = Vec::with_capacity(frame.data.len() + 20);
        self.packet(&mut wire, &prefix, &frame.data, timestamp, true, channel);
        self.last = Some((timestamp, Instant::now()));
        Ok(wire)
    }
    fn report(&self, channel: u8) -> Result<Vec<u8>> {
        let Some((rtp, at)) = self.last else {
            bail!("no RTP clock");
        };
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?;
        let ntp_secs = (now.as_secs() + 2_208_988_800) as u32;
        let ntp_frac = ((now.subsec_nanos() as u64) << 32) / 1_000_000_000;
        let extrapolated = rtp.wrapping_add(
            (at.elapsed().as_nanos() * u128::from(self.clock_rate) / 1_000_000_000) as u32,
        );
        let mut compound = vec![0x80, 200, 0, 6];
        for value in [
            self.ssrc,
            ntp_secs,
            ntp_frac as u32,
            extrapolated,
            self.packets,
            self.octets,
        ] {
            compound.extend_from_slice(&value.to_be_bytes());
        }
        let name = format!("{:08x}@uvms", self.ssrc);
        let mut sdes = self.ssrc.to_be_bytes().to_vec();
        sdes.extend_from_slice(&[1, name.len() as u8]);
        sdes.extend_from_slice(name.as_bytes());
        sdes.push(0);
        while !sdes.len().is_multiple_of(4) {
            sdes.push(0);
        }
        compound.extend_from_slice(&[0x81, 202]);
        compound.extend_from_slice(&((sdes.len() / 4) as u16).to_be_bytes());
        compound.extend_from_slice(&sdes);
        let mut wire = vec![b'$', channel];
        wire.extend_from_slice(&(compound.len() as u16).to_be_bytes());
        wire.extend_from_slice(&compound);
        Ok(wire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn rtcp_before_fragmented_requests_is_consumed_and_pipeline_preserved() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let sender = tokio::spawn(async move {
            let mut socket = TcpStream::connect(address).await.unwrap();
            socket
                .write_all(&[b'$', 1, 0, 4, 0x80, 201, 0, 0])
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(10)).await;
            socket.write_all(b"GET_PARAMETER rtsp://localhost/camera RTSP/1.0\r\nCSeq: 7\r\nContent-Length: 3\r\n\r\nabcOPTIONS * RTSP/1.0\r\nCSeq: 8\r\n\r\n").await.unwrap();
        });
        let (socket, _) = listener.accept().await.unwrap();
        let (mut read, _) = socket.into_split();
        let mut wire = Vec::new();
        let first = read_request(&mut read, &mut wire).await.unwrap();
        assert_eq!((first.cseq, first.body), (7, b"abc".to_vec()));
        let second = read_request(&mut read, &mut wire).await.unwrap();
        assert_eq!((second.cseq, second.method), (8, "OPTIONS".into()));
        sender.await.unwrap();
    }
    #[test]
    fn transport_is_explicit() {
        assert_eq!(transport("RTP/AVP;unicast;client_port=5000-5001"), None);
        assert_eq!(
            transport("RTP/AVP/TCP;unicast;interleaved=4-5"),
            Some((4, 5))
        );
        assert_eq!(transport("RTP/AVP/TCP;multicast;interleaved=0-1"), None);
        assert_eq!(transport("RTP/AVP/TCP;unicast;interleaved=1-1"), None);
        assert_eq!(
            resource("rtsp://uvms:secret@localhost:8555/cam/trackID=0").unwrap(),
            ("cam".into(), "rtsp://localhost:8555/cam/".into(), true)
        );
    }
    #[test]
    fn audio_relay_preserves_aac_payload_and_uses_audio_clock() {
        let config = AudioConfig {
            codec: AudioCodec::Aac,
            clock_rate: 48000,
            channels: 2,
            config: vec![0x11, 0x90],
        };
        let mut packetizer = Packetizer::new(123, 65535);
        packetizer.payload_type = 97;
        packetizer.clock_rate = 48000;
        let frame = AudioFrame {
            data: vec![1, 2, 3, 4],
            timestamp: 0,
            duration: 1024,
            timestamp_90k: 90000,
        };
        let wire = packetizer.audio(&config, &frame, 6).unwrap();
        assert_eq!(&wire[..2], &[b'$', 6]);
        assert_eq!(wire[5] & 127, 97);
        assert_eq!(u32::from_be_bytes(wire[8..12].try_into().unwrap()), 48000);
        assert_eq!(&wire[16..], &[0, 16, 0, 32, 1, 2, 3, 4]);
        assert_eq!(packetizer.sequence, 0);
        let sdp = audio_sdp(&config);
        assert!(sdp.contains("MPEG4-GENERIC/48000/2"));
        assert!(sdp.contains("config=1190;"));
        assert!(resource("rtsp://localhost/camera/trackID=1").is_ok());
        assert!(resource("rtsp://localhost/camera/trackID=2").is_err());
    }
    #[test]
    fn packetization_reassembles_single_nal_and_fu_a_exactly() {
        let nals = [vec![0x67, 0x42, 0xc0, 0x16], {
            let mut n = vec![0x65];
            n.extend((0..5000).map(|v| (v % 251) as u8));
            n
        }];
        let mut avcc = Vec::new();
        for nal in &nals {
            avcc.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            avcc.extend_from_slice(nal);
        }
        let mut p = Packetizer::new(0x12345678, u16::MAX - 1);
        let wire = p
            .frame(
                &Frame {
                    avcc,
                    timestamp_90k: 0x123456789,
                    duration_90k: 6000,
                    keyframe: true,
                },
                4,
            )
            .unwrap();
        let mut cursor = 0;
        let mut result = Vec::new();
        let mut partial = Vec::new();
        let mut markers = 0;
        let mut expected_sequence = u16::MAX - 1;
        while cursor < wire.len() {
            assert_eq!(&wire[cursor..cursor + 2], &[b'$', 4]);
            let size =
                u16::from_be_bytes(wire[cursor + 2..cursor + 4].try_into().unwrap()) as usize;
            let rtp = &wire[cursor + 4..cursor + 4 + size];
            assert_eq!(
                u16::from_be_bytes(rtp[2..4].try_into().unwrap()),
                expected_sequence
            );
            expected_sequence = expected_sequence.wrapping_add(1);
            assert_eq!(
                u32::from_be_bytes(rtp[4..8].try_into().unwrap()),
                0x23456789
            );
            assert!(size <= 1212);
            let data = &rtp[12..];
            if data[0] & 31 == 28 {
                if data[1] & 0x80 != 0 {
                    partial = vec![(data[0] & 0xe0) | (data[1] & 31)];
                }
                partial.extend_from_slice(&data[2..]);
                if data[1] & 0x40 != 0 {
                    result.push(std::mem::take(&mut partial));
                }
            } else {
                result.push(data.to_vec());
            }
            if rtp[1] & 0x80 != 0 {
                markers += 1;
                assert_eq!(cursor + size + 4, wire.len());
            }
            cursor += size + 4;
        }
        assert_eq!(result, nals);
        assert_eq!(markers, 1);
        let report = p.report(5).unwrap();
        assert_eq!(&report[4..8], &[0x80, 200, 0, 6]);
        assert_eq!(&report[32..34], &[0x81, 202]);
    }
}
