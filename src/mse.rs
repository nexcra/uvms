//! Native v15 binary envelopes around Rust ISO-BMFF, with original text commands.
use crate::App;
use axum::extract::ws::{Message, WebSocket};
use std::{sync::Arc, time::Duration};
use tokio::sync::{broadcast, OwnedSemaphorePermit};
use unreal_core_port::mse::{self, InitTimeshift, Tracks};
use unreal_mp4::Muxer;
async fn send(ws: &mut WebSocket, data: Vec<u8>) -> anyhow::Result<()> {
    tokio::time::timeout(
        Duration::from_secs(5),
        ws.send(Message::Binary(data.into())),
    )
    .await??;
    Ok(())
}
async fn error(ws: &mut WebSocket, text: &str) {
    if let Ok(b) = mse::error(Tracks::Video, true, text) {
        let _ = send(ws, b).await;
    }
}
pub async fn serve(
    mut ws: WebSocket,
    app: Arc<App>,
    mut id: String,
    _permit: OwnedSemaphorePermit,
) {
    // The SDK appends subsequent init segments to the same SourceBuffer. Keep
    // one playback clock across continue/switch, rebasing each new source at
    // its first IDR. Frames discarded while paused do not advance this clock.
    let mut next_timestamp_90k = 0u64;
    'source: loop {
        let session_offset_90k = next_timestamp_90k;
        let feed = match app
            .hub
            .acquire(&id, &app.config, app.shutdown.clone())
            .await
        {
            Ok(f) => f,
            Err(_) => {
                error(&mut ws, "RTSP source unavailable").await;
                break;
            }
        };
        let v = feed.video();
        let audio = feed.archive_audio();
        let audio_config = audio.as_ref().map(|a| unreal_mp4::AudioConfig {
            sample_rate: a.clock_rate,
            channels: a.channels,
            decoder_config: a.config.clone(),
        });
        let mut mux = match Muxer::new_av(&v.sps, &v.pps, v.width, v.height, audio_config) {
            Ok(m) => m,
            Err(_) => {
                error(&mut ws, "Unsupported H264 stream").await;
                break;
            }
        };
        let xml = format!(
            "<metadata><mimetypecodec>video/mp4; codecs=\"{}\"</mimetypecodec></metadata>",
            mux.codec()
        );
        let Ok(init) = mse::initialization(
            if audio.is_some() {
                Tracks::AudioVideo
            } else {
                Tracks::Video
            },
            true,
            InitTimeshift::default(),
            &xml,
            mux.init_segment(),
        ) else {
            break;
        };
        if send(&mut ws, init).await.is_err() {
            break;
        }
        let mut rx = feed.subscribe();
        let mut audio_rx = feed.subscribe_archive_audio();
        let mut base = None;
        let mut paused = false;
        let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
        let mut alive = tokio::time::Instant::now();
        loop {
            tokio::select! {
                            _=app.shutdown.cancelled()=>break 'source,
                            _=feed.stopped.cancelled()=>{error(&mut ws,"RTSP connection ended; reconnect").await;break 'source},
                            _=heartbeat.tick()=>{if alive.elapsed()>Duration::from_secs(45){break 'source}
            if !matches!(tokio::time::timeout(Duration::from_secs(5),ws.send(Message::Ping(bytes::Bytes::new()))).await,Ok(Ok(()))){break 'source}},
                            message=ws.recv()=>match message {
                                Some(Ok(Message::Text(t)))=>{
                                    alive=tokio::time::Instant::now();
                                    if t.eq_ignore_ascii_case("pause"){paused=true;}
                                    else if t.eq_ignore_ascii_case("continue"){continue 'source;}
                                    else if let Some(alias)=t.strip_prefix("switch:") {if app.config.streams.contains_key(alias){id=alias.to_owned();continue 'source}else{error(&mut ws,"Unknown stream").await;break 'source}}
                                    else {error(&mut ws,"Live stream supports pause, continue and switch; seek requires VOD").await;break 'source}
                                },
                                Some(Ok(Message::Ping(_)|Message::Pong(_)))=>alive=tokio::time::Instant::now(),
                                _=>break 'source,
                            },
                            sample=audio_rx.recv(), if audio.is_some()=>match sample {
                                Ok(f)=>{
                                    if paused {continue}
                                    let Some(base_ts)=base else {continue};
                                    let Some(relative)=f.timestamp_90k.checked_sub(base_ts) else {continue};
                                    let Some(ts)=relative.checked_add(session_offset_90k) else {break 'source};
                                    let clock=u64::from(audio.as_ref().unwrap().clock_rate);
                                    let Some(audio_ts)=ts.checked_mul(clock).map(|t|t/90000) else {break 'source};
                                    let mut packet=match mux.audio_fragment_with_prefix(&f.data,audio_ts,f.duration,10) {Ok(b)=>b,Err(_)=>{error(&mut ws,"Invalid audio sample; reconnect").await;break 'source}};
                                    let Ok(header)=mse::media_header(0,(ts/90).min(i32::MAX as u64)as i32,None) else {break 'source};
                                    packet[..10].copy_from_slice(&header);
                                    if send(&mut ws,packet).await.is_err(){break 'source}
                                    next_timestamp_90k=next_timestamp_90k.max(ts.saturating_add(u64::from(f.duration)*90000/clock));
                                },
                                Err(_)=>{error(&mut ws,"Audio interrupted; reconnect").await;break 'source},
                            },
                            frame=rx.recv()=>match frame {
                                Ok(f)=>{
                                    if paused{continue}
            if base.is_none() {if !f.keyframe{continue} base=Some(f.timestamp_90k)}
                                    let Some(ts)=f.timestamp_90k.checked_sub(base.unwrap()).and_then(|relative|session_offset_90k.checked_add(relative)) else {error(&mut ws,"Invalid media timestamp").await;break 'source};
                                    let Some(next)=ts.checked_add(u64::from(f.duration_90k)) else {error(&mut ws,"Media timestamp overflow").await;break 'source};
                                    let mut packet=match mux.fragment_with_prefix(&f.avcc,ts,f.duration_90k,f.keyframe,10){Ok(b)=>b,Err(_)=>{error(&mut ws,"H264 format changed or unsupported B-frame; reconnect with baseline camera stream").await;break 'source}};
                                    let Ok(header)=mse::media_header(0,(ts/90).min(i32::MAX as u64)as i32,None) else {break 'source};
                                    packet[..10].copy_from_slice(&header);
                                    if send(&mut ws,packet).await.is_err(){break 'source}
                                    next_timestamp_90k=next_timestamp_90k.max(next);
                                },
                                Err(broadcast::error::RecvError::Lagged(_))=>{error(&mut ws,"Slow viewer: reconnect at keyframe").await;break 'source},
                                Err(_)=>break 'source,
                            }
                        }
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(2), ws.send(Message::Close(None))).await;
}
