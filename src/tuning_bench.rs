//! Explicit release-mode component comparisons, not end-to-end viewer capacity.
use bytes::Bytes;
use std::{hint::black_box, time::Instant};

#[test]
#[ignore = "manual bounded release benchmark"]
fn component_costs() {
    use crate::rtp_video::{Packetizer, PreparedFrame};
    let mut annex = vec![0, 0, 0, 1, 0x65];
    annex.resize(32768, 1);
    let prepared = PreparedFrame::new(annex.into(), 1100).unwrap();
    let audio = Bytes::from(vec![42; 320]);
    for round in 0..6 {
        for kind in ["rtp_list", "audio_payload", "mse_header"] {
            let mut timings = [0; 2];
            let mut totals = [0; 2];
            for shared in if round % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            } {
                let mut packetizer = Packetizer::default();
                let mut scratch = Vec::new();
                let started = Instant::now();
                let mut total = 0;
                match kind {
                    "rtp_list" => {
                        for _ in 0..4000 {
                            for _ in 0..16 {
                                if shared {
                                    packetizer.payloads_into(black_box(&prepared), &mut scratch);
                                    for packet in scratch.drain(..) {
                                        total += black_box(packet).len();
                                    }
                                } else {
                                    for packet in packetizer.payloads(black_box(&prepared)) {
                                        total += black_box(packet).len();
                                    }
                                }
                            }
                        }
                    }
                    "audio_payload" => {
                        for _ in 0..100000 {
                            for _ in 0..16 {
                                let packet = if shared {
                                    black_box(&audio).clone()
                                } else {
                                    Bytes::copy_from_slice(black_box(&audio))
                                };
                                total += black_box(packet).len();
                            }
                        }
                    }
                    _ => {
                        for frame in 0..100000 {
                            for _ in 0..16 {
                                if shared {
                                    total += black_box(
                                        unreal_core_port::mse::media_header(
                                            0,
                                            black_box(frame),
                                            None,
                                        )
                                        .unwrap(),
                                    )
                                    .len();
                                } else {
                                    total += black_box(
                                        unreal_core_port::mse::media(
                                            0,
                                            black_box(frame),
                                            None,
                                            &[],
                                        )
                                        .unwrap(),
                                    )
                                    .len();
                                }
                            }
                        }
                    }
                }
                timings[usize::from(shared)] = started.elapsed().as_micros();
                totals[usize::from(shared)] = total;
            }
            assert_eq!(totals[0], totals[1]);
            println!(
                "{}",
                serde_json::json!({"component":kind,"round":round,"uncached_us":timings[0],"reused_us":timings[1],"viewers":16})
            );
        }
    }
}

#[tokio::test]
#[ignore = "manual bounded release benchmark"]
async fn database_connection_costs() {
    let root = std::env::temp_dir().join(format!("uvms-db-bench-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let archive = crate::archive::Archive::default();
    archive.initialize(root.to_str().unwrap()).await.unwrap();
    archive.save_storage_limit(10485760).await.unwrap();
    for round in 0..6 {
        let mut timings = [0; 2];
        let mut totals = [0u64; 2];
        for pooled in if round % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let started = Instant::now();
            let mut total = 0;
            for _ in 0..1000 {
                let value = if pooled {
                    archive.storage_limit().await.unwrap().unwrap()
                } else {
                    let path = root.join("archive.sqlite");
                    tokio::task::spawn_blocking(move || {
                        let db = rusqlite::Connection::open_with_flags(
                            path,
                            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                        )
                        .unwrap();
                        db.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
                        db.query_row(
                            "SELECT value FROM settings WHERE key='recording_limit_bytes'",
                            [],
                            |r| r.get::<_, i64>(0),
                        )
                        .unwrap() as u64
                    })
                    .await
                    .unwrap()
                };
                total += black_box(value);
            }
            timings[usize::from(pooled)] = started.elapsed().as_micros();
            totals[usize::from(pooled)] = total;
        }
        assert_eq!(totals[0], totals[1]);
        println!(
            "{}",
            serde_json::json!({"component":"database_connection","round":round,"uncached_us":timings[0],"reused_us":timings[1],"queries":1000})
        );
    }
    drop(archive);
    std::fs::remove_dir_all(root).unwrap();
}
