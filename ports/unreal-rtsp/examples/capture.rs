use anyhow::{ensure, Result};
use std::{fs, time::Duration};
use unreal_rtsp::Client;

#[tokio::main]
async fn main() -> Result<()> {
    let url =
        std::env::var("RTSP_TEST_URL").unwrap_or_else(|_| "rtsp://127.0.0.1:8554/camera".into());
    let output = std::env::var("RTSP_CAPTURE_DIR").ok();
    let mut client = Client::connect(&url).await?;
    let video = client.video();
    println!(
        "Video {}x{}, SPS {} bytes, PPS {} bytes",
        video.width,
        video.height,
        video.sps.len(),
        video.pps.len()
    );
    let mut count = 0;
    let mut keyframes = 0;
    let mut total = 0;
    let mut previous = None;
    if let Some(dir) = &output {
        fs::create_dir_all(dir)?;
        fs::write(format!("{dir}/sps.bin"), &video.sps)?;
        fs::write(format!("{dir}/pps.bin"), &video.pps)?;
    }
    let frames = std::env::var("RTSP_CAPTURE_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(40);
    while count < frames {
        let frame = tokio::time::timeout(Duration::from_secs(15), client.next_frame()).await??;
        ensure!(frame.duration_90k > 0, "Zero duration");
        if let Some(t) = previous {
            ensure!(frame.timestamp_90k > t, "Non-monotonic frame timestamp");
        }
        previous = Some(frame.timestamp_90k);
        let mut offset = 0;
        let mut nals = Vec::new();
        while offset < frame.avcc.len() {
            ensure!(offset + 4 <= frame.avcc.len(), "Truncated length");
            let n = u32::from_be_bytes(frame.avcc[offset..offset + 4].try_into()?) as usize;
            offset += 4;
            ensure!(n > 0 && offset + n <= frame.avcc.len(), "Invalid AVCC NAL");
            nals.push(frame.avcc[offset] & 31);
            offset += n;
        }
        if frame.keyframe {
            ensure!(
                nals.contains(&7) && nals.contains(&8) && nals.contains(&5),
                "IDR lacks parameter sets"
            );
            keyframes += 1;
        }
        if let Some(dir) = &output {
            fs::write(format!("{dir}/frame-{count:03}.bin"), &frame.avcc)?;
        }
        println!(
            "frame={count} timestamp={} duration={} key={} bytes={} nals={nals:?}",
            frame.timestamp_90k,
            frame.duration_90k,
            frame.keyframe,
            frame.avcc.len()
        );
        total += frame.avcc.len();
        count += 1;
    }
    ensure!(keyframes > 0, "No IDR received");
    println!("PASS frames={count} keyframes={keyframes} bytes={total}");
    Ok(())
}
