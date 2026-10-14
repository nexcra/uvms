//! Offline validation only: mux original-DLL synthetic capture frames into MP4.
use anyhow::{Context, Result};
use serde_json::Value;
use std::{fs, io::Write, path::PathBuf};
use unreal_mp4::Muxer;

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    anyhow::ensure!(args.len() == 2, "usage: capture_mux CAPTURE_DIR OUTPUT.mp4");
    let root = PathBuf::from(&args[0]);
    let rows: Vec<Value> = fs::read_to_string(root.join("frames.jsonl"))?
        .lines()
        .map(serde_json::from_str)
        .collect::<std::result::Result<_, _>>()?;
    let first = rows
        .iter()
        .position(|r| r["keyframe"] == 1)
        .context("no IDR")?;
    let initial = fs::read(root.join(rows[first]["file"].as_str().unwrap()))?;
    let nals = rows[first]["nals"].as_array().unwrap();
    let parameter = |kind| -> Result<&[u8]> {
        let entry = nals
            .iter()
            .find(|n| n["type"] == kind)
            .context("parameter set missing")?;
        let offset = entry["offset"].as_u64().unwrap() as usize + 4;
        let length = entry["length"].as_u64().unwrap() as usize;
        Ok(&initial[offset..offset + length])
    };
    let mut mux = Muxer::new(parameter(7)?, parameter(8)?, 640, 360)?;
    let mut file = fs::File::create(&args[1])?;
    file.write_all(mux.init_segment())?;
    let base = rows[first]["start100ns"].as_u64().unwrap();
    let mut bytes = 0usize;
    for row in &rows[first..] {
        let data = fs::read(root.join(row["file"].as_str().unwrap()))?;
        let start = row["start100ns"].as_u64().unwrap();
        let end = row["end100ns"].as_u64().unwrap();
        let fragment = mux.fragment(
            &data,
            (start - base) * 9 / 1000,
            ((end - start) * 9 / 1000) as u32,
            row["keyframe"] == 1,
        )?;
        file.write_all(&fragment)?;
        bytes += data.len();
    }
    println!(
        "codec={} frames={} original_AVCC_bytes={} first_capture_index={} output={}",
        mux.codec(),
        rows.len() - first,
        bytes,
        first,
        PathBuf::from(&args[1]).display()
    );
    Ok(())
}
