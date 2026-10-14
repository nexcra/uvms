//! Persistent archive metadata and recording intent. SQLite work never runs on a
//! Tokio worker; video writers only index a completed, synced, renamed clip.
use anyhow::{ensure, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::OnceLock};

#[derive(Default)]
pub struct Archive {
    path: OnceLock<PathBuf>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Clip {
    pub stream: String,
    pub file: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub bytes: u64,
    pub estimated: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    On,
    Off,
    #[default]
    Auto,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Schedule {
    pub start_ms: u64,
    pub end_ms: u64,
    #[serde(default)]
    pub repeat_seconds: Option<u64>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Control {
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub schedules: Vec<Schedule>,
}
impl Control {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.schedules.len() <= 64, "at most 64 schedules");
        for s in &self.schedules {
            ensure!(
                s.start_ms < s.end_ms && s.end_ms <= i64::MAX as u64,
                "invalid schedule interval"
            );
            if let Some(r) = s.repeat_seconds {
                ensure!(
                    (60..=366 * 86400).contains(&r) && s.end_ms - s.start_ms <= r * 1000,
                    "invalid repeat interval"
                );
            }
        }
        Ok(())
    }
    pub fn active(&self, default: bool, now: u64) -> bool {
        match self.mode {
            Mode::On => true,
            Mode::Off => false,
            Mode::Auto => {
                if self.schedules.is_empty() {
                    return default;
                }
                self.schedules.iter().any(|s| {
                    if now < s.start_ms {
                        return false;
                    }
                    let elapsed = now - s.start_ms;
                    let offset = s
                        .repeat_seconds
                        .map(|r| elapsed % (r * 1000))
                        .unwrap_or(elapsed);
                    offset < s.end_ms - s.start_ms
                })
            }
        }
    }
}
impl Archive {
    pub async fn initialize(&self, root: &str) -> Result<()> {
        let path = PathBuf::from(root).join("archive.sqlite");
        self.path.set(path).ok();
        self.run(|db| { db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS clips(stream TEXT NOT NULL,file TEXT NOT NULL,start_ms INTEGER NOT NULL,end_ms INTEGER NOT NULL,bytes INTEGER NOT NULL,estimated INTEGER NOT NULL,PRIMARY KEY(stream,file)); CREATE INDEX IF NOT EXISTS clips_time ON clips(stream,start_ms,end_ms); CREATE TABLE IF NOT EXISTS controls(stream TEXT PRIMARY KEY,json TEXT NOT NULL);")?; Ok(()) }).await
    }
    async fn run<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let path = self
            .path
            .get()
            .ok_or_else(|| anyhow::anyhow!("archive is not initialized"))?
            .clone();
        tokio::task::spawn_blocking(move || {
            let mut db = Connection::open(path)?;
            db.busy_timeout(std::time::Duration::from_secs(5))?;
            f(&mut db)
        })
        .await?
    }
    pub async fn insert(&self, c: Clip) -> Result<()> {
        ensure!(
            c.start_ms <= c.end_ms && c.end_ms <= i64::MAX as u64 && c.bytes <= i64::MAX as u64,
            "archive metadata exceeds supported range"
        );
        self.run(move|db| { db.execute("INSERT INTO clips VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(stream,file) DO UPDATE SET start_ms=excluded.start_ms,end_ms=excluded.end_ms,bytes=excluded.bytes,estimated=excluded.estimated WHERE clips.estimated=1",params![c.stream,c.file,c.start_ms as i64,c.end_ms as i64,c.bytes as i64,c.estimated])?; Ok(()) }).await
    }
    pub async fn search(
        &self,
        stream: Option<String>,
        from: u64,
        to: u64,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Clip>> {
        self.run(move|db| { let mut q=db.prepare("SELECT stream,file,start_ms,end_ms,bytes,estimated FROM clips WHERE (?1 IS NULL OR stream=?1) AND end_ms>=?2 AND start_ms<=?3 ORDER BY start_ms DESC,stream,file LIMIT ?4 OFFSET ?5")?; let rows=q.query_map(params![stream,from.min(i64::MAX as u64)as i64,to.min(i64::MAX as u64)as i64,limit as i64,offset as i64],|r|Ok(Clip{stream:r.get(0)?,file:r.get(1)?,start_ms:r.get::<_,i64>(2)? as u64,end_ms:r.get::<_,i64>(3)?as u64,bytes:r.get::<_,i64>(4)?as u64,estimated:r.get(5)?}))?; Ok(rows.collect::<std::result::Result<Vec<_>,_>>()?) }).await
    }
    pub async fn control(&self, stream: &str) -> Result<Control> {
        let stream = stream.to_owned();
        self.run(move |db| {
            use rusqlite::OptionalExtension;
            let text: Option<String> = db
                .query_row("SELECT json FROM controls WHERE stream=?1", [stream], |r| {
                    r.get(0)
                })
                .optional()?;
            Ok(match text {
                Some(t) => serde_json::from_str(&t)?,
                None => Control::default(),
            })
        })
        .await
    }
    pub async fn save_control(&self, stream: &str, control: Control) -> Result<()> {
        control.validate()?;
        let stream = stream.to_owned();
        let json = serde_json::to_string(&control)?;
        self.run(move|db|{db.execute("INSERT INTO controls VALUES(?1,?2) ON CONFLICT(stream) DO UPDATE SET json=excluded.json",params![stream,json])?;Ok(())}).await
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeating_schedule_boundaries_and_manual_override() {
        let mut c = Control {
            mode: Mode::Auto,
            schedules: vec![Schedule {
                start_ms: 1000,
                end_ms: 3000,
                repeat_seconds: Some(60),
            }],
        };
        c.validate().unwrap();
        for t in [1000, 2999, 61000, 62999] {
            assert!(c.active(false, t));
        }
        for t in [999, 3000, 60999, 63000] {
            assert!(!c.active(true, t));
        }
        c.mode = Mode::On;
        assert!(c.active(false, 0));
        c.mode = Mode::Off;
        assert!(!c.active(true, 1000));
        c.schedules[0].repeat_seconds = Some(0);
        assert!(c.validate().is_err());
    }
    #[tokio::test]
    async fn index_and_recording_intent_survive_reopening() {
        let p = std::env::temp_dir().join(format!("uvms-archive-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&p).unwrap();
        let a = Archive::default();
        a.initialize(p.to_str().unwrap()).await.unwrap();
        a.insert(Clip {
            stream: "cam".into(),
            file: "1-a.mp4".into(),
            start_ms: 100,
            end_ms: 200,
            bytes: 50,
            estimated: false,
        })
        .await
        .unwrap();
        a.save_control(
            "cam",
            Control {
                mode: Mode::On,
                schedules: vec![],
            },
        )
        .await
        .unwrap();
        let b = Archive::default();
        b.initialize(p.to_str().unwrap()).await.unwrap();
        assert!(matches!(b.control("cam").await.unwrap().mode, Mode::On));
        assert_eq!(
            b.search(Some("cam".into()), 150, 160, 10, 0)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(b
            .search(Some("other".into()), 0, 1000, 10, 0)
            .await
            .unwrap()
            .is_empty());
        assert!(b.search(None, 201, 1000, 10, 0).await.unwrap().is_empty());
        std::fs::remove_dir_all(p).unwrap();
    }
}
