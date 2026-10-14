//! Bounded recording backlog, independent of segment fsync and archive indexing.
use crate::ingest::SharedFrame;
use anyhow::{bail, Context, Result};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;
use unreal_rtsp::AudioFrame;

const MAX_SAMPLES: usize = 1024;
const MAX_BYTES: usize = 16 * 1024 * 1024;

pub enum Sample {
    Video {
        frame: Arc<SharedFrame>,
        received_ms: u64,
    },
    Audio(Arc<AudioFrame>),
}
pub struct Queued {
    pub sample: Sample,
    _budget: OwnedSemaphorePermit,
}
pub struct Input {
    pub rx: mpsc::Receiver<Result<Queued>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Input {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Input {
    pub fn new(
        mut video: broadcast::Receiver<Arc<SharedFrame>>,
        mut audio: broadcast::Receiver<Arc<AudioFrame>>,
        has_audio: bool,
        stopped: CancellationToken,
    ) -> Self {
        let (tx, rx) = mpsc::channel(MAX_SAMPLES);
        let budget = Arc::new(Semaphore::new(MAX_BYTES));
        let task = tokio::spawn(async move {
            let result: Result<()> = async {
                loop {
                    // Drain already received samples before reporting source EOF.
                    let sample = tokio::select! {
                        _=stopped.cancelled(), if video.is_empty() && (!has_audio || audio.is_empty()) => bail!("recording source ended"),
                        _=tx.closed()=>return Ok(()),
                        f=video.recv()=>Sample::Video { frame:f.context("recording video backlog")?,received_ms:crate::record::now_ms() },
                        a=audio.recv(),if has_audio=>Sample::Audio(a.context("recording audio backlog")?),
                    };
                    let bytes = match &sample {
                        Sample::Video {frame,..}=>frame.avcc.len(),
                        Sample::Audio(frame)=>frame.data.len(),
                    }.saturating_add(128);
                    if bytes > MAX_BYTES { bail!("recording sample exceeds backlog budget") }
                    let permit = budget.clone().acquire_many_owned(bytes as u32).await?;
                    if tx.send(Ok(Queued {sample,_budget:permit})).await.is_err() { return Ok(()) }
                }
            }.await;
            if let Err(error) = result {
                let _ = tx.send(Err(error)).await;
            }
        });
        Self { rx, task }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unreal_rtsp::Frame;

    #[tokio::test]
    async fn backlog_byte_budget_and_drop_bound_memory_and_release_subscriptions() {
        let (video, rx) = broadcast::channel(32);
        let (audio, audio_rx) = broadcast::channel(128);
        let input = Input::new(rx, audio_rx, false, CancellationToken::new());
        for index in 0..20 {
            assert!(video
                .send(Arc::new(SharedFrame::new(Frame {
                    avcc: vec![0; 1024 * 1024],
                    timestamp_90k: index,
                    duration_90k: 1,
                    keyframe: true,
                })))
                .is_ok());
            tokio::task::yield_now().await;
        }
        assert_eq!(input.rx.len(), 15);
        drop(input);
        tokio::task::yield_now().await;
        assert_eq!(video.receiver_count(), 0);
        assert_eq!(audio.receiver_count(), 0);
    }

    #[tokio::test]
    async fn disk_wait_does_not_overrun_the_live_broadcast_or_lose_eof_tail() {
        let (video, slow_reader) = broadcast::channel(32);
        let mut slow_reader = slow_reader;
        let (audio, audio_rx) = broadcast::channel(128);
        let stopped = CancellationToken::new();
        let mut buffered = Input::new(video.subscribe(), audio_rx, true, stopped.clone());
        // The writer remains blocked throughout this burst, just as during fsync.
        for index in 0..240 {
            assert!(video
                .send(Arc::new(SharedFrame::new(Frame {
                    avcc: vec![1; 1024],
                    timestamp_90k: index * 6000,
                    duration_90k: 6000,
                    keyframe: index % 15 == 0,
                })))
                .is_ok());
            for sample in index * 2..index * 2 + 2 {
                audio
                    .send(Arc::new(AudioFrame {
                        data: vec![1; 64],
                        timestamp: sample * 1024,
                        duration: 1024,
                        timestamp_90k: sample * 1920,
                    }))
                    .unwrap();
            }
            tokio::task::yield_now().await;
        }
        assert!(matches!(
            slow_reader.recv().await,
            Err(broadcast::error::RecvError::Lagged(_))
        ));
        stopped.cancel();
        let (mut videos, mut audios) = (0, 0);
        for _ in 0..720 {
            let queued = buffered.rx.recv().await.unwrap().unwrap();
            match queued.sample {
                Sample::Video { frame, .. } => {
                    assert_eq!(frame.timestamp_90k, videos * 6000);
                    videos += 1;
                }
                Sample::Audio(frame) => {
                    assert_eq!(frame.timestamp, audios * 1024);
                    audios += 1;
                }
            }
        }
        assert_eq!((videos, audios), (240, 480));
        assert!(buffered.rx.recv().await.unwrap().is_err());
        drop(audio);
        drop(buffered);
        tokio::task::yield_now().await;
        assert_eq!(video.receiver_count(), 1);
    }
}
