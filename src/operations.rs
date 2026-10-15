//! Bounded, asynchronous operational journal. No camera URLs or arbitrary errors
//! enter the journal. This is an engine event log, not motion detection.
use crate::{record::now_ms, App};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::mpsc;

const RETAIN: usize = 10_000;
#[derive(Clone, Debug, Serialize)]
pub struct Event {
    pub id: u64,
    pub time_ms: u64,
    pub stream: Option<String>,
    pub kind: String,
    pub state: String,
}
struct Inner {
    sender: mpsc::Sender<Event>,
    receiver: Mutex<Option<mpsc::Receiver<Event>>>,
    lost: AtomicU64,
    failed: AtomicBool,
    stop: tokio_util::sync::CancellationToken,
}
#[derive(Clone)]
pub struct Events(Arc<Inner>);
impl Default for Events {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel(256);
        Self(Arc::new(Inner {
            sender,
            receiver: Mutex::new(Some(receiver)),
            lost: AtomicU64::new(0),
            failed: AtomicBool::new(false),
            stop: tokio_util::sync::CancellationToken::new(),
        }))
    }
}
impl Events {
    pub fn stop(&self) {
        self.0.stop.cancel();
    }
    pub fn emit(&self, stream: Option<&str>, kind: &'static str, state: &str) {
        let event = Event {
            id: 0,
            time_ms: now_ms(),
            stream: stream.map(str::to_owned),
            kind: kind.into(),
            state: state.into(),
        };
        if self.0.sender.try_send(event).is_err() {
            self.0.lost.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub async fn start(&self, app: Arc<App>) -> anyhow::Result<tokio::task::JoinHandle<()>> {
        app.archive.run(|db| {
            db.execute_batch("CREATE TABLE IF NOT EXISTS events(id INTEGER PRIMARY KEY AUTOINCREMENT,time_ms INTEGER NOT NULL,stream TEXT,kind TEXT NOT NULL,state TEXT NOT NULL); CREATE INDEX IF NOT EXISTS events_stream_id ON events(stream,id);")?;
            Ok(())
        }).await?;
        let previous = app
            .archive
            .read(|db| {
                use rusqlite::OptionalExtension;
                Ok(db
                    .query_row(
                        "SELECT state FROM events WHERE kind='service' ORDER BY id DESC LIMIT 1",
                        [],
                        |r| r.get::<_, String>(0),
                    )
                    .optional()?)
            })
            .await?;
        if previous.is_some_and(|state| state != "stopped") {
            self.emit(None, "service", "previous_shutdown_unconfirmed");
        }
        let mut receiver = self
            .0
            .receiver
            .lock()
            .unwrap()
            .take()
            .expect("event worker started once");
        let events = self.clone();
        Ok(tokio::spawn(async move {
            loop {
                let first = tokio::select! {
                    value=receiver.recv()=>value,
                    _=events.0.stop.cancelled()=>{receiver.close();receiver.recv().await}
                };
                let Some(first) = first else { break };
                let mut batch = vec![first];
                while batch.len() < 64 {
                    match receiver.try_recv() {
                        Ok(event) => batch.push(event),
                        Err(_) => break,
                    }
                }
                let count = batch.len() as u64;
                let result = app.archive.run(move |db| {
                    let tx = db.transaction()?;
                    for e in batch {
                        tx.execute("INSERT INTO events(time_ms,stream,kind,state) VALUES(?1,?2,?3,?4)",params![e.time_ms as i64,e.stream,e.kind,e.state])?;
                    }
                    tx.execute("DELETE FROM events WHERE id <= (SELECT id FROM events ORDER BY id DESC LIMIT 1 OFFSET ?1)",[RETAIN as i64])?;
                    tx.commit()?;
                    Ok(())
                }).await;
                events.0.failed.store(result.is_err(), Ordering::Relaxed);
                if result.is_err() {
                    events.0.lost.fetch_add(count, Ordering::Relaxed);
                    tracing::warn!(
                        events = count,
                        "Operational journal write failed; see /api/status"
                    );
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            }
        }))
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventQuery {
    stream: Option<String>,
    after_id: Option<u64>,
    limit: Option<usize>,
}
pub async fn events(State(app): State<Arc<App>>, Query(q): Query<EventQuery>) -> Response {
    if q.stream
        .as_ref()
        .is_some_and(|s| !app.config.streams.contains_key(s))
        || q.after_id.is_some_and(|v| v > i64::MAX as u64)
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let limit = q.limit.unwrap_or(200).clamp(1, 1000);
    let after = q.after_id.unwrap_or(0);
    let result = app.archive.read(move |db| {
        // One read transaction keeps the retention cursor and results consistent.
        let tx = db.transaction()?;
        let oldest: Option<i64> = tx.query_row("SELECT MIN(id) FROM events",[],|r|r.get(0))?;
        let mut query=tx.prepare("SELECT id,time_ms,stream,kind,state FROM events WHERE id>?1 AND (?2 IS NULL OR stream=?2) ORDER BY id LIMIT ?3")?;
        let rows=query.query_map(params![after as i64,q.stream,limit as i64],|r|Ok(Event{id:r.get::<_,i64>(0)? as u64,time_ms:r.get::<_,i64>(1)? as u64,stream:r.get(2)?,kind:r.get(3)?,state:r.get(4)?}))?;
        let events=rows.collect::<Result<Vec<_>,_>>()?;
        let next=events.last().map(|e|e.id).unwrap_or(after);
        Ok(json!({"events":events,"next_after_id":next,"oldest_id":oldest,"cursor_expired":after>0 && oldest.is_some_and(|id|after<(id as u64).saturating_sub(1)),"retention_events":RETAIN}))
    }).await;
    match result {
        Ok(value) => Json(value).into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

// OS free-space observation only: never write a probe file on a GET request.
async fn free_space(root: String) -> Option<u64> {
    tokio::task::spawn_blocking(move || {
        #[cfg(unix)]
        {
            let path = std::ffi::CString::new(root).ok()?;
            let mut data = std::mem::MaybeUninit::<libc::statvfs>::uninit();
            // SAFETY: NUL-terminated path and a writable statvfs output buffer.
            if unsafe { libc::statvfs(path.as_ptr(), data.as_mut_ptr()) } != 0 {
                return None;
            }
            // SAFETY: successful statvfs initialized the output.
            let data = unsafe { data.assume_init() };
            Some((data.f_bavail as u128 * data.f_frsize as u128).min(u64::MAX as u128) as u64)
        }
        #[cfg(not(unix))]
        {
            let _ = root;
            None
        }
    })
    .await
    .ok()
    .flatten()
}
pub async fn snapshot(app: &App) -> serde_json::Value {
    let database_ok = app
        .archive
        .read(|db| {
            db.query_row("SELECT COUNT(*) FROM controls", [], |_| Ok(()))?;
            Ok(())
        })
        .await
        .is_ok();
    let available = free_space(app.config.recording_dir.clone()).await;
    let mut cameras = Vec::new();
    let mut healthy = database_ok
        && !app.events.0.failed.load(Ordering::Relaxed)
        && available.is_some_and(|n| n >= 1024 * 1024);
    for (id, config) in &app.config.streams {
        let active = app.recordings.active(id, config.record).await;
        let state = app.recordings.get(id).await;
        let online = app.hub.online(id).await;
        let recording_ok = !active || state == "recording";
        healthy &= recording_ok;
        cameras.push(json!({"id":id,"input_active":online,"recording_requested":active,"recording":state,"recording_ok":recording_ok}));
    }
    json!({"status":if healthy{"ok"}else{"degraded"},"version":env!("CARGO_PKG_VERSION"),"database_readable":database_ok,
        "storage":{"used_bytes":app.recordings.used_bytes(),"limit_bytes":app.config.recording_limit_bytes,"filesystem_available_bytes":available,"unfinished_files_at_startup":app.recordings.unfinished_files()},
        "journal":{"write_failed":app.events.0.failed.load(Ordering::Relaxed),"lost_events":app.events.0.lost.load(Ordering::Relaxed),"retention_events":RETAIN},
        "sessions":{"occupied_slots":app.config.max_sessions.saturating_sub(app.slots.available_permits()),"limit":app.config.max_sessions},"cameras":cameras})
}
pub async fn status(State(app): State<Arc<App>>) -> Response {
    Json(snapshot(&app).await).into_response()
}
pub async fn ready(State(app): State<Arc<App>>) -> Response {
    let value = snapshot(&app).await;
    (
        if value["status"] == "ok" {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(value),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn body(response: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }
    async fn fixture() -> (Arc<App>, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("uvms-operations-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let mut app = crate::tests::app();
        Arc::get_mut(&mut Arc::get_mut(&mut app).unwrap().config)
            .unwrap()
            .recording_dir = root.to_str().unwrap().into();
        app.archive
            .initialize(root.to_str().unwrap())
            .await
            .unwrap();
        (app, root)
    }
    #[tokio::test]
    async fn journal_retention_cursor_filter_shutdown_drain_and_restart() {
        let (app, root) = fixture().await;
        let worker = app.events.start(app.clone()).await.unwrap();
        app.archive.run(|db| {
            let tx=db.transaction()?;
            for _ in 0..10_002 {tx.execute("INSERT INTO events(time_ms,stream,kind,state) VALUES(1,'old','input','online')",[])?;}
            tx.commit()?;Ok(())
        }).await.unwrap();
        app.events.emit(Some("cam"), "input", "interrupted");
        app.events.emit(Some("cam"), "input", "online");
        app.events.stop();
        worker.await.unwrap();
        let count = app
            .archive
            .read(|db| Ok(db.query_row("SELECT COUNT(*) FROM events", [], |r| r.get::<_, i64>(0))?))
            .await
            .unwrap();
        assert_eq!(count, 10_000);
        let value = body(
            events(
                State(app.clone()),
                Query(EventQuery {
                    stream: None,
                    after_id: Some(1),
                    limit: Some(2),
                }),
            )
            .await,
        )
        .await;
        assert_eq!(value["cursor_expired"], true);
        assert_eq!(value["oldest_id"], 5);
        assert_eq!(value["next_after_id"], 6);
        let value = body(
            events(
                State(app.clone()),
                Query(EventQuery {
                    stream: None,
                    after_id: Some(10_002),
                    limit: None,
                }),
            )
            .await,
        )
        .await;
        assert_eq!(value["events"][0]["state"], "interrupted");
        assert_eq!(value["events"][1]["state"], "online");
        let reopened = crate::archive::Archive::default();
        reopened.initialize(root.to_str().unwrap()).await.unwrap();
        let count = reopened
            .read(|db| {
                Ok(
                    db.query_row("SELECT COUNT(*) FROM events WHERE stream='cam'", [], |r| {
                        r.get::<_, i64>(0)
                    })?,
                )
            })
            .await
            .unwrap();
        assert_eq!(count, 2);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn full_journal_queue_never_blocks_media_and_failures_are_visible() {
        let (app, root) = fixture().await;
        for _ in 0..300 {
            app.events.emit(None, "service", "test");
        }
        assert_eq!(app.events.0.lost.load(Ordering::Relaxed), 44);
        let worker = app.events.start(app.clone()).await.unwrap();
        app.events.stop();
        worker.await.unwrap();
        assert_eq!(snapshot(&app).await["journal"]["lost_events"], 44);
        // A missing database is degraded and a read-only health check must not
        // create an empty replacement database.
        std::fs::rename(root.join("archive.sqlite"), root.join("saved.sqlite")).unwrap();
        let response = ready(State(app.clone())).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body(response).await["database_readable"], false);
        assert!(!root.join("archive.sqlite").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn journal_write_error_is_reported_without_crashing_the_engine() {
        let (app, root) = fixture().await;
        let worker = app.events.start(app.clone()).await.unwrap();
        app.archive
            .run(|db| {
                db.execute("DROP TABLE events", [])?;
                Ok(())
            })
            .await
            .unwrap();
        app.events.emit(None, "service", "test");
        app.events.stop();
        worker.await.unwrap();
        let state = snapshot(&app).await;
        assert_eq!(state["journal"]["write_failed"], true);
        assert_eq!(state["journal"]["lost_events"], 1);
        assert_eq!(state["status"], "degraded");
        std::fs::remove_dir_all(root).unwrap();
    }
}
