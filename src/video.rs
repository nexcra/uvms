//! Per-camera HEVC -> NVIDIA -> H264 adapter, upstream of every shared output.
use anyhow::{ensure, Context, Result};
use std::{
    collections::{HashMap, VecDeque},
    sync::OnceLock,
    time::Duration,
};
use tokio::sync::{mpsc, oneshot};
use unreal_rtsp::{AudioConfig, Client, Frame, MediaEvent, VideoCodec, VideoConfig};

type Answer = oneshot::Sender<Result<Vec<Frame>>>;
struct Worker {
    input: mpsc::Sender<(Frame, Answer)>,
}
impl Worker {
    async fn start(device: u32, bitrate: u32) -> Result<Self> {
        let (input, mut receive) = mpsc::channel::<(Frame, Answer)>(1);
        let (ready, wait) = oneshot::channel();
        std::thread::Builder::new()
            .name("uvms-nvidia".into())
            .spawn(move || {
                let mut engine = match unreal_nvidia::Transcoder::new(device, bitrate) {
                    Ok(e) => {
                        let _ = ready.send(Ok(()));
                        e
                    }
                    Err(e) => {
                        let _ = ready.send(Err(anyhow::anyhow!(
                            "NVIDIA GPU initialization failed: {e}"
                        )));
                        return;
                    }
                };
                let mut duration = HashMap::new();
                let mut last = None;
                while let Some((frame, reply)) = receive.blocking_recv() {
                    let result = (|| {
                        ensure!(
                            duration.len() < 64,
                            "NVIDIA output stalled; pending timestamp limit"
                        );
                        duration.insert(frame.timestamp_90k, frame.duration_90k);
                        let bytes = to_annex_b(&frame.avcc)?;
                        let output =
                            engine.push(&bytes, frame.timestamp_90k, frame.duration_90k)?;
                        output
                            .into_iter()
                            .map(|packet| {
                                ensure!(
                                    last.is_none_or(|p| packet.timestamp_90k > p),
                                    "NVENC output timestamps did not advance"
                                );
                                last = Some(packet.timestamp_90k);
                                let duration = duration
                                    .remove(&packet.timestamp_90k)
                                    .context("NVENC returned unknown timestamp")?;
                                let (avcc, keyframe) = from_annex_b(&packet.annex_b)?;
                                Ok(Frame {
                                    avcc,
                                    keyframe,
                                    timestamp_90k: packet.timestamp_90k,
                                    duration_90k: duration,
                                })
                            })
                            .collect()
                    })();
                    let failed = result.is_err();
                    let _ = reply.send(result);
                    if failed {
                        break;
                    }
                }
                // Engine and CUDA context are destroyed on their original OS thread.
            })?;
        wait.await
            .context("NVIDIA worker stopped during initialization")??;
        Ok(Self { input })
    }
    async fn push(&self, frame: Frame) -> Result<Vec<Frame>> {
        let (reply, wait) = oneshot::channel();
        self.input
            .send((frame, reply))
            .await
            .context("NVIDIA worker stopped")?;
        wait.await.context("NVIDIA worker stopped")?
    }
}

pub struct Source {
    client: Client,
    worker: Option<Worker>,
    video: VideoConfig,
    pending: VecDeque<MediaEvent>,
}
impl Source {
    pub async fn connect(url: &str, device: u32, bitrate: u32) -> Result<Self> {
        let client = Client::connect(url).await.map_err(|_| {
            anyhow::anyhow!("camera RTSP handshake failed (H264/H265 low-delay input required)")
        })?;
        let video = client.video().clone();
        let worker = if client.video_codec() == VideoCodec::H265 {
            Some(Worker::start(device, bitrate).await?)
        } else {
            None
        };
        let mut result = Self {
            client,
            worker,
            video,
            pending: VecDeque::new(),
        };
        if result.worker.is_some() {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
            let mut queued = 0;
            loop {
                let event = tokio::time::timeout_at(deadline, result.convert_next())
                    .await
                    .context("H265 GPU initialization timed out waiting for H264 IDR")??;
                queued += match &event {
                    MediaEvent::Video(f) => f.avcc.len(),
                    MediaEvent::Audio(f) => f.data.len(),
                };
                ensure!(
                    result.pending.len() < 512 && queued <= 32 * 1024 * 1024,
                    "GPU startup media queue exceeded limit"
                );
                let ready = matches!(&event,MediaEvent::Video(f) if f.keyframe);
                if let MediaEvent::Video(f) = &event {
                    if ready {
                        result.video = config_from_frame(&f.avcc)?;
                    }
                }
                result.pending.push_back(event);
                if ready {
                    break;
                }
            }
        }
        Ok(result)
    }
    pub fn video(&self) -> &VideoConfig {
        if self.worker.is_some() {
            &self.video
        } else {
            self.client.video()
        }
    }
    pub fn video_codec(&self) -> VideoCodec {
        self.client.video_codec()
    }
    pub fn audio(&self) -> Option<&AudioConfig> {
        self.client.audio()
    }
    pub fn audio_warning(&self) -> Option<&str> {
        self.client.audio_warning()
    }
    pub fn audio_sync(&self) -> (bool, i64) {
        self.client.audio_sync()
    }
    pub async fn next_media(&mut self) -> Result<MediaEvent> {
        if let Some(event) = self.pending.pop_front() {
            return Ok(event);
        }
        self.convert_next().await
    }
    async fn convert_next(&mut self) -> Result<MediaEvent> {
        loop {
            let event = self.client.next_media().await?;
            match (event, self.worker.as_ref()) {
                (MediaEvent::Video(frame), Some(worker)) => {
                    let frames = worker.push(frame).await?;
                    // Low-delay input/output has one picture per access unit. Reject buffering
                    // mismatches instead of mixing reordered video with audio incorrectly.
                    ensure!(frames.len() <= 1, "Unexpected reordered NVIDIA output");
                    if let Some(frame) = frames.into_iter().next() {
                        if frame.keyframe {
                            let config = config_from_frame(&frame.avcc)?;
                            if self.video.sps.first().is_some_and(|n| n & 31 == 7) {
                                ensure!(
                                    self.video == config,
                                    "NVENC configuration changed; reconnect required"
                                );
                            }
                        }
                        return Ok(MediaEvent::Video(frame));
                    }
                }
                (event, _) => return Ok(event),
            }
        }
    }
}
fn nals(avcc: &[u8]) -> Result<Vec<&[u8]>> {
    ensure!(avcc.len() <= 8 * 1024 * 1024, "Video frame too large");
    let mut at = 0;
    let mut output = Vec::new();
    while at < avcc.len() {
        ensure!(at + 4 <= avcc.len(), "Truncated video NAL length");
        let len = u32::from_be_bytes(avcc[at..at + 4].try_into().unwrap()) as usize;
        at += 4;
        ensure!(
            len > 0 && len <= avcc.len() - at && output.len() < 256,
            "Invalid video NAL length/count"
        );
        output.push(&avcc[at..at + len]);
        at += len;
    }
    ensure!(!output.is_empty(), "Empty video frame");
    Ok(output)
}
fn to_annex_b(avcc: &[u8]) -> Result<Vec<u8>> {
    let mut output = Vec::with_capacity(avcc.len());
    for nal in nals(avcc)? {
        output.extend_from_slice(&[0, 0, 0, 1]);
        output.extend_from_slice(nal);
    }
    Ok(output)
}
fn from_annex_b(bytes: &[u8]) -> Result<(Vec<u8>, bool)> {
    ensure!(bytes.len() <= 8 * 1024 * 1024, "NVENC packet too large");
    let mut starts = Vec::new();
    let mut at = 0;
    while at + 3 <= bytes.len() {
        let length = if bytes[at..].starts_with(&[0, 0, 0, 1]) {
            4
        } else if bytes[at..].starts_with(&[0, 0, 1]) {
            3
        } else {
            at += 1;
            continue;
        };
        ensure!(starts.len() < 256, "NVENC NAL count exceeded");
        starts.push((at, at + length));
        at += length;
    }
    ensure!(
        starts.first().is_some_and(|&(at, _)| at == 0),
        "NVENC output lacks Annex B prefix"
    );
    let mut avcc = Vec::with_capacity(bytes.len() + 256);
    let mut key = false;
    let mut slice = false;
    for (i, &(_, begin)) in starts.iter().enumerate() {
        let end = starts.get(i + 1).map(|v| v.0).unwrap_or(bytes.len());
        ensure!(end > begin, "Empty NVENC NAL");
        let nal = &bytes[begin..end];
        let kind = nal[0] & 31;
        ensure!(
            nal[0] & 128 == 0 && (1..=23).contains(&kind),
            "Invalid NVENC H264 NAL"
        );
        key |= kind == 5;
        slice |= matches!(kind, 1..=5);
        avcc.extend_from_slice(&(nal.len() as u32).to_be_bytes());
        avcc.extend_from_slice(nal);
    }
    ensure!(
        slice && avcc.len() <= 8 * 1024 * 1024,
        "NVENC output lacks video/too large"
    );
    Ok((avcc, key))
}
fn config_from_frame(avcc: &[u8]) -> Result<VideoConfig> {
    let mut sps = None;
    let mut pps = None;
    for nal in nals(avcc)? {
        match nal[0] & 31 {
            7 => sps = Some(nal.to_vec()),
            8 => pps = Some(nal.to_vec()),
            _ => {}
        }
    }
    unreal_rtsp::h264_video_config(
        sps.context("NVENC IDR omitted SPS")?,
        pps.context("NVENC IDR omitted PPS")?,
    )
}
pub async fn status(device: u32, bitrate: u32) -> serde_json::Value {
    static PROBE: OnceLock<(bool, String)> = OnceLock::new();
    let (available,message)=tokio::task::spawn_blocking(move||PROBE.get_or_init(||match unreal_nvidia::Transcoder::new(device,bitrate){
        Ok(_)=>(true,"Driver and HEVC decode/H264 encode session available; actual conversion requires a media test".into()),
        Err(e)=>(false,e.to_string()),
    }).clone()).await.unwrap_or((false,"GPU probe worker failed".into()));
    serde_json::json!({"compiled":unreal_nvidia::COMPILED,"available":available,"device":device,"message":message,"input":"HEVC Main 8-bit 4:2:0, progressive, no B-frames","output":"H264 baseline","runtime":"NVIDIA Driver API + NVDEC + NVENC"})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encoder_packet_preserves_nals_and_rejects_malformed_output() {
        let bytes = [
            0, 0, 0, 1, 0x67, 0x42, 0, 0, 1, 0x68, 0xce, 0, 0, 0, 1, 0x65, 1, 2, 3,
        ];
        let (avcc, key) = from_annex_b(&bytes).unwrap();
        assert!(key);
        assert_eq!(
            nals(&avcc).unwrap(),
            vec![&[0x67, 0x42][..], &[0x68, 0xce][..], &[0x65, 1, 2, 3][..]]
        );
        assert!(from_annex_b(&[0, 0, 1, 0x67, 0x42]).is_err());
        assert!(from_annex_b(&[0, 0, 1, 0x80]).is_err());
        assert!(to_annex_b(&[0, 0, 0, 4, 1]).is_err());
        assert_eq!(
            to_annex_b(&[0, 0, 0, 3, 0x26, 1, 9]).unwrap(),
            vec![0, 0, 0, 1, 0x26, 1, 9]
        );
    }
    #[tokio::test]
    async fn absent_gpu_is_an_explicit_error() {
        // Opt in on a known GPU-less runner; never mark real hardware conversion tested.
        if std::env::var_os("UVMS_EXPECT_NO_GPU").is_some() {
            assert!(Worker::start(0, 4_000_000).await.is_err());
        }
    }
}
