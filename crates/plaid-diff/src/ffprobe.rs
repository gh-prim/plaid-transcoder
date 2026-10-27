//! Runs ffprobe and extracts the fields we compare.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

#[derive(Debug, Default)]
pub struct Stream {
    pub index: Option<u64>,
    pub codec_type: Option<String>,
    pub codec_name: Option<String>,
    pub time_base: Option<String>,
    pub width: Option<u64>,
    pub height: Option<u64>,
    pub sample_rate: Option<u64>,
    pub channels: Option<u64>,
    pub extradata_size: u64,
    pub extradata_hash: Option<String>,
    pub default: bool,
    pub forced: bool,
    pub attached_pic: bool,
    pub tags: Vec<(String, String)>,
}

#[derive(Debug, Default)]
pub struct Packet {
    pub stream_index: Option<u64>,
    pub pts: Option<i64>,
    pub dts: Option<i64>,
    pub duration: Option<i64>,
    pub size: Option<u64>,
    pub pos: Option<u64>,
    pub keyframe: bool,
    pub data_hash: Option<String>,
}

pub struct Probe {
    pub streams: Vec<Stream>,
    pub packets: Vec<Packet>,
}

/// Raw demuxer output: `nofillin` stops libavformat from deriving missing
/// timestamps/durations and `noparse` disables parsers, so packets are exactly
/// what matroskadec produced.
pub fn run(ffprobe: &str, path: &Path) -> Result<Probe, String> {
    let out = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-fflags",
            "+nofillin+noparse",
            "-show_streams",
            "-show_packets",
        ])
        .args(["-show_data_hash", "MD5", "-of", "json"])
        .arg(path)
        .output()
        .map_err(|e| format!("cannot run {ffprobe}: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("ffprobe JSON: {e}"))?;
    let list = |key: &str| {
        v.get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    Ok(Probe {
        streams: list("streams").iter().map(stream).collect(),
        packets: list("packets").iter().map(packet).collect(),
    })
}

/// ffprobe prints some numbers as JSON strings ("size": "512").
fn int(v: &Value, key: &str) -> Option<i64> {
    match v.get(key)? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn uint(v: &Value, key: &str) -> Option<u64> {
    int(v, key).and_then(|i| u64::try_from(i).ok())
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn stream(v: &Value) -> Stream {
    let flag = |key: &str| v.get("disposition").and_then(|d| int(d, key)).unwrap_or(0) != 0;
    let tags = v
        .get("tags")
        .and_then(Value::as_object)
        .map(|t| {
            t.iter()
                .filter_map(|(k, val)| Some((k.clone(), val.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default();
    Stream {
        index: uint(v, "index"),
        codec_type: text(v, "codec_type"),
        codec_name: text(v, "codec_name"),
        time_base: text(v, "time_base"),
        width: uint(v, "width"),
        height: uint(v, "height"),
        sample_rate: uint(v, "sample_rate"),
        channels: uint(v, "channels"),
        extradata_size: uint(v, "extradata_size").unwrap_or(0),
        extradata_hash: text(v, "extradata_hash"),
        default: flag("default"),
        forced: flag("forced"),
        attached_pic: flag("attached_pic"),
        tags,
    }
}

fn packet(v: &Value) -> Packet {
    Packet {
        stream_index: uint(v, "stream_index"),
        pts: int(v, "pts"),
        dts: int(v, "dts"),
        duration: int(v, "duration"),
        size: uint(v, "size"),
        pos: uint(v, "pos"),
        keyframe: text(v, "flags").is_some_and(|f| f.starts_with('K')),
        data_hash: text(v, "data_hash"),
    }
}
