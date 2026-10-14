//! Synthetic-test audio/video verification; never prints source URLs or credentials.
use anyhow::{ensure, Result};
use unreal_rtsp::{Client, MediaEvent};
#[tokio::main]
async fn main() -> Result<()> {
    let address = std::env::var("RTSP_TEST_URL")?;
    let mut client = Client::connect(&address).await?;
    let audio = client
        .audio()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("No supported audio track"))?;
    let mut videos = 0;
    let mut audios = 0;
    let mut audio_bytes = 0;
    let mut last = None;
    let mut first_video = None;
    let mut first_audio = None;
    let mut last_video = 0;
    let mut steady_video = None;
    let mut steady_audio = None;
    let seconds = std::env::var("RTSP_TEST_SECONDS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
        .min(120);
    let started = std::time::Instant::now();
    tokio::time::timeout(std::time::Duration::from_secs(seconds + 20), async {
        while videos < 30 || audios < 60 || started.elapsed().as_secs() < seconds {
            match client.next_media().await? {
                MediaEvent::Video(f) => {
                    videos += 1;
                    first_video.get_or_insert(f.timestamp_90k);
                    last_video = f.timestamp_90k;
                    if started.elapsed().as_secs() >= 5 {
                        steady_video.get_or_insert(f.timestamp_90k);
                    }
                }
                MediaEvent::Audio(f) => {
                    ensure!(
                        last.is_none_or(|ts| f.timestamp_90k >= ts),
                        "Audio timestamp went backwards"
                    );
                    last = Some(f.timestamp_90k);
                    first_audio.get_or_insert(f.timestamp_90k);
                    audio_bytes += f.data.len();
                    audios += 1;
                    if started.elapsed().as_secs() >= 5 {
                        steady_audio.get_or_insert(f.timestamp_90k);
                    }
                }
            }
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    println!("codec={:?} rate={} channels={} video_frames={} audio_frames={} audio_bytes={} first_video_90k={} first_audio_90k={}",audio.codec,audio.clock_rate,audio.channels,videos,audios,audio_bytes,first_video.unwrap(),first_audio.unwrap());
    let (synced, correction) = client.audio_sync();
    println!("wall_seconds={:.3} video_span_seconds={:.3} audio_span_seconds={:.3} sync={} correction_ms={:.3}",started.elapsed().as_secs_f64(),(last_video-first_video.unwrap()) as f64/90000.0,(last.unwrap()-first_audio.unwrap()) as f64/90000.0,if synced {"sender_reports"}else{"first_arrival"},correction as f64/90.0);
    if let (Some(video), Some(audio)) = (steady_video, steady_audio) {
        let video_span = (last_video - video) as f64 / 90000.0;
        let audio_span = (last.unwrap() - audio) as f64 / 90000.0;
        println!("steady_video_seconds={video_span:.3} steady_audio_seconds={audio_span:.3} duration_difference_ms={:.3}",(audio_span-video_span)*1000.0);
        if seconds >= 30 {
            ensure!(
                (audio_span - video_span).abs() < 0.25,
                "Steady audio/video clock span diverged >250ms"
            );
        }
    }
    Ok(())
}
