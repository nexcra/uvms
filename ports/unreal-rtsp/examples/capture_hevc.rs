//! Native RTP depacketization verification only, not a GPU conversion test.
use anyhow::{ensure, Result};
use std::io::Write;
#[tokio::main]
async fn main() -> Result<()> {
    let url = std::env::var("RTSP_TEST_URL")?;
    let path = std::env::var("HEVC_OUTPUT")?;
    let mut client = unreal_rtsp::Client::connect(&url).await?;
    ensure!(
        client.video_codec() == unreal_rtsp::VideoCodec::H265,
        "Expected H265 source"
    );
    let mut file = std::fs::File::create(path)?;
    let mut previous = None;
    for _ in 0..30 {
        let frame =
            tokio::time::timeout(std::time::Duration::from_secs(10), client.next_frame()).await??;
        ensure!(
            previous.is_none_or(|p| frame.timestamp_90k > p),
            "HEVC PTS reversed"
        );
        previous = Some(frame.timestamp_90k);
        let mut at = 0;
        while at < frame.avcc.len() {
            let n = u32::from_be_bytes(frame.avcc[at..at + 4].try_into()?) as usize;
            at += 4;
            file.write_all(&[0, 0, 0, 1])?;
            file.write_all(&frame.avcc[at..at + n])?;
            at += n;
        }
    }
    println!("HEVC native RTP capture: 30 frames, monotonically increasing PTS; GPU conversion NOT tested");
    Ok(())
}
