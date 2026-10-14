//! Isolated local transport load: real ICE/DTLS/SRTP, no video decoding.
//! Packet-contiguous RTP marker frames are NOT proof of browser playback.
use anyhow::{bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering::Relaxed},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use webrtc::{
    api::{
        interceptor_registry::register_default_interceptors, media_engine::MediaEngine, APIBuilder,
    },
    ice_transport::ice_candidate::RTCIceCandidateInit,
    interceptor::registry::Registry,
    peer_connection::{
        configuration::RTCConfiguration, peer_connection_state::RTCPeerConnectionState,
        sdp::session_description::RTCSessionDescription, RTCPeerConnection,
    },
    rtp_transceiver::{
        rtp_codec::RTPCodecType, rtp_transceiver_direction::RTCRtpTransceiverDirection,
        RTCRtpTransceiverInit,
    },
};

#[derive(Default)]
struct Counters {
    packets: AtomicU64,
    bytes: AtomicU64,
    frames: AtomicU64,
    gaps: AtomicU64,
}
impl Counters {
    fn read(&self) -> [u64; 4] {
        [
            self.packets.load(Relaxed),
            self.bytes.load(Relaxed),
            self.frames.load(Relaxed),
            self.gaps.load(Relaxed),
        ]
    }
}
struct Viewer {
    peer: Arc<RTCPeerConnection>,
    counts: Arc<Counters>,
}
async fn connect() -> Result<Viewer> {
    let mut media = MediaEngine::default();
    media.register_default_codecs()?;
    let registry = register_default_interceptors(Registry::new(), &mut media)?;
    let api = APIBuilder::new()
        .with_media_engine(media)
        .with_interceptor_registry(registry)
        .build();
    let peer = Arc::new(api.new_peer_connection(RTCConfiguration::default()).await?);
    let counts = Arc::new(Counters::default());
    let counter = counts.clone();
    peer.on_track(Box::new(move |track, _, _| {
        let counter = counter.clone();
        Box::pin(async move {
            if track.kind() != RTPCodecType::Video {
                return;
            }
            tokio::spawn(async move {
                let mut buffer = vec![0u8; 2048];
                let mut previous = None;
                let mut timestamp = None;
                let mut valid = false;
                while let Ok((packet, _)) = track.read(&mut buffer).await {
                    let sequence = packet.header.sequence_number;
                    let gap = previous.is_some_and(|p: u16| p.wrapping_add(1) != sequence);
                    if timestamp != Some(packet.header.timestamp) {
                        timestamp = Some(packet.header.timestamp);
                        valid = previous.is_some();
                    }
                    if gap {
                        counter.gaps.fetch_add(1, Relaxed);
                        valid = false;
                    }
                    counter.packets.fetch_add(1, Relaxed);
                    counter
                        .bytes
                        .fetch_add(packet.payload.len() as u64, Relaxed);
                    if packet.header.marker && valid {
                        counter.frames.fetch_add(1, Relaxed);
                    }
                    previous = Some(sequence);
                }
            });
        })
    }));
    peer.add_transceiver_from_kind(
        RTPCodecType::Video,
        Some(RTCRtpTransceiverInit {
            direction: RTCRtpTransceiverDirection::Recvonly,
            send_encodings: vec![],
        }),
    )
    .await?;
    let negotiate = async {
        let (mut ws, _) =
            connect_async("ws://127.0.0.1:8091/webrtc_playnow/singleport/udp/camera").await?;
        let greeting = ws.next().await.context("missing greeting")??.into_text()?;
        if !greeting.starts_with("H264|-|-|") {
            bail!("unexpected codec greeting");
        }
        let offer = peer.create_offer(None).await?;
        let mut gathering = peer.gathering_complete_promise().await;
        peer.set_local_description(offer).await?;
        gathering.recv().await;
        ws.send(Message::Text(
            serde_json::to_string(&peer.local_description().await.context("missing offer")?)?
                .into(),
        ))
        .await?;
        let answer = ws.next().await.context("missing answer")??.into_text()?;
        let mut parts = answer.split("|-|-|");
        peer.set_remote_description(serde_json::from_str::<RTCSessionDescription>(
            parts.next().context("missing answer SDP")?,
        )?)
        .await?;
        if let Some(candidate) = parts.next() {
            peer.add_ice_candidate(serde_json::from_str::<RTCIceCandidateInit>(candidate)?)
                .await?;
        }
        let _ = ws.close(None).await;
        while counts.frames.load(Relaxed) < 10 {
            if peer.connection_state() == RTCPeerConnectionState::Failed {
                bail!("ICE failed");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok::<_, anyhow::Error>(())
    };
    let result = match tokio::time::timeout(Duration::from_secs(35), negotiate).await {
        Ok(result) => result,
        Err(error) => Err(error.into()),
    };
    if let Err(error) = result {
        let _ = peer.close().await;
        return Err(error);
    }
    Ok(Viewer { peer, counts })
}
fn process(pid: u32) -> Result<(u64, f64)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let fields: Vec<_> = stat
        .rsplit_once(')')
        .context("invalid proc stat")?
        .1
        .split_whitespace()
        .collect();
    let ticks = fields[11].parse::<u64>()? + fields[12].parse::<u64>()?;
    let status = std::fs::read_to_string(format!("/proc/{pid}/status"))?;
    let rss = status
        .lines()
        .find(|l| l.starts_with("VmRSS:"))
        .context("no RSS")?
        .split_whitespace()
        .nth(1)
        .context("no RSS size")?
        .parse::<f64>()?
        / 1024.;
    Ok((ticks, rss))
}
#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    let counts = std::env::var("UVMS_CAP_COUNTS")
        .unwrap_or("32,64,128,256".into())
        .split(',')
        .map(str::parse::<usize>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let seconds = std::env::var("UVMS_CAP_SECONDS")
        .unwrap_or("60".into())
        .parse::<u64>()?;
    let ramp = std::env::var("UVMS_CAP_RAMP")
        .unwrap_or("4".into())
        .parse::<usize>()?;
    if counts.is_empty()
        || counts.iter().any(|n| *n == 0 || *n > 1024)
        || counts.windows(2).any(|w| w[0] >= w[1])
        || !(10..=300).contains(&seconds)
        || !(1..=32).contains(&ramp)
    {
        bail!("invalid bounded parameters");
    }
    let output =
        std::env::var("UVMS_CAP_OUTPUT").unwrap_or("test-results/capacity/transport.json".into());
    let pid = String::from_utf8(
        std::process::Command::new("systemctl")
            .args(["show", "uvms-rust-opt-bench", "-p", "MainPID", "--value"])
            .output()?
            .stdout,
    )?
    .trim()
    .parse::<u32>()?;
    let hz = String::from_utf8(
        std::process::Command::new("getconf")
            .arg("CLK_TCK")
            .output()?
            .stdout,
    )?
    .trim()
    .parse::<f64>()?;
    let mut viewers = vec![];
    let mut stages: Vec<Value> = vec![];
    for target in counts {
        while viewers.len() < target {
            let n = (target - viewers.len()).min(ramp);
            let tasks = (0..n).map(|_| tokio::spawn(connect())).collect::<Vec<_>>();
            for task in tasks {
                viewers.push(task.await??);
            }
            if viewers.len() % 32 == 0 {
                println!("{}", json!({"connected":viewers.len(),"target":target}));
            }
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
        let before = viewers.iter().map(|v| v.counts.read()).collect::<Vec<_>>();
        let server_start = process(pid)?;
        let client_start = process(std::process::id())?;
        let start = Instant::now();
        let mut server_peak = server_start.1;
        let mut client_peak = client_start.1;
        let mut stalled = 0;
        let mut previous = before.clone();
        let mut intervals = vec![];
        for second in 1..=seconds {
            tokio::time::sleep(Duration::from_secs(1)).await;
            server_peak = server_peak.max(process(pid)?.1);
            client_peak = client_peak.max(process(std::process::id())?.1);
            if second % 10 == 0 || second == seconds {
                let now = viewers.iter().map(|v| v.counts.read()).collect::<Vec<_>>();
                let count = now
                    .iter()
                    .zip(&previous)
                    .filter(|(a, b)| a[2] == b[2])
                    .count();
                stalled += count;
                intervals.push(json!({"elapsed":start.elapsed().as_secs_f64(),"stalled":count}));
                previous = now;
            }
        }
        let elapsed = start.elapsed().as_secs_f64();
        let server_end = process(pid)?;
        let client_end = process(std::process::id())?;
        let mut clients = vec![];
        let mut passed = stalled == 0;
        let mut total_mbps = 0f64;
        let mut min_fps = f64::INFINITY;
        let mut gaps = 0;
        for (viewer, b) in viewers.iter().zip(&before) {
            let a = viewer.counts.read();
            let fps = (a[2] - b[2]) as f64 / elapsed;
            let mbps = 8. * (a[1] - b[1]) as f64 / elapsed / 1e6;
            let connected = viewer.peer.connection_state() == RTCPeerConnectionState::Connected;
            passed &= connected && fps >= 13. && (a[3] - b[3]) == 0;
            total_mbps += mbps;
            min_fps = min_fps.min(fps);
            gaps += a[3] - b[3];
            clients.push(json!({"connected":connected,"fps":fps,"mbps":mbps,"sequence_discontinuities":a[3]-b[3],"packets":a[0]-b[0]}));
        }
        let stage = json!({"viewers":target,"passed":passed,"seconds":elapsed,"min_complete_rtp_fps":min_fps,"total_mbps":total_mbps,"sequence_discontinuities":gaps,"server_cpu_one_core_percent":100.*(server_end.0-server_start.0) as f64/hz/elapsed,"server_peak_rss_mib":server_peak,"receiver_cpu_one_core_percent":100.*(client_end.0-client_start.0) as f64/hz/elapsed,"receiver_peak_rss_mib":client_peak,"stalled_intervals":stalled,"intervals":intervals,"clients":clients});
        let mut summary = stage.clone();
        summary.as_object_mut().unwrap().remove("clients");
        println!("{summary}");
        stages.push(stage);
        if let Some(parent) = std::path::Path::new(&output).parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(
            &output,
            serde_json::to_vec_pretty(
                &json!({"scope":"Local WSL ICE/DTLS/SRTP receive, packet-contiguous marker frames; no decoder, browser or physical NIC validation","stages":stages}),
            )?,
        )?;
        if !passed {
            break;
        }
    }
    for viewer in viewers {
        let _ = viewer.peer.close().await;
    }
    if stages.iter().any(|stage| stage["passed"] != true) {
        bail!("transport quality threshold failed; see saved report");
    }
    Ok(())
}
