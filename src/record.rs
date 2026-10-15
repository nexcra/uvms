//! Append-only fragmented MP4 segments. Publish by atomic rename after sync.
//! Budget is global and persistent across restarts; never deletes recordings.
use crate::{
    archive::{Clip, Control},
    App,
};
use anyhow::{bail, Result};
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::{watch, Mutex},
};
use unreal_mp4::Muxer;
#[derive(Default)]
pub struct Status {
    states: Mutex<HashMap<String, String>>,
    used: AtomicU64,
    unfinished: AtomicU64,
    controls: Mutex<HashMap<String, watch::Sender<Control>>>,
    control_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}
#[derive(Debug)]
struct StorageFull;
impl std::fmt::Display for StorageFull {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("recording budget reached")
    }
}
impl std::error::Error for StorageFull {}
impl Status {
    pub async fn get(&self, id: &str) -> String {
        self.states
            .lock()
            .await
            .get(id)
            .cloned()
            .unwrap_or_else(|| "off".into())
    }
    pub fn used_bytes(&self) -> u64 {
        self.used.load(Ordering::SeqCst)
    }
    pub fn unfinished_files(&self) -> u64 {
        self.unfinished.load(Ordering::Relaxed)
    }
    pub async fn active(&self, id: &str, default: bool) -> bool {
        self.controls
            .lock()
            .await
            .get(id)
            .is_some_and(|tx| tx.borrow().active(default, now_ms()))
    }
    async fn set(&self, id: &str, s: &str, events: &crate::operations::Events) {
        let previous = self.states.lock().await.insert(id.into(), s.into());
        if previous.as_deref() != Some(s) {
            events.emit(Some(id), "recording", s);
        }
    }
    fn reserve(&self, n: u64, limit: u64) -> Result<()> {
        self.used
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |v| {
                v.checked_add(n).filter(|sum| *sum <= limit)
            })
            .map_err(|_| StorageFull)?;
        Ok(())
    }
    fn release(&self, n: u64) {
        self.used.fetch_sub(n, Ordering::SeqCst);
    }
}
pub async fn start(app: Arc<App>) -> Result<Vec<tokio::task::JoinHandle<()>>> {
    tokio::fs::create_dir_all(&app.config.recording_dir).await?;
    app.archive.initialize(&app.config.recording_dir).await?;
    let mut used = 0u64;
    let mut dirs = tokio::fs::read_dir(&app.config.recording_dir).await?;
    while let Some(dir) = dirs.next_entry().await? {
        if !dir.file_type().await?.is_dir() {
            continue;
        }
        let id = dir.file_name().to_string_lossy().into_owned();
        let indexed = if app.config.streams.contains_key(&id) {
            app.archive.indexed_files(&id).await?
        } else {
            HashMap::new()
        };
        let mut files = tokio::fs::read_dir(dir.path()).await?;
        while let Some(f) = files.next_entry().await? {
            if f.file_type().await?.is_file() {
                let bytes = f.metadata().await?.len();
                used = used.saturating_add(bytes);
                let name = f.file_name().to_string_lossy().into_owned();
                if name.ends_with(".partial") {
                    app.recordings.unfinished.fetch_add(1, Ordering::Relaxed);
                    app.events
                        .emit(Some(&id), "archive", "unfinished_file_preserved");
                }
                if indexed.get(&name) == Some(&bytes) {
                    continue;
                }
                if safe_filename(&name) && app.config.streams.contains_key(&id) {
                    if let Some(start_ms) =
                        name.split('-').next().and_then(|s| s.parse::<u64>().ok())
                    {
                        let duration = legacy_duration(&f.path()).await.ok();
                        app.archive
                            .insert(Clip {
                                stream: id.clone(),
                                file: name,
                                start_ms,
                                end_ms: start_ms.saturating_add(duration.unwrap_or(0)),
                                bytes,
                                estimated: duration.is_none(),
                            })
                            .await?;
                    }
                }
            }
        }
    }
    app.recordings.used.store(used, Ordering::SeqCst);
    let mut tasks = vec![];
    for (id, s) in &app.config.streams {
        let default = s.record;
        let (tx, mut control) = watch::channel(app.archive.control(id).await?);
        app.recordings.controls.lock().await.insert(id.clone(), tx);
        app.recordings
            .control_locks
            .lock()
            .await
            .insert(id.clone(), Arc::new(Mutex::new(())));
        let dir = PathBuf::from(&app.config.recording_dir).join(id);
        tokio::fs::create_dir_all(&dir).await?;
        if !tokio::fs::symlink_metadata(&dir).await?.is_dir() {
            bail!("recording camera directory must not be a symlink")
        }
        let app = app.clone();
        let id = id.clone();
        tasks.push(tokio::spawn(async move {
            loop {
                if app.shutdown.is_cancelled(){break}
                if !control.borrow().active(default,now_ms()) {
                    let state=if matches!(control.borrow().mode,crate::archive::Mode::Off)||control.borrow().schedules.is_empty(){"off"}else{"scheduled"};
                    app.recordings.set(&id,state, &app.events).await;
                    tokio::select!{_=app.shutdown.cancelled()=>break,_=control.changed()=>{},_=tokio::time::sleep(Duration::from_millis(250))=>{}}
                    continue;
                }
                app.recordings.set(&id,"connecting", &app.events).await;
                let outcome=record_source(&app,&id,&dir,&mut control,default).await;
                if app.shutdown.is_cancelled(){app.recordings.set(&id,"stopped", &app.events).await;break}
                if outcome.as_ref().is_err_and(|e|e.is::<StorageFull>()) {
                    app.recordings.set(&id,"storage_full", &app.events).await;
                    tracing::warn!(stream=%id,"Recording budget reached; existing files preserved");
                    tokio::select!{_=app.shutdown.cancelled()=>break,_=control.changed()=>{}}
                    continue;
                }
                if let Err(error)=&outcome{app.recordings.set(&id,"retrying", &app.events).await;let backlog=error.downcast_ref::<tokio::sync::broadcast::error::RecvError>().map(|e|e.to_string());tracing::warn!(stream=%id,backlog,"Recording interrupted; check source and storage");}
                if outcome.is_ok(){continue}
                tokio::select!{_=app.shutdown.cancelled()=>break,_=control.changed()=>{},_=tokio::time::sleep(Duration::from_secs(3))=>{}}
            }
        }));
    }
    Ok(tasks)
}
struct Segment {
    file: tokio::fs::File,
    partial: PathBuf,
    final_path: PathBuf,
    mux: Muxer,
    base: u64,
    written: u64,
    start_ms: u64,
    end_ms: u64,
}
impl Segment {
    async fn finish(mut self, app: &App, id: &str) -> Result<()> {
        self.file.flush().await?;
        self.file.sync_all().await?;
        drop(self.file);
        tokio::fs::rename(self.partial, &self.final_path).await?;
        app.archive
            .insert(Clip {
                stream: id.into(),
                file: self
                    .final_path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                start_ms: self.start_ms,
                end_ms: self.end_ms,
                bytes: self.written,
                estimated: false,
            })
            .await?;
        Ok(())
    }
}
// A failed write may leave a prefix on disk. Charge that prefix and release only
// bytes proven absent after flushing the file's asynchronous write buffer.
async fn release_unwritten(
    file: &mut tokio::fs::File,
    before: u64,
    reserved: u64,
    status: &Status,
) {
    let _ = file.flush().await;
    if let Ok(meta) = file.metadata().await {
        let written = meta.len().saturating_sub(before).min(reserved);
        status.release(reserved - written);
    }
}
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as u64
}
// Import only bounded box headers and moof metadata, never load old media into
// RAM or infer its duration from today's recording configuration.
async fn legacy_duration(path: &std::path::Path) -> Result<u64> {
    let mut f = tokio::fs::File::open(path).await?;
    let size = f.metadata().await?.len();
    let mut pos = 0;
    let mut end = 0;
    while pos + 8 <= size {
        let mut header = [0; 8];
        f.read_exact(&mut header).await?;
        let len = u32::from_be_bytes(header[..4].try_into().unwrap()) as u64;
        if len < 8 || len > size - pos {
            bail!("unsupported MP4 box")
        }
        if &header[4..] == b"moof" {
            if len > 1024 * 1024 {
                bail!("oversized metadata")
            };
            let mut body = vec![0; (len - 8) as usize];
            f.read_exact(&mut body).await?;
            end = end.max(moof_end(&body)?);
        }
        pos += len;
        f.seek(std::io::SeekFrom::Start(pos)).await?;
    }
    if pos != size || end == 0 {
        bail!("unknown MP4 duration")
    };
    Ok(end / 90)
}
fn boxes(mut b: &[u8]) -> Result<Vec<(&[u8], &[u8])>> {
    let mut out = vec![];
    while !b.is_empty() {
        if b.len() < 8 {
            bail!("short box")
        };
        let n = u32::from_be_bytes(b[..4].try_into().unwrap()) as usize;
        if n < 8 || n > b.len() {
            bail!("invalid box")
        };
        out.push((&b[4..8], &b[8..n]));
        b = &b[n..];
    }
    Ok(out)
}
fn moof_end(body: &[u8]) -> Result<u64> {
    let mut end = 0;
    for (kind, traf) in boxes(body)? {
        if kind != b"traf" {
            continue;
        }
        let mut track = 0;
        let mut base = 0;
        let mut duration = 0;
        for (kind, b) in boxes(traf)? {
            if kind == b"tfhd" && b.len() >= 8 {
                track = u32::from_be_bytes(b[4..8].try_into().unwrap());
            }
            if kind == b"tfdt" {
                base = match (b.first(), b.len()) {
                    (Some(1), n) if n >= 12 => u64::from_be_bytes(b[4..12].try_into().unwrap()),
                    (Some(0), n) if n >= 8 => {
                        u32::from_be_bytes(b[4..8].try_into().unwrap()) as u64
                    }
                    _ => bail!("invalid tfdt"),
                };
            }
            if kind == b"trun" {
                if b.len() < 8 {
                    bail!("invalid trun")
                };
                let flags = u32::from_be_bytes(b[..4].try_into().unwrap()) & 0xffffff;
                let count = u32::from_be_bytes(b[4..8].try_into().unwrap());
                if count > 100000 || flags & 0x100 == 0 {
                    bail!("missing sample duration")
                };
                let mut pos =
                    8 + if flags & 1 != 0 { 4 } else { 0 } + if flags & 4 != 0 { 4 } else { 0 };
                for _ in 0..count {
                    if pos + 4 > b.len() {
                        bail!("short trun")
                    };
                    duration += u32::from_be_bytes(b[pos..pos + 4].try_into().unwrap()) as u64;
                    pos += 4;
                    for flag in [0x200, 0x400, 0x800] {
                        if flags & flag != 0 {
                            pos += 4;
                        }
                    }
                    if pos > b.len() {
                        bail!("short sample")
                    }
                }
            }
        }
        if track == 1 {
            end = end.max(base.saturating_add(duration));
        }
    }
    Ok(end)
}
async fn record_source(
    app: &App,
    id: &str,
    dir: &std::path::Path,
    control: &mut watch::Receiver<Control>,
    default: bool,
) -> Result<()> {
    let feed = tokio::select! {_=app.shutdown.cancelled()=>return Ok(()),r=app.hub.acquire(id,&app.config,app.shutdown.clone())=>r?};
    let v = feed.video();
    let audio = feed.archive_audio();
    let mut input = crate::record_input::Input::new(
        feed.subscribe(),
        feed.subscribe_archive_audio(),
        audio.is_some(),
        feed.stopped.clone(),
    );
    let mut segment: Option<Segment> = None;
    let result: Result<()> = async {
        loop {
            if !control.borrow().active(default, now_ms()) {
                break;
            }
            let queued = tokio::select! {
                _=app.shutdown.cancelled()=>break,
                _=control.changed()=>continue,
                _=tokio::time::sleep(Duration::from_millis(250))=>continue,
                r=input.rx.recv()=>r.ok_or_else(||anyhow::anyhow!("recording reader ended"))??
            };
            let (f, received_ms) = match &queued.sample {
                crate::record_input::Sample::Audio(a) => {
                    if let Some(s) = segment.as_mut() {
                        if a.timestamp_90k >= s.base {
                            let timestamp = (a.timestamp_90k - s.base)
                                .saturating_mul(audio.as_ref().unwrap().clock_rate as u64)
                                / 90000;
                            let b = s.mux.audio_fragment(&a.data, timestamp, a.duration)?;
                            app.recordings
                                .reserve(b.len() as u64, app.config.recording_limit_bytes)?;
                            if let Err(e) = s.file.write_all(&b).await {
                                release_unwritten(
                                    &mut s.file,
                                    s.written,
                                    b.len() as u64,
                                    &app.recordings,
                                )
                                .await;
                                segment.take();
                                return Err(e.into());
                            }
                            s.written += b.len() as u64;
                            let audio_end = s.start_ms
                                + (a.timestamp_90k - s.base) / 90
                                + a.duration as u64 * 1000
                                    / audio.as_ref().unwrap().clock_rate as u64;
                            s.end_ms = s.end_ms.max(audio_end);
                        }
                    }
                    continue;
                }
                crate::record_input::Sample::Video { frame, received_ms } => (frame, *received_ms),
            };
            if segment.as_ref().is_some_and(|s| {
                f.keyframe
                    && f.timestamp_90k.saturating_sub(s.base) >= app.config.segment_seconds * 90000
            }) {
                segment.take().unwrap().finish(app, id).await?;
            }
            if segment.is_none() {
                if !f.keyframe {
                    continue;
                }
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
                let first = mux.fragment(&f.avcc, 0, f.duration_90k, f.keyframe)?;
                let reserved = (mux.init_segment().len() + first.len()) as u64;
                let timestamp = received_ms;
                let stem = format!("{timestamp}-{}", uuid::Uuid::new_v4().simple());
                let partial = dir.join(format!("{stem}.partial"));
                let final_path = dir.join(format!("{stem}.mp4"));
                app.recordings
                    .reserve(reserved, app.config.recording_limit_bytes)?;
                let mut file = match tokio::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&partial)
                    .await
                {
                    Ok(file) => file,
                    Err(e) => {
                        app.recordings.release(reserved);
                        return Err(e.into());
                    }
                };
                let initial: Result<()> = async {
                    file.write_all(mux.init_segment()).await?;
                    file.write_all(&first).await?;
                    Ok(())
                }
                .await;
                if let Err(e) = initial {
                    release_unwritten(&mut file, 0, reserved, &app.recordings).await;
                    return Err(e);
                }
                segment = Some(Segment {
                    file,
                    partial,
                    final_path,
                    mux,
                    base: f.timestamp_90k,
                    written: reserved,
                    start_ms: timestamp,
                    end_ms: timestamp + f.duration_90k as u64 / 90,
                });
                app.recordings.set(id, "recording", &app.events).await;
                continue;
            }
            let s = segment.as_mut().unwrap();
            let b = s.mux.fragment(
                &f.avcc,
                f.timestamp_90k.saturating_sub(s.base),
                f.duration_90k,
                f.keyframe,
            )?;
            app.recordings
                .reserve(b.len() as u64, app.config.recording_limit_bytes)?;
            // On disk failure retain .partial; a partial trailing box must not be published.
            if let Err(e) = s.file.write_all(&b).await {
                release_unwritten(&mut s.file, s.written, b.len() as u64, &app.recordings).await;
                segment.take();
                return Err(e.into());
            }
            s.written += b.len() as u64;
            s.end_ms = s
                .end_ms
                .max(s.start_ms + (f.timestamp_90k - s.base + f.duration_90k as u64) / 90);
        }
        Ok(())
    }
    .await;
    if let Some(s) = segment {
        s.finish(app, id).await?;
    }
    result
}
#[derive(Default, serde::Deserialize)]
pub struct Search {
    stream: Option<String>,
    from: Option<u64>,
    to: Option<u64>,
    limit: Option<usize>,
    offset: Option<usize>,
}
pub async fn list(State(app): State<Arc<App>>, Query(q): Query<Search>) -> Response {
    if q.stream
        .as_ref()
        .is_some_and(|s| !app.config.streams.contains_key(s))
        || q.from.unwrap_or(0) > q.to.unwrap_or(u64::MAX)
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    match app
        .archive
        .search(
            q.stream,
            q.from.unwrap_or(0),
            q.to.unwrap_or(i64::MAX as u64),
            q.limit.unwrap_or(200).clamp(1, 1000),
            q.offset.unwrap_or(0).min(1_000_000),
        )
        .await
    {
        Ok(clips) => {
            let mut files = vec![];
            for c in clips {
                let root = PathBuf::from(&app.config.recording_dir).join(&c.stream);
                if !app.config.streams.contains_key(&c.stream)
                    || !safe_filename(&c.file)
                    || !tokio::fs::symlink_metadata(&root)
                        .await
                        .is_ok_and(|m| m.is_dir())
                    || !tokio::fs::symlink_metadata(root.join(&c.file))
                        .await
                        .is_ok_and(|m| m.is_file())
                {
                    continue;
                }
                files.push(json!({"url":format!("/api/recordings/{}/{}",c.stream,c.file),"stream":c.stream,"file":c.file,"start_ms":c.start_ms,"end_ms":c.end_ms,"bytes":c.bytes,"estimated":c.estimated}));
            }
            Json(json!({"files":files,"used_bytes":app.recordings.used.load(Ordering::SeqCst),"limit_bytes":app.config.recording_limit_bytes})).into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
pub async fn control_status(State(app): State<Arc<App>>, Path(id): Path<String>) -> Response {
    let Some(stream) = app.config.streams.get(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(c) = app.archive.control(&id).await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    Json(json!({"mode":c.mode,"schedules":c.schedules,"active":c.active(stream.record,now_ms()),"status":app.recordings.get(&id).await})).into_response()
}
pub async fn control(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(c): Json<Control>,
) -> Response {
    if !crate::same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !app.config.streams.contains_key(&id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    if c.validate().is_err() {
        return (StatusCode::BAD_REQUEST, "invalid recording schedule").into_response();
    }
    let Some(lock) = app.recordings.control_locks.lock().await.get(&id).cloned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let _guard = lock.lock().await;
    if app.archive.save_control(&id, c.clone()).await.is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    app.events.emit(
        Some(&id),
        "recording_control",
        match c.mode {
            crate::archive::Mode::On => "on",
            crate::archive::Mode::Off => "off",
            crate::archive::Mode::Auto => "auto",
        },
    );
    if let Some(tx) = app.recordings.controls.lock().await.get(&id) {
        tx.send_replace(c);
    }
    control_status(State(app), Path(id)).await
}
pub(crate) fn safe_filename(s: &str) -> bool {
    s.ends_with(".mp4")
        && s.len() < 96
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() || b"- .mp".contains(&b))
        && !s.contains("..")
        && !s.contains(' ')
}
pub async fn download(
    State(app): State<Arc<App>>,
    Path((id, file)): Path<(String, String)>,
    headers: HeaderMap,
    Query(q): Query<Download>,
) -> Response {
    if !app.config.streams.contains_key(&id) || !safe_filename(&file) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let root = PathBuf::from(&app.config.recording_dir).join(id);
    if !tokio::fs::symlink_metadata(&root)
        .await
        .is_ok_and(|m| m.is_dir())
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    let path = root.join(&file);
    if !tokio::fs::symlink_metadata(&path)
        .await
        .is_ok_and(|m| m.is_file())
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Ok(mut f) = tokio::fs::File::open(path).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let size = f.metadata().await.map(|m| m.len()).unwrap_or(0);
    let range = match headers.get(header::RANGE) {
        Some(v) => match v.to_str().ok().and_then(|s| byte_range(s, size).ok()) {
            Some(r) => Some(r),
            None => {
                return Response::builder()
                    .status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header(header::CONTENT_RANGE, format!("bytes */{size}"))
                    .body(Body::empty())
                    .unwrap()
            }
        },
        None => None,
    };
    let (start, end) = range.unwrap_or((0, size.saturating_sub(1)));
    let len = if size == 0 { 0 } else { end - start + 1 };
    if f.seek(std::io::SeekFrom::Start(start)).await.is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let mut response = Response::builder()
        .status(if range.is_some() {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(header::CONTENT_TYPE, "video/mp4")
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, len)
        .header(
            header::CONTENT_DISPOSITION,
            format!(
                "{}; filename=\"{file}\"",
                if q.download == Some(1) {
                    "attachment"
                } else {
                    "inline"
                }
            ),
        );
    if range.is_some() {
        response = response.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}"))
    }
    response
        .body(Body::from_stream(tokio_util::io::ReaderStream::new(
            f.take(len),
        )))
        .unwrap()
}
#[derive(Default, serde::Deserialize)]
pub struct Download {
    download: Option<u8>,
}
fn byte_range(value: &str, size: u64) -> Result<(u64, u64)> {
    let range = value
        .strip_prefix("bytes=")
        .ok_or_else(|| anyhow::anyhow!("range unit"))?;
    let (a, b) = range
        .split_once('-')
        .ok_or_else(|| anyhow::anyhow!("range syntax"))?;
    if size == 0 || range.contains(',') {
        bail!("invalid range")
    }
    if a.is_empty() {
        let suffix: u64 = b.parse()?;
        if suffix == 0 {
            bail!("empty range")
        }
        return Ok((size.saturating_sub(suffix), size - 1));
    }
    let start: u64 = a.parse()?;
    let end = if b.is_empty() {
        size - 1
    } else {
        b.parse::<u64>()?.min(size - 1)
    };
    if start >= size || start > end {
        bail!("unsatisfiable range")
    }
    Ok((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn legacy_clip_duration_comes_from_media_not_current_segment_setting() {
        let path = std::env::temp_dir().join(format!("uvms-duration-{}.mp4", uuid::Uuid::new_v4()));
        let mut mux = Muxer::new(&[0x67, 0x42, 0xe0, 0x1e], &[0x68, 0xce], 640, 360).unwrap();
        let mut bytes = mux.init_segment().to_vec();
        bytes.extend(
            mux.fragment(&[0, 0, 0, 2, 0x65, 0xb0], 0, 9000, true)
                .unwrap(),
        );
        bytes.extend(
            mux.fragment(&[0, 0, 0, 2, 0x41, 0xc0], 9000, 9000, false)
                .unwrap(),
        );
        tokio::fs::write(&path, &bytes).await.unwrap();
        assert_eq!(legacy_duration(&path).await.unwrap(), 200);
        bytes.truncate(bytes.len() - 2);
        tokio::fs::write(&path, bytes).await.unwrap();
        assert!(legacy_duration(&path).await.is_err());
        tokio::fs::remove_file(path).await.unwrap();
    }
    #[test]
    fn http_ranges_are_bounded() {
        assert_eq!(byte_range("bytes=2-4", 10).unwrap(), (2, 4));
        assert_eq!(byte_range("bytes=8-", 10).unwrap(), (8, 9));
        assert_eq!(byte_range("bytes=-3", 10).unwrap(), (7, 9));
        assert_eq!(byte_range("bytes=0-99", 10).unwrap(), (0, 9));
        for v in [
            "bytes=10-",
            "bytes=4-2",
            "bytes=-0",
            "bytes=0-1,3-4",
            "items=0-1",
        ] {
            assert!(byte_range(v, 10).is_err());
        }
        assert!(byte_range("bytes=0-", 0).is_err());
    }
    #[test]
    fn recording_budget_cannot_overflow_or_overcommit() {
        let state = Status::default();
        state.reserve(9, 10).unwrap();
        assert!(state.reserve(2, 10).is_err());
        assert_eq!(state.used.load(Ordering::SeqCst), 9);
        assert!(state.reserve(u64::MAX, 10).is_err());
        assert!(state.reserve(2, 10).unwrap_err().is::<StorageFull>());
    }
    #[tokio::test]
    async fn failed_append_releases_only_bytes_absent_from_partial_file() {
        let path =
            std::env::temp_dir().join(format!("uvms-accounting-{}.partial", uuid::Uuid::new_v4()));
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
            .unwrap();
        let state = Status::default();
        state.reserve(5, 100).unwrap();
        file.write_all(&[0; 5]).await.unwrap();
        state.reserve(10, 100).unwrap();
        // Model a failed ten-byte append that leaves three bytes on disk.
        file.write_all(&[1; 3]).await.unwrap();
        release_unwritten(&mut file, 5, 10, &state).await;
        assert_eq!(state.used.load(Ordering::SeqCst), 8);
        assert_eq!(file.metadata().await.unwrap().len(), 8);
        drop(file);
        tokio::fs::remove_file(path).await.unwrap();
    }
    #[test]
    fn download_filename_rejects_traversal() {
        for n in [
            "../a.mp4",
            "a/b.mp4",
            "a\\b.mp4",
            "..mp4",
            "a.partial",
            "a.mp4?x",
        ] {
            assert!(!safe_filename(n));
        }
        assert!(safe_filename("1730-abcdef0123.mp4"));
    }
}
