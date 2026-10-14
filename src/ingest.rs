//! One native RTSP reader shared by WebRTC, MSE, recording and RTSP relay.
use crate::config::{self, Config};
use anyhow::{Context, Result};
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::sync::{broadcast, Mutex};
use tokio_util::sync::CancellationToken;
use unreal_rtsp::{AudioConfig, AudioFrame, Frame, MediaEvent, VideoCodec, VideoConfig};
/// An immutable sample with lazy shared RTC normalization. MSE-only feeds
/// do not pay the Annex B conversion cost.
pub struct SharedFrame {
    frame: Frame,
    pub(crate) rtc_payload: OnceLock<std::result::Result<bytes::Bytes, String>>,
}
impl SharedFrame {
    pub fn new(frame: Frame) -> Self {
        Self {
            frame,
            rtc_payload: OnceLock::new(),
        }
    }
}
impl std::ops::Deref for SharedFrame {
    type Target = Frame;
    fn deref(&self) -> &Frame {
        &self.frame
    }
}
pub struct Feed {
    input_codec: VideoCodec,
    video: VideoConfig,
    audio: Option<AudioConfig>,
    audio_warning: Option<String>,
    archive_audio: Option<AudioConfig>,
    frames: broadcast::Sender<Arc<SharedFrame>>,
    audio_frames: broadcast::Sender<Arc<AudioFrame>>,
    rtc_audio: broadcast::Sender<Arc<AudioFrame>>,
    archive_audio_frames: broadcast::Sender<Arc<AudioFrame>>,
    audio_errors: std::sync::atomic::AtomicU64,
    audio_sr_sync: std::sync::atomic::AtomicBool,
    audio_correction: std::sync::atomic::AtomicI64,
    pub stopped: CancellationToken,
}
impl Feed {
    pub fn video(&self) -> VideoConfig {
        self.video.clone()
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<SharedFrame>> {
        self.frames.subscribe()
    }
    pub fn audio(&self) -> Option<AudioConfig> {
        self.audio.clone()
    }
    pub fn audio_warning(&self) -> Option<&str> {
        self.audio_warning.as_deref()
    }
    pub fn subscribe_audio(&self) -> broadcast::Receiver<Arc<AudioFrame>> {
        self.audio_frames.subscribe()
    }
    pub fn subscribe_rtc_audio(&self) -> broadcast::Receiver<Arc<AudioFrame>> {
        self.rtc_audio.subscribe()
    }
    pub fn archive_audio(&self) -> Option<AudioConfig> {
        self.archive_audio.clone()
    }
    pub fn subscribe_archive_audio(&self) -> broadcast::Receiver<Arc<AudioFrame>> {
        self.archive_audio_frames.subscribe()
    }
}
#[derive(Default)]
pub struct Hub {
    feeds: Mutex<HashMap<String, Arc<Feed>>>,
    initializing: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    video_errors: Mutex<HashMap<String, String>>,
}
impl Hub {
    pub async fn acquire(
        &self,
        id: &str,
        config: &Config,
        shutdown: CancellationToken,
    ) -> Result<Arc<Feed>> {
        let stream = config.streams.get(id).context("unknown stream")?;
        if let Some(feed) = self
            .feeds
            .lock()
            .await
            .get(id)
            .filter(|f| !f.stopped.is_cancelled())
        {
            return Ok(feed.clone());
        }
        let initialize = self
            .initializing
            .lock()
            .await
            .entry(id.to_owned())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let _guard = initialize.lock().await;
        // Concurrent requests for this camera share one handshake, without blocking other cameras.
        if let Some(feed) = self
            .feeds
            .lock()
            .await
            .get(id)
            .filter(|f| !f.stopped.is_cancelled())
        {
            return Ok(feed.clone());
        }
        let source = config::source_url(stream)?;
        let connected = tokio::time::timeout(
            Duration::from_secs(30),
            crate::video::Source::connect(&source, config.nvidia_device, config.nvidia_bitrate),
        )
        .await
        .map_err(|_| anyhow::anyhow!("camera/GPU initialization timeout"))
        .and_then(|r| r);
        let mut client = match connected {
            Ok(client) => {
                self.video_errors.lock().await.remove(id);
                client
            }
            Err(error) => {
                self.video_errors
                    .lock()
                    .await
                    .insert(id.into(), error.to_string());
                return Err(error);
            }
        };
        let (frames, _) = broadcast::channel(32);
        let (audio_frames, _) = broadcast::channel(128);
        let (rtc_audio, _) = broadcast::channel(128);
        let (archive_audio_frames, _) = broadcast::channel(128);
        let mut audio_converter = client
            .audio()
            .map(crate::audio::RtcAudio::new)
            .transpose()?;
        let mut archive_converter = client
            .audio()
            .map(crate::audio::ArchiveAudio::new)
            .transpose()?;
        if let Some(warning) = client.audio_warning() {
            tracing::warn!(stream=%id,reason=warning,"Camera audio unavailable");
        }
        let feed = Arc::new(Feed {
            input_codec: client.video_codec(),
            video: client.video().clone(),
            audio: client.audio().cloned(),
            audio_warning: client.audio_warning().map(str::to_owned),
            archive_audio: archive_converter
                .as_ref()
                .map(crate::audio::ArchiveAudio::config),
            frames,
            audio_frames,
            rtc_audio,
            archive_audio_frames,
            audio_errors: std::sync::atomic::AtomicU64::new(0),
            audio_sr_sync: std::sync::atomic::AtomicBool::new(false),
            audio_correction: std::sync::atomic::AtomicI64::new(0),
            stopped: CancellationToken::new(),
        });
        self.feeds.lock().await.insert(id.into(), feed.clone());
        let live = feed.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            let mut last_subscriber = tokio::time::Instant::now();
            let mut last_video = tokio::time::Instant::now();
            loop {
                tokio::select! {
                    _=shutdown.cancelled()=>break,
                    result=tokio::time::timeout(Duration::from_secs(15),client.next_media())=> match result {
                        Ok(Ok(MediaEvent::Video(frame)))=>{
                            last_video=tokio::time::Instant::now();
                            if client.video()!=&live.video {tracing::warn!(stream=%id,"H264 configuration changed; reopening session");break}
                            if live.frames.receiver_count()>0 || live.audio_frames.receiver_count()>0 || live.rtc_audio.receiver_count()>0 || live.archive_audio_frames.receiver_count()>0 {last_subscriber=tokio::time::Instant::now();}
                            else if last_subscriber.elapsed()>=Duration::from_secs(30){break}
                            let _=live.frames.send(Arc::new(SharedFrame::new(frame)));
                        },
                        Ok(Ok(MediaEvent::Audio(frame)))=>{
                            let (synced,correction)=client.audio_sync();
                            live.audio_sr_sync.store(synced,std::sync::atomic::Ordering::Relaxed);
                            live.audio_correction.store(correction,std::sync::atomic::Ordering::Relaxed);
                            if last_video.elapsed()>Duration::from_secs(15) {tracing::warn!(stream=%id,"RTSP video stopped while audio continued");break;}
                            if let Some(converter)=audio_converter.as_mut().filter(|_|live.rtc_audio.receiver_count()>0) {
                                // Decode AAC once per camera, preserving encoded AAC for archives.
                                match converter.push(&frame) {
                                    Ok(converted)=>for packet in converted {let _=live.rtc_audio.send(Arc::new(packet));},
                                    Err(_)=>{let count=live.audio_errors.fetch_add(1,std::sync::atomic::Ordering::Relaxed)+1;if count==1||count.is_multiple_of(100) {tracing::warn!(stream=%id,errors=count,"WebRTC audio conversion failed");}}
                                }
                            }
                            if let Some(converter)=archive_converter.as_mut().filter(|_|live.archive_audio_frames.receiver_count()>0) {
                                match converter.push(&frame) {
                                    Ok(converted)=>for packet in converted {let _=live.archive_audio_frames.send(Arc::new(packet));},
                                    Err(_)=>{let count=live.audio_errors.fetch_add(1,std::sync::atomic::Ordering::Relaxed)+1;if count==1||count.is_multiple_of(100) {tracing::warn!(stream=%id,errors=count,"Archive audio conversion failed");}}
                                }
                            }
                            let _=live.audio_frames.send(Arc::new(frame));
                        },
                        _=>{tracing::warn!(stream=%id,"RTSP source ended; viewers reconnect and recorder retries");break;}
                    }
                }
            }
            live.stopped.cancel();
        });
        Ok(feed)
    }
    pub async fn online(&self, id: &str) -> bool {
        self.feeds
            .lock()
            .await
            .get(id)
            .is_some_and(|f| !f.stopped.is_cancelled())
    }
    pub async fn video_status(&self, id: &str) -> serde_json::Value {
        if let Some(error) = self.video_errors.lock().await.get(id).cloned() {
            return serde_json::json!({"state":"unavailable","error":error});
        }
        let feeds = self.feeds.lock().await;
        match feeds.get(id).filter(|f| !f.stopped.is_cancelled()) {
            Some(f) => {
                serde_json::json!({"state":"available","input":format!("{:?}",f.input_codec),"output":"H264","processing":if f.input_codec==VideoCodec::H265 {"nvidia_nvdec_nvenc"}else{"passthrough"}})
            }
            None => serde_json::json!({"state":"offline"}),
        }
    }
    pub async fn audio_status(&self, id: &str) -> serde_json::Value {
        let feeds = self.feeds.lock().await;
        let Some(feed) = feeds.get(id).filter(|f| !f.stopped.is_cancelled()) else {
            return serde_json::json!({"state":"offline"});
        };
        let describe = |config: &AudioConfig| serde_json::json!({"codec":format!("{:?}",config.codec),"sample_rate":config.clock_rate,"channels":config.channels});
        let errors = feed.audio_errors.load(std::sync::atomic::Ordering::Relaxed);
        serde_json::json!({"state":if feed.audio_warning.is_some(){"unsupported"}else if errors>0{"conversion_errors"}else if feed.audio.is_some(){"available"}else{"no_audio"},
            "input":feed.audio.as_ref().map(describe),"webrtc":feed.audio.as_ref().map(crate::audio::rtc_config).as_ref().map(describe),
            "archive":feed.archive_audio.as_ref().map(describe),"warning":feed.audio_warning(),"conversion_errors":errors,
            "synchronization":if feed.audio_sr_sync.load(std::sync::atomic::Ordering::Relaxed){"rtcp_sender_reports"}else{"first_rtp_arrival"},
            "audio_correction_ms":feed.audio_correction.load(std::sync::atomic::Ordering::Relaxed) as f64/90.0})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn stalled_camera_does_not_lock_other_camera_or_status() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url_var = format!("UVMS_HUB_TEST_{}", uuid::Uuid::new_v4().simple());
        std::env::set_var(
            &url_var,
            format!("rtsp://{}/camera", listener.local_addr().unwrap()),
        );
        let mut config = Config::default();
        config.streams.insert(
            "slow".into(),
            config::StreamConfig {
                url_env: url_var.clone(),
                record: false,
            },
        );
        config.streams.insert(
            "other".into(),
            config::StreamConfig {
                url_env: format!("{url_var}_MISSING"),
                record: false,
            },
        );
        let config = Arc::new(config);
        let hub = Arc::new(Hub::default());
        let reader = {
            let hub = hub.clone();
            let config = config.clone();
            tokio::spawn(
                async move { hub.acquire("slow", &config, CancellationToken::new()).await },
            )
        };
        let (_socket, _) = listener.accept().await.unwrap();
        assert!(
            !tokio::time::timeout(Duration::from_millis(250), hub.online("other"))
                .await
                .unwrap()
        );
        assert!(tokio::time::timeout(
            Duration::from_millis(250),
            hub.acquire("other", &config, CancellationToken::new())
        )
        .await
        .unwrap()
        .is_err());
        reader.abort();
        let _ = reader.await;
        std::env::remove_var(url_var);
    }
}
