//! Bounded, per-camera fMP4 HLS/DVR cache. This reuses the Rust MP4 mux; the
//! original documented TS HLS output is not a claim of identical mux code.
use crate::{record::now_ms, App};
use anyhow::{bail, Result};
use axum::{
    extract::{Path, Query, State as AxumState},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use bytes::Bytes;
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Mutex;
use unreal_mp4::Muxer;

#[derive(Default)]
pub struct State {
    cameras: Mutex<HashMap<String, Arc<Mutex<Cache>>>>,
}
#[derive(Default)]
struct Cache {
    clips: VecDeque<Clip>,
    bytes: usize,
    next: u64,
    epoch: u64,
    target: u64,
    error: Option<String>,
}
struct Clip {
    sequence: u64,
    epoch: u64,
    start_ms: u64,
    duration: f64,
    init: Bytes,
    data: Bytes,
}
impl Cache {
    fn push(&mut self, mut clip: Clip, window: u64, limit: usize) {
        clip.sequence = self.next;
        self.next += 1;
        if self.target == 0 {
            self.target = clip.duration.round().max(1.) as u64;
        }
        self.bytes += clip.data.len() + clip.init.len();
        self.clips.push_back(clip);
        while self.clips.len() > 1
            && (self.bytes > limit
                || self
                    .clips
                    .back()
                    .unwrap()
                    .start_ms
                    .saturating_sub(self.clips.front().unwrap().start_ms)
                    > window * 1000)
        {
            self.evict();
        }
        if self.bytes > limit {
            self.evict();
            self.error = Some("HLS segment exceeds cache budget".into());
        }
    }
    fn evict(&mut self) {
        if let Some(c) = self.clips.pop_front() {
            self.bytes -= c.init.len() + c.data.len();
        }
    }
    // RFC8216 clients select a live start at least three target durations
    // behind the edge. A one-segment cold playlist has no such start; native
    // demuxers can reject it rather than waiting for the playlist to grow.
    fn live_start(&self) -> Option<usize> {
        let required = 3. * self.target.max(1) as f64;
        let mut duration = 0.;
        for (index, c) in self.clips.iter().enumerate().rev() {
            duration += c.duration;
            if duration + 0.000001 >= required {
                return Some(index);
            }
        }
        None
    }
    fn playlist(&self, from: Option<u64>) -> Option<String> {
        let live_start = self.live_start()?;
        let start = from
            .map(|time| {
                self.clips
                    .iter()
                    .position(|c| c.start_ms + (c.duration * 1000.) as u64 > time)
                    .unwrap_or(self.clips.len() - 1)
            })
            .map(|index| index.min(live_start))
            .unwrap_or(live_start);
        let first = &self.clips[start];
        let mut out=format!("#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:{}\n#EXT-X-MEDIA-SEQUENCE:{}\n#EXT-X-DISCONTINUITY-SEQUENCE:{}\n#EXT-X-INDEPENDENT-SEGMENTS\n",self.target.max(1),first.sequence,first.epoch);
        let mut epoch = None;
        for c in self.clips.iter().skip(start) {
            if epoch != Some(c.epoch) {
                if epoch.is_some() {
                    out.push_str("#EXT-X-DISCONTINUITY\n")
                };
                out.push_str(&format!("#EXT-X-MAP:URI=\"init-{}.mp4\"\n", c.epoch));
                epoch = Some(c.epoch);
            }
            if let Some(date) = chrono::DateTime::from_timestamp_millis(c.start_ms as i64) {
                out.push_str(&format!(
                    "#EXT-X-PROGRAM-DATE-TIME:{}\n",
                    date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                ));
            }
            out.push_str(&format!(
                "#EXTINF:{:.6},\nsegment-{}.m4s\n",
                c.duration, c.sequence
            ));
        }
        if from.is_some() {
            out.push_str("#EXT-X-START:TIME-OFFSET=0,PRECISE=YES\n");
        }
        Some(out)
    }
}
async fn ensure(app: &Arc<App>, id: &str) -> Arc<Mutex<Cache>> {
    let mut cameras = app.hls.cameras.lock().await;
    if let Some(c) = cameras.get(id) {
        return c.clone();
    }
    // A restart must not reuse media URLs while a player still caches old init
    // and segment responses from the previous service process.
    let cache = Arc::new(Mutex::new(Cache {
        next: now_ms(),
        epoch: now_ms(),
        target: app.config.hls_segment_seconds,
        ..Cache::default()
    }));
    cameras.insert(id.into(), cache.clone());
    let (app, id, cache_task) = (app.clone(), id.to_owned(), cache.clone());
    tokio::spawn(async move {
        while !app.shutdown.is_cancelled() {
            if run(&app, &id, &cache_task).await.is_err() {
                cache_task.lock().await.error = Some("camera or media unavailable".into());
            }
            tokio::select! {_=app.shutdown.cancelled()=>break,_=tokio::time::sleep(Duration::from_secs(2))=>{}}
        }
    });
    cache
}
async fn run(app: &App, id: &str, cache: &Mutex<Cache>) -> Result<()> {
    let feed = tokio::select! {_=app.shutdown.cancelled()=>return Ok(()),r=app.hub.acquire(id,&app.config,app.shutdown.clone())=>r?};
    let v = feed.video();
    let mut rx = feed.subscribe();
    let audio = feed.archive_audio();
    let mut audio_rx = feed.subscribe_archive_audio();
    let mut mux = Muxer::new_av(
        &v.sps,
        &v.pps,
        v.width,
        v.height,
        audio.as_ref().map(|a| unreal_mp4::AudioConfig {
            sample_rate: a.clock_rate,
            channels: a.channels,
            decoder_config: a.config.clone(),
        }),
    )?;
    let init = Bytes::copy_from_slice(mux.init_segment());
    let epoch = {
        let mut c = cache.lock().await;
        c.epoch += 1;
        c.epoch
    };
    let mut base: Option<u64> = None;
    let mut segment_start = 0;
    let mut wall = 0;
    let mut data = Vec::new();
    loop {
        let frame = tokio::select! {_=app.shutdown.cancelled()=>return Ok(()),_=feed.stopped.cancelled()=>bail!("source stopped"),
        a=audio_rx.recv(),if audio.is_some()=>{
            let a=a?;
                if let Some(base)=base {if a.timestamp_90k>=segment_start{
                let timestamp=(a.timestamp_90k-base).saturating_mul(audio.as_ref().unwrap().clock_rate as u64)/90000;
                let b=mux.audio_fragment(&a.data,timestamp,a.duration)?;
                if data.len().saturating_add(b.len()).saturating_add(init.len())>app.config.hls_limit_bytes as usize{bail!("HLS cache limit")}
                data.extend_from_slice(&b);
            }}continue;
        },r=rx.recv()=>r?};
        if base.is_none() {
            if !frame.keyframe {
                continue;
            }
            base = Some(frame.timestamp_90k);
            segment_start = frame.timestamp_90k;
            wall = now_ms();
        }
        if frame.keyframe
            && frame.timestamp_90k.saturating_sub(segment_start)
                >= app.config.hls_segment_seconds * 90000
            && !data.is_empty()
        {
            let duration = (frame.timestamp_90k - segment_start) as f64 / 90000.;
            if duration.round() as u64 > app.config.hls_segment_seconds {
                bail!("camera IDR interval exceeds configured HLS target duration")
            }
            let clip = Clip {
                sequence: 0,
                epoch,
                start_ms: wall,
                duration,
                init: init.clone(),
                data: Bytes::from(std::mem::take(&mut data)),
            };
            {
                let mut c = cache.lock().await;
                c.error = None;
                c.push(
                    clip,
                    app.config.hls_window_seconds,
                    app.config.hls_limit_bytes as usize,
                );
            }
            wall = wall.saturating_add((duration * 1000.) as u64);
            segment_start = frame.timestamp_90k;
        }
        let b = mux.fragment(
            &frame.avcc,
            frame.timestamp_90k - base.unwrap(),
            frame.duration_90k,
            frame.keyframe,
        )?;
        if data
            .len()
            .saturating_add(b.len())
            .saturating_add(init.len())
            > app.config.hls_limit_bytes as usize
        {
            bail!("HLS IDR segment exceeds cache limit")
        }
        data.extend_from_slice(&b);
        let last_end = frame.timestamp_90k + frame.duration_90k as u64;
        // RFC8216 target duration stays fixed for this playlist URI. Never
        // publish a segment whose rounded EXTINF exceeds the declared target.
        if last_end.saturating_sub(segment_start) >= app.config.hls_segment_seconds * 90000 + 45000
        {
            bail!("camera IDR interval exceeds configured HLS target duration")
        }
    }
}
#[derive(Default, Deserialize)]
pub struct Play {
    from: Option<u64>,
}
pub async fn playlist(
    AxumState(app): AxumState<Arc<App>>,
    Path(id): Path<String>,
    Query(q): Query<Play>,
) -> Response {
    if !app.config.streams.contains_key(&id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let cache = ensure(&app, &id).await;
    // Wait for a usable live window, not just the first complete segment. Allow
    // the bounded RTSP handshake plus three target durations on a cold source.
    let deadline = tokio::time::Instant::now()
        + Duration::from_secs((3 * app.config.hls_segment_seconds + 15).min(45));
    while tokio::time::Instant::now() < deadline {
        if let Some(text) = cache.lock().await.playlist(q.from) {
            return (
                [
                    (header::CONTENT_TYPE, "application/vnd.apple.mpegurl"),
                    (header::CACHE_CONTROL, "no-store"),
                ],
                text,
            )
                .into_response();
        }
        if app.shutdown.is_cancelled() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [(header::RETRY_AFTER, "2")],
        "Waiting for a complete camera playback window",
    )
        .into_response()
}
pub async fn status(AxumState(app): AxumState<Arc<App>>, Path(id): Path<String>) -> Response {
    if !app.config.streams.contains_key(&id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let cache = ensure(&app, &id).await;
    let c = cache.lock().await;
    Json(json!({"ready":c.live_start().is_some(),"oldest_ms":c.clips.front().map(|c|c.start_ms),"newest_ms":c.clips.back().map(|c|c.start_ms+(c.duration*1000.)as u64),"window_seconds":app.config.hls_window_seconds,"bytes":c.bytes,"error":c.error})).into_response()
}
pub async fn media(
    AxumState(app): AxumState<Arc<App>>,
    Path((id, file)): Path<(String, String)>,
) -> Response {
    let cameras = app.hls.cameras.lock().await;
    let Some(cache) = cameras.get(&id).cloned() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    drop(cameras);
    let c = cache.lock().await;
    let data = if let Some(epoch) = file
        .strip_prefix("init-")
        .and_then(|f| f.strip_suffix(".mp4"))
        .and_then(|f| f.parse::<u64>().ok())
    {
        c.clips
            .iter()
            .find(|c| c.epoch == epoch)
            .map(|c| c.init.clone())
    } else if let Some(seq) = file
        .strip_prefix("segment-")
        .and_then(|f| f.strip_suffix(".m4s"))
        .and_then(|f| f.parse::<u64>().ok())
    {
        c.clips
            .iter()
            .find(|c| c.sequence == seq)
            .map(|c| c.data.clone())
    } else {
        None
    };
    match data {
        Some(b) => (
            [
                (header::CONTENT_TYPE, "video/mp4"),
                (header::CACHE_CONTROL, "private,max-age=30"),
            ],
            b,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn clip(epoch: u64, start_ms: u64) -> Clip {
        Clip {
            sequence: 0,
            epoch,
            start_ms,
            duration: 2.,
            init: Bytes::from_static(b"init"),
            data: Bytes::from_static(b"media"),
        }
    }
    #[test]
    fn cold_playlist_waits_for_three_target_durations_not_one_clip() {
        let mut c = Cache {
            target: 2,
            ..Cache::default()
        };
        for i in 0..2 {
            c.push(clip(1, 100000 + i * 2000), 300, 1000);
            assert!(c.playlist(None).is_none());
        }
        c.push(clip(1, 104000), 300, 1000);
        assert!(c.playlist(None).is_some());
        // A near-edge DVR request must still offer a complete startup window.
        assert!(c
            .playlist(Some(105000))
            .unwrap()
            .contains("MEDIA-SEQUENCE:0"));
        let mut fractional = Cache {
            target: 2,
            ..Cache::default()
        };
        for i in 0..3 {
            let mut x = clip(1, i * 1900);
            x.duration = 1.9;
            fractional.push(x, 300, 1000);
        }
        assert!(fractional.playlist(None).is_none());
        fractional.push(clip(1, 5700), 300, 1000);
        assert!(fractional.playlist(None).is_some());
    }
    #[test]
    fn bounded_cache_playlist_sequences_discontinuity_and_dvr() {
        let mut c = Cache::default();
        for i in 0..8 {
            c.push(clip(if i < 5 { 1 } else { 2 }, 100000 + i * 2000), 10, 1000);
        }
        assert_eq!(c.clips.len(), 6);
        assert_eq!(c.clips.front().unwrap().sequence, 2);
        let p = c.playlist(Some(104000)).unwrap();
        assert!(p.contains("MEDIA-SEQUENCE:2"));
        assert!(p.contains("DISCONTINUITY-SEQUENCE:1"));
        assert!(p.contains("#EXT-X-DISCONTINUITY\n"));
        assert!(p.contains("init-2.mp4"));
        assert!(p.contains("PRECISE=YES"));
        let live = c.playlist(None).unwrap();
        assert!(live.contains("MEDIA-SEQUENCE:5"));
        c.push(clip(2, 116000), 10, 9);
        assert_eq!(c.clips.len(), 1);
        assert!(c.bytes <= 9);
        c.push(clip(2, 118000), 10, 1);
        assert!(c.clips.is_empty());
        assert!(c.playlist(None).is_none());
    }
}
