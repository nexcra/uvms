//! Coverage of completed, present, precisely indexed recordings. Unknown-length
//! legacy files never fabricate coverage. Intervals are half-open [from,to).
use crate::{archive::Clip, App};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{path::PathBuf, sync::Arc};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Range {
    from: u64,
    to: u64,
}
#[derive(Debug, PartialEq, Serialize)]
struct Interval {
    start_ms: u64,
    end_ms: u64,
}

fn coverage(mut clips: Vec<Clip>, from: u64, to: u64) -> (Vec<Interval>, Vec<Interval>) {
    clips.sort_unstable_by_key(|c| c.start_ms);
    let mut recorded: Vec<Interval> = vec![];
    for c in clips {
        if c.estimated {
            continue;
        }
        let start = c.start_ms.max(from);
        let end = c.end_ms.min(to);
        if start >= end {
            continue;
        }
        if let Some(last) = recorded.last_mut().filter(|last| start <= last.end_ms) {
            last.end_ms = last.end_ms.max(end);
        } else {
            recorded.push(Interval {
                start_ms: start,
                end_ms: end,
            });
        }
    }
    let mut gaps = vec![];
    let mut cursor = from;
    for interval in &recorded {
        if cursor < interval.start_ms {
            gaps.push(Interval {
                start_ms: cursor,
                end_ms: interval.start_ms,
            });
        }
        cursor = interval.end_ms;
    }
    if cursor < to {
        gaps.push(Interval {
            start_ms: cursor,
            end_ms: to,
        });
    }
    (recorded, gaps)
}

pub async fn get(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Query(range): Query<Range>,
) -> Response {
    if !app.config.streams.contains_key(&id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    if range.from >= range.to
        || range.to > i64::MAX as u64
        || range.to - range.from > 31 * 86400 * 1000
    {
        return (
            StatusCode::BAD_REQUEST,
            "from/to must specify a positive interval of at most 31 days in Unix milliseconds",
        )
            .into_response();
    }
    let clips = match app
        .archive
        .timeline_candidates(id.clone(), range.from, range.to)
        .await
    {
        Ok(clips) => clips,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    if clips.len() > 10_000 {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            "more than 10000 clips; narrow the time interval",
        )
            .into_response();
    }
    let root = PathBuf::from(&app.config.recording_dir).join(&id);
    // Filesystem work is offloaded as one bounded task, not thousands of Tokio
    // filesystem jobs. Read-only: never repairs or deletes recordings on GET.
    let result=tokio::task::spawn_blocking(move || {
        let safe_root=std::fs::symlink_metadata(&root).is_ok_and(|m|m.is_dir());
        let mut verified=vec![];
        let mut unavailable=0;
        let mut unknown=0;
        for c in clips {
            if c.end_ms<=range.from && !c.estimated || c.start_ms>=range.to { continue; }
            if !safe_root || !crate::record::safe_filename(&c.file) || !std::fs::symlink_metadata(root.join(&c.file)).is_ok_and(|m|m.is_file() && m.len()==c.bytes) {
                unavailable+=1;continue;
            }
            if c.estimated { unknown+=1;continue; }
            verified.push(c);
        }
        let count=verified.len();
        let (recorded,gaps)=coverage(verified,range.from,range.to);
        let recorded_ms:u64=recorded.iter().map(|r|r.end_ms-r.start_ms).sum();
        json!({"stream":id,"from_ms":range.from,"to_ms":range.to,"recorded":recorded,"gaps":gaps,"recorded_ms":recorded_ms,"gap_ms":range.to-range.from-recorded_ms,"completed_clips":count,"unavailable_clips":unavailable,"unknown_duration_clips":unknown,"includes_active_recording":false})
    }).await;
    match result {
        Ok(value) => Json(value).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn timeline_excludes_missing_changed_and_unknown_files_without_touching_them() {
        let root = std::env::temp_dir().join(format!("uvms-timeline-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("cam")).unwrap();
        let mut app = crate::tests::app();
        let config = Arc::get_mut(&mut Arc::get_mut(&mut app).unwrap().config).unwrap();
        config.recording_dir = root.to_str().unwrap().into();
        config
            .streams
            .insert("cam".into(), crate::config::StreamConfig::default());
        app.archive
            .initialize(root.to_str().unwrap())
            .await
            .unwrap();
        for (file, start_ms, end_ms, estimated, size) in [
            ("1-a.mp4", 10, 20, false, Some(1)),
            ("2-b.mp4", 20, 30, false, None),
            ("3-c.mp4", 30, 40, false, Some(2)),
            ("4-d.mp4", 40, 40, true, Some(1)),
            ("5-e.mp4", 60, 80, false, Some(1)),
        ] {
            if let Some(size) = size {
                std::fs::write(root.join("cam").join(file), vec![0; size]).unwrap();
            }
            app.archive
                .insert(Clip {
                    stream: "cam".into(),
                    file: file.into(),
                    start_ms,
                    end_ms,
                    bytes: 1,
                    estimated,
                })
                .await
                .unwrap();
        }
        let response = get(
            State(app.clone()),
            Path("cam".into()),
            Query(Range { from: 0, to: 100 }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["recorded_ms"], 30);
        assert_eq!(value["gap_ms"], 70);
        assert_eq!(value["unavailable_clips"], 2);
        assert_eq!(value["unknown_duration_clips"], 1);
        assert_eq!(
            std::fs::metadata(root.join("cam/3-c.mp4")).unwrap().len(),
            2
        );
        for (from, to) in [(10, 10), (20, 10), (0, 32 * 86400 * 1000)] {
            assert_eq!(
                get(
                    State(app.clone()),
                    Path("cam".into()),
                    Query(Range { from, to })
                )
                .await
                .status(),
                StatusCode::BAD_REQUEST
            );
        }
        app.archive
            .run(|db| {
                let tx = db.transaction()?;
                for i in 0..10_001 {
                    tx.execute(
                        "INSERT INTO clips VALUES('cam',?1,50,60,1,0)",
                        [format!("many-{i}.mp4")],
                    )?;
                }
                tx.commit()?;
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(
            get(
                State(app),
                Path("cam".into()),
                Query(Range { from: 0, to: 100 })
            )
            .await
            .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn merges_overlap_clips_edges_and_does_not_invent_unknown_coverage() {
        let clips = [
            (5, 20, false),
            (15, 30, false),
            (30, 40, false),
            (60, 100, false),
            (40, 60, true),
        ]
        .into_iter()
        .map(|(start_ms, end_ms, estimated)| Clip {
            stream: "cam".into(),
            file: "1-a.mp4".into(),
            start_ms,
            end_ms,
            bytes: 1,
            estimated,
        })
        .collect();
        let (recorded, gaps) = coverage(clips, 10, 90);
        assert_eq!(
            recorded,
            vec![
                Interval {
                    start_ms: 10,
                    end_ms: 40
                },
                Interval {
                    start_ms: 60,
                    end_ms: 90
                }
            ]
        );
        assert_eq!(
            gaps,
            vec![Interval {
                start_ms: 40,
                end_ms: 60
            }]
        );
        assert_eq!(
            coverage(vec![], 10, 90).1,
            vec![Interval {
                start_ms: 10,
                end_ms: 90
            }]
        );
    }
}
