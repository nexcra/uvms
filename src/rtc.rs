//! Original playback signaling and encoded H.264 handoff, with Rust WebRTC transport.
//! Native evidence and the independent original-DLL oracle are documented in
//! docs/original-webrtc-native.md. This module never decodes or encodes video.
use anyhow::{bail, Context, Result};
use axum::extract::ws::{Message, WebSocket};
use bytes::Bytes;
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio::sync::{broadcast, watch, OwnedSemaphorePermit};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use unreal_core_port::frame::{normalize, Metadata};
use unreal_rtsp::{AudioCodec, AudioConfig, Frame, VideoConfig};
use webrtc::{
    api::{
        interceptor_registry::register_default_interceptors,
        media_engine::{MediaEngine, MIME_TYPE_H264},
        setting_engine::SettingEngine,
        APIBuilder,
    },
    ice::{
        network_type::NetworkType,
        udp_mux::{UDPMux, UDPMuxDefault, UDPMuxParams},
        udp_network::UDPNetwork,
    },
    ice_transport::{ice_candidate_type::RTCIceCandidateType, ice_server::RTCIceServer},
    interceptor::registry::Registry,
    peer_connection::{
        configuration::RTCConfiguration,
        peer_connection_state::RTCPeerConnectionState,
        sdp::{sdp_type::RTCSdpType, session_description::RTCSessionDescription},
        RTCPeerConnection,
    },
    rtp::{header::Header, packet::Packet},
    rtp_transceiver::rtp_codec::{RTCRtpCodecCapability, RTCRtpCodecParameters, RTPCodecType},
    track::track_local::{
        track_local_static_rtp::TrackLocalStaticRTP, TrackLocal, TrackLocalWriter,
    },
};

use crate::{
    config::Config,
    ingest::{Feed, SharedFrame},
    rtp_video::{Packetizer, PreparedFrame},
    App,
};

// Service-owned mux: closing a peer removes its ufrag, not the shared socket.
pub(crate) async fn shared_udp(app: &App) -> Result<Arc<UDPMuxDefault>> {
    app.rtc_udp
        .get_or_try_init(|| async {
            let socket = tokio::net::UdpSocket::bind((
                std::net::Ipv4Addr::UNSPECIFIED,
                app.config.ice_udp_min,
            ))
            .await?;
            Ok::<_, anyhow::Error>(UDPMuxDefault::new(UDPMuxParams::new(socket)))
        })
        .await
        .cloned()
}

pub(crate) async fn close_udp(app: &App) {
    if let Some(udp) = app.rtc_udp.get() {
        let _ = udp.close().await;
    }
}

// Leaves room for RTP, negotiated extensions, SRTP and IP headers below a usual MTU.
const RTP_PAYLOAD_LIMIT: usize = 1100;
const SIGNAL_TIMEOUT: Duration = Duration::from_secs(12);

pub async fn serve(
    mut socket: WebSocket,
    feed: Arc<Feed>,
    config: Arc<Config>,
    _permit: OwnedSemaphorePermit,
    shutdown: CancellationToken,
    udp: Arc<UDPMuxDefault>,
) {
    let capability = match h264(&feed.video()) {
        Ok(value) => value,
        Err(_) => {
            signal_error(&mut socket, "Error: Unsupported H264 camera configuration").await;
            return;
        }
    };
    let peer = match make_peer(
        &config,
        &capability,
        feed.audio().as_ref().map(crate::audio::rtc_config).as_ref(),
        udp,
    )
    .await
    {
        Ok(peer) => Arc::new(peer),
        Err(_) => {
            signal_error(
                &mut socket,
                "Error: Initialization of peer connection failed",
            )
            .await;
            return;
        }
    };
    if let Err(error) = run(socket, &peer, feed, capability, shutdown).await {
        // Do not log SDP or source URLs. Context labels are controlled by this module.
        tracing::debug!(reason = %error, "WebRTC playback session ended");
    }
    let _ = tokio::time::timeout(Duration::from_secs(3), peer.close()).await;
}

async fn run(
    mut socket: WebSocket,
    peer: &Arc<RTCPeerConnection>,
    feed: Arc<Feed>,
    capability: RTCRtpCodecCapability,
    shutdown: CancellationToken,
) -> Result<()> {
    // Native codec greeting makes the original SDK request the audio transceiver.
    let audio_codec = feed
        .audio()
        .map(|c| crate::audio::rtc_config(&c))
        .map(|c| match c.codec {
            AudioCodec::Opus => "opus",
            AudioCodec::Pcmu => "PCMU",
            AudioCodec::Pcma => "PCMA",
            AudioCodec::Aac => unreachable!(),
        })
        .unwrap_or("");
    send_text(&mut socket, format!("H264|-|-|{audio_codec}")).await?;
    let offer = tokio::select! {
        _ = shutdown.cancelled() => return Ok(()),
        _ = feed.stopped.cancelled() => return Ok(()),
        message = tokio::time::timeout(SIGNAL_TIMEOUT, socket.recv()) => {
            match message {
                Ok(Some(Ok(Message::Text(text)))) => text,
                _ => bail!("offer not received before deadline"),
            }
        }
    };
    let offer = match serde_json::from_str::<RTCSessionDescription>(&offer) {
        Ok(offer) if offer.sdp_type == RTCSdpType::Offer => offer,
        _ => {
            signal_error(&mut socket, "Error: Invalid offer").await;
            return Ok(());
        }
    };
    let track = Arc::new(TrackLocalStaticRTP::new(
        capability,
        "video_label".into(),
        "stream_label".into(),
    ));
    let sender = peer
        .add_track(track.clone() as Arc<dyn TrackLocal + Send + Sync>)
        .await
        .map_err(|_| anyhow::anyhow!("cannot create H264 track"))?;
    let audio_config = feed.audio().map(|c| crate::audio::rtc_config(&c));
    let audio_track = audio_config.as_ref().map(|c| {
        Arc::new(TrackLocalStaticRTP::new(
            audio_capability(c),
            "audio_label".into(),
            "stream_label".into(),
        ))
    });
    let audio_sender = if let Some(track) = &audio_track {
        Some(
            peer.add_track(track.clone() as Arc<dyn TrackLocal + Send + Sync>)
                .await?,
        )
    } else {
        None
    };
    let mut audio_frames = feed.subscribe_rtc_audio();
    let mut audio_sequence = 0u16;
    let mut audio_rtcp_buffer = vec![0; 2048];
    let (states, mut state) = watch::channel(RTCPeerConnectionState::New);
    peer.on_peer_connection_state_change(Box::new(move |value| {
        states.send_replace(value);
        Box::pin(async {})
    }));
    let handshake = async {
        peer.set_remote_description(offer).await?;
        let answer = peer.create_answer(None).await?;
        let mut gathering = peer.gathering_complete_promise().await;
        peer.set_local_description(answer).await?;
        gathering.recv().await;
        let local = peer
            .local_description()
            .await
            .context("missing local answer")?;
        original_answer(&local)
    };
    let answer = tokio::select! {
        _ = shutdown.cancelled() => return Ok(()),
        _ = feed.stopped.cancelled() => return Ok(()),
        result = tokio::time::timeout(SIGNAL_TIMEOUT, handshake) => match result {
            Ok(Ok(answer)) => answer,
            _ => {
                signal_error(&mut socket, "Error: Initialization of peer connection failed").await;
                return Ok(());
            }
        }
    };
    send_text(&mut socket, answer).await?;
    // The original SDK intentionally closes WS here. Its lifetime is unrelated
    // to media playback; retain feed and session permit through ICE termination.
    let _ = tokio::time::timeout(Duration::from_secs(2), socket.send(Message::Close(None))).await;
    drop(socket);

    let mut frames = feed.subscribe();
    let mut rtcp_buffer = vec![0; 2048];
    let mut payloader = Packetizer::default();
    let mut sequence = 0u16;
    let mut waiting_for_idr = true;
    let mut connected = peer.connection_state() == RTCPeerConnectionState::Connected;
    let mut ever_connected = connected;
    let started = Instant::now();
    let mut disconnected_at = None;
    let mut last_frame = Instant::now();
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = feed.stopped.cancelled() => break,
            changed = state.changed() => {
                if changed.is_err() { break; }
                match *state.borrow_and_update() {
                    RTCPeerConnectionState::Connected => {
                        connected = true;
                        ever_connected = true;
                        disconnected_at = None;
                    }
                    RTCPeerConnectionState::Disconnected => {
                        connected = false;
                        waiting_for_idr = true;
                        disconnected_at.get_or_insert_with(Instant::now);
                    }
                    RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed => break,
                    _ => {}
                }
            }
            read = sender.read(&mut rtcp_buffer) => {
                if read.is_err() { break; }
                // Reading drives RTCP interceptors (NACK receiver/report handling).
                // Camera keyframe generation is outside this send-only transport.
            }
            read = async {match &audio_sender {Some(sender)=>sender.read(&mut audio_rtcp_buffer).await,None=>std::future::pending().await}} => {if read.is_err(){break;}}
            audio = audio_frames.recv(), if audio_track.is_some() => {
                let frame=match audio {Ok(frame)=>frame,Err(broadcast::error::RecvError::Lagged(_))=>continue,Err(_)=>break};
                if !connected || waiting_for_idr {continue;}
                let config=audio_config.as_ref().unwrap();
                let packet=Packet{header:Header{version:2,marker:false,payload_type:audio_payload_type(config),sequence_number:audio_sequence,timestamp:(frame.timestamp_90k*u64::from(config.clock_rate)/90000) as u32,..Default::default()},payload:Bytes::copy_from_slice(&frame.data)};
                audio_sequence=audio_sequence.wrapping_add(1);
                if !matches!(tokio::time::timeout(Duration::from_secs(2),audio_track.as_ref().unwrap().write_rtp(&packet)).await,Ok(Ok(_))){bail!("WebRTC transport cannot accept audio");}
            }
            _ = interval.tick() => {
                if (!ever_connected && started.elapsed() > Duration::from_secs(20))
                    || disconnected_at.is_some_and(|t| Instant::now().duration_since(t) > Duration::from_secs(5))
                    || last_frame.elapsed() > Duration::from_secs(15) {
                    break;
                }
            }
            frame = frames.recv() => {
                let frame = match frame {
                    Ok(frame) => frame,
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        waiting_for_idr = true;
                        payloader = Packetizer::default();
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                last_frame = Instant::now();
                if !connected || (waiting_for_idr && !frame.keyframe) { continue; }
                let packet_data = match packet_payloads(&frame, &mut payloader) {
                    Ok(payloads) => payloads,
                    Err(_) => { waiting_for_idr = true; continue; }
                };
                if packet_data.is_empty() { continue; }
                waiting_for_idr = false;
                let last = packet_data.len() - 1;
                for (index, payload) in packet_data.into_iter().enumerate() {
                    let packet = Packet {
                        header: Header {
                            version: 2,
                            marker: index == last,
                            payload_type: 96, // Replaced by the negotiated binding.
                            sequence_number: sequence,
                            timestamp: native_timestamp(frame.timestamp_90k),
                            ..Default::default()
                        },
                        payload,
                    };
                    sequence = sequence.wrapping_add(1);
                    if !matches!(tokio::time::timeout(Duration::from_secs(2), track.write_rtp(&packet)).await, Ok(Ok(_))) {
                        bail!("WebRTC transport cannot accept media");
                    }
                }
            }
        }
    }
    Ok(())
}

fn prepared_payload(frame: &SharedFrame) -> Result<&PreparedFrame> {
    frame
        .rtc_payload
        .get_or_init(|| {
            prepare_payload(frame)
                .and_then(|payload| PreparedFrame::new(payload, RTP_PAYLOAD_LIMIT))
                .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|e| anyhow::anyhow!(e.clone()))
}

fn prepare_payload(frame: &Frame) -> Result<Bytes> {
    let start = i64::try_from(u128::from(frame.timestamp_90k) * 10_000_000 / 90_000)
        .context("camera timestamp out of range")?;
    let normalized = normalize(
        &frame.avcc,
        Metadata {
            kind: 2,
            start_100ns: start,
            end_100ns: start.saturating_add(i64::from(frame.duration_90k) * 10_000_000 / 90_000),
            keyframe: frame.keyframe,
        },
        None,
    )
    .map_err(|_| anyhow::anyhow!("unsupported camera NAL layout"))?;
    Ok(Bytes::from(normalized.into_payload()))
}

fn packet_payloads(frame: &SharedFrame, payloader: &mut Packetizer) -> Result<Vec<Bytes>> {
    Ok(payloader.payloads(prepared_payload(frame)?))
}

// Original FUN_1800a0b40 truncates the 100ns timestamp to milliseconds then *90.
fn native_timestamp(timestamp_90k: u64) -> u32 {
    (timestamp_90k / 90 * 90) as u32
}

fn h264(video: &VideoConfig) -> Result<RTCRtpCodecCapability> {
    if video.sps.len() < 4 || video.sps[0] & 31 != 7 || video.pps.first().map(|b| b & 31) != Some(8)
    {
        bail!("missing H264 parameter sets");
    }
    let profile = format!(
        "{:02x}{:02x}{:02x}",
        video.sps[1], video.sps[2], video.sps[3]
    );
    Ok(RTCRtpCodecCapability {
        mime_type: MIME_TYPE_H264.into(),
        clock_rate: 90_000,
        sdp_fmtp_line: format!(
            "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id={profile}"
        ),
        ..Default::default()
    })
}

fn audio_payload_type(config: &AudioConfig) -> u8 {
    match config.codec {
        AudioCodec::Pcmu => 0,
        AudioCodec::Pcma => 8,
        _ => 111,
    }
}
fn audio_capability(config: &AudioConfig) -> RTCRtpCodecCapability {
    RTCRtpCodecCapability {
        mime_type: match config.codec {
            AudioCodec::Pcmu => "audio/PCMU",
            AudioCodec::Pcma => "audio/PCMA",
            AudioCodec::Opus => "audio/opus",
            AudioCodec::Aac => unreachable!("AAC converted before WebRTC"),
        }
        .into(),
        clock_rate: config.clock_rate,
        channels: config.channels,
        sdp_fmtp_line: if config.codec == AudioCodec::Opus {
            "minptime=10;useinbandfec=1".into()
        } else {
            String::new()
        },
        ..Default::default()
    }
}

async fn make_peer(
    config: &Config,
    capability: &RTCRtpCodecCapability,
    audio: Option<&AudioConfig>,
    udp: Arc<UDPMuxDefault>,
) -> Result<RTCPeerConnection> {
    let mut media = MediaEngine::default();
    media.register_codec(
        RTCRtpCodecParameters {
            capability: capability.clone(),
            payload_type: 96,
            ..Default::default()
        },
        RTPCodecType::Video,
    )?;
    if let Some(audio) = audio {
        media.register_codec(
            RTCRtpCodecParameters {
                capability: audio_capability(audio),
                payload_type: audio_payload_type(audio),
                ..Default::default()
            },
            RTPCodecType::Audio,
        )?;
    }
    let registry = register_default_interceptors(Registry::new(), &mut media)?;
    let mut settings = SettingEngine::default();
    settings.set_network_types(vec![NetworkType::Udp4]);
    settings.set_udp_network(UDPNetwork::Muxed(udp));
    settings.set_ice_timeouts(
        Some(Duration::from_secs(5)),
        Some(Duration::from_secs(15)),
        Some(Duration::from_secs(2)),
    );
    if let Some(ip) = config.public_ip {
        settings.set_nat_1to1_ips(vec![ip.to_string()], RTCIceCandidateType::Host);
    }
    Ok(APIBuilder::new()
        .with_media_engine(media)
        .with_interceptor_registry(registry)
        .with_setting_engine(settings)
        .build()
        .new_peer_connection(RTCConfiguration {
            ice_servers: config
                .ice_servers
                .iter()
                .map(|url| RTCIceServer {
                    urls: vec![url.clone()],
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        })
        .await?)
}

fn original_answer(answer: &RTCSessionDescription) -> Result<String> {
    let mut media_index = None;
    let mut mid = "0";
    let mut selected = None;
    for line in answer.sdp.lines() {
        if line.starts_with("m=") {
            media_index = Some(media_index.map_or(0u16, |i| i + 1));
        }
        if let Some(value) = line.strip_prefix("a=mid:") {
            mid = value;
        }
        if let Some(value) = line.strip_prefix("a=candidate:") {
            if value
                .split_whitespace()
                .nth(2)
                .is_some_and(|protocol| protocol.eq_ignore_ascii_case("udp"))
            {
                selected = Some(
                    json!({"type":"candidate", "sdpMid":mid, "sdpMLineIndex":media_index.unwrap_or(0), "candidate":format!("candidate:{value}")}),
                );
                break;
            }
        }
    }
    let candidate = selected.context("no UDP ICE candidate")?;
    Ok(format!(
        "{}|-|-|{}",
        serde_json::to_string(answer)?,
        candidate
    ))
}

async fn send_text(socket: &mut WebSocket, text: String) -> Result<()> {
    tokio::time::timeout(
        Duration::from_secs(5),
        socket.send(Message::Text(text.into())),
    )
    .await
    .context("signaling send timeout")?
    .map_err(|_| anyhow::anyhow!("signaling socket closed"))
}

async fn signal_error(socket: &mut WebSocket, text: &str) {
    let _ = send_text(socket, text.into()).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use webrtc::rtp::{codecs::h264::H264Payloader, packetizer::Payloader};

    #[test]
    fn original_answer_uses_udp_candidate_and_native_delimiter() {
        let answer = RTCSessionDescription::answer("v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\nm=video 9 UDP/TLS/RTP/SAVPF 102\r\na=mid:video\r\na=candidate:1 1 tcp 1 127.0.0.1 9 typ host\r\na=candidate:2 1 UDP 2 192.0.2.1 50000 typ host\r\n".into()).unwrap();
        let result = original_answer(&answer).unwrap();
        let (sdp, candidate) = result.split_once("|-|-|").unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(sdp).unwrap()["type"],
            "answer"
        );
        let value: serde_json::Value = serde_json::from_str(candidate).unwrap();
        assert_eq!(value["sdpMid"], "video");
        assert_eq!(value["sdpMLineIndex"], 0);
        assert!(value["candidate"].as_str().unwrap().contains("UDP"));
    }

    #[test]
    fn viewers_share_prepared_payloads_without_changing_packetization() {
        let nals = [
            &[0x67, 0x42, 0, 0x1f][..],
            &[0x68, 0xce, 0x38][..],
            &[0x65, 0xb8, 3, 4][..],
        ];
        let mut avcc = Vec::new();
        let mut annex_b = Vec::new();
        for nal in nals {
            avcc.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            avcc.extend_from_slice(nal);
            annex_b.extend_from_slice(&[0, 0, 0, 1]);
            annex_b.extend_from_slice(nal);
        }
        let frame = SharedFrame::new(Frame {
            avcc,
            timestamp_90k: 9000,
            duration_90k: 6000,
            keyframe: true,
        });
        assert!(frame.rtc_payload.get().is_none());
        let expected = H264Payloader::default()
            .payload(RTP_PAYLOAD_LIMIT, &Bytes::from(annex_b.clone()))
            .unwrap();
        std::thread::scope(|scope| {
            let workers: Vec<_> = (0..16)
                .map(|_| {
                    scope.spawn(|| {
                        assert_eq!(prepare_payload(&frame).unwrap().as_ref(), annex_b);
                        let packets = packet_payloads(&frame, &mut Packetizer::default()).unwrap();
                        assert_eq!(packets, expected);
                        packets.last().unwrap().as_ptr() as usize
                    })
                })
                .collect();
            let pointers: Vec<_> = workers.into_iter().map(|t| t.join().unwrap()).collect();
            assert!(pointers.iter().all(|p| *p == pointers[0]));
        });
        let invalid = SharedFrame::new(Frame {
            avcc: vec![0, 0, 0, 8, 0x65],
            timestamp_90k: 0,
            duration_90k: 6000,
            keyframe: true,
        });
        assert!(prepared_payload(&invalid).is_err());
        assert!(invalid.rtc_payload.get().unwrap().is_err());
    }

    #[test]
    fn packetization_preserves_large_idr_nal_body() {
        let mut nal = vec![0x65];
        nal.extend((0..3200).map(|n| (n % 251 + 2) as u8));
        let mut avcc = (nal.len() as u32).to_be_bytes().to_vec();
        avcc.extend_from_slice(&nal);
        let frame = SharedFrame::new(Frame {
            avcc,
            timestamp_90k: 6001,
            duration_90k: 6000,
            keyframe: true,
        });
        let payloads = packet_payloads(&frame, &mut Packetizer::default()).unwrap();
        assert!(payloads.len() > 1);
        assert!(payloads
            .iter()
            .all(|p| p.len() <= RTP_PAYLOAD_LIMIT && p[0] & 31 == 28));
        assert_ne!(payloads.first().unwrap()[1] & 0x80, 0);
        assert_ne!(payloads.last().unwrap()[1] & 0x40, 0);
        let mut restored = vec![(payloads[0][0] & 0xe0) | (payloads[0][1] & 31)];
        for packet in payloads {
            restored.extend_from_slice(&packet[2..]);
        }
        assert_eq!(restored, nal);
        assert_eq!(native_timestamp(frame.timestamp_90k), 5940);
    }
}
