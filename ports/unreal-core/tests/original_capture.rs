use serde_json::Value;
use std::{fs, path::PathBuf};
use unreal_core_port::frame::{self, Metadata};

/// Requires external synthetic-video frame capture made by original URTSPClient.dll.
/// The output of FUN_140025dc0 was NOT captured; this tests preservation and the
/// inferred index contract, not equivalence with original normalization runtime.
#[test]
#[ignore = "set UNREAL_CAPTURE_DIR to evidence/runtime/rtsp-original-mta"]
fn preserves_all_40_original_rtsp_frames() {
    let root =
        PathBuf::from(std::env::var_os("UNREAL_CAPTURE_DIR").expect("UNREAL_CAPTURE_DIR required"));
    let manifest = fs::read_to_string(root.join("frames.jsonl")).unwrap();
    let mut count = 0;
    let mut keyframes = Vec::new();
    let mut total_nals = 0;
    let mut payload_bytes = 0;
    let mut previous_start = None;
    for line in manifest.lines().filter(|line| !line.is_empty()) {
        let row: Value = serde_json::from_str(line).unwrap();
        let input = fs::read(root.join(row["file"].as_str().unwrap())).unwrap();
        assert_eq!(input.len() as u64, row["length"].as_u64().unwrap());
        assert_eq!(row["trailingBytes"], 0);
        let metadata = Metadata {
            kind: row["kind"].as_u64().unwrap() as u32,
            start_100ns: row["start100ns"].as_i64().unwrap(),
            end_100ns: row["end100ns"].as_i64().unwrap(),
            keyframe: row["keyframe"].as_u64().unwrap() != 0,
        };
        let normalized = frame::normalize(&input, metadata, None).unwrap();
        assert_eq!(normalized.metadata, metadata);
        assert_eq!(metadata.kind, 2);
        if let Some(previous) = previous_start {
            assert!((666_660..=666_670).contains(&(metadata.start_100ns - previous)));
        }
        assert!(metadata.end_100ns > metadata.start_100ns);
        previous_start = Some(metadata.start_100ns);
        let input_nals = row["nals"].as_array().unwrap();
        assert_eq!(normalized.nals().len(), input_nals.len());
        assert_eq!(normalized.payload().len(), input.len());
        let trailer = normalized.index_trailer();
        assert_eq!(
            u32::from_le_bytes(trailer[..4].try_into().unwrap()) as usize,
            input_nals.len()
        );
        for (index, (source, output)) in input_nals.iter().zip(normalized.nals()).enumerate() {
            let offset = source["offset"].as_u64().unwrap() as usize + 4;
            let length = source["length"].as_u64().unwrap() as usize;
            assert_eq!(output.offset as usize, offset);
            assert_eq!(output.length as usize, length);
            assert_eq!(
                &input[offset..offset + length],
                &normalized.payload()[offset..offset + length]
            );
            assert_eq!(&normalized.payload()[offset - 4..offset], &[0, 0, 0, 1]);
            assert_eq!(input[offset] & 31, source["type"].as_u64().unwrap() as u8);
            let index_offset = 4 + index * 8;
            assert_eq!(
                u32::from_le_bytes(trailer[index_offset..index_offset + 4].try_into().unwrap()),
                output.offset
            );
            assert_eq!(
                u32::from_le_bytes(
                    trailer[index_offset + 4..index_offset + 8]
                        .try_into()
                        .unwrap()
                ),
                output.length
            );
        }
        let types: Vec<_> = input_nals
            .iter()
            .map(|n| n["type"].as_u64().unwrap())
            .collect();
        if metadata.keyframe {
            keyframes.push(count);
            assert_eq!(types, [7, 8, 5, 5, 5, 5, 5]);
        } else {
            assert_eq!(types, [1, 1, 1, 1, 1]);
        }
        count += 1;
        total_nals += types.len();
        payload_bytes += input.len();
    }
    assert_eq!(count, 40);
    assert_eq!(keyframes, [11, 26]);
    assert_eq!(total_nals, 204);
    println!("original capture: {count} frames, {total_nals} unchanged NALs, {payload_bytes} payload bytes, keyframes {keyframes:?}; metadata preserved");
}
