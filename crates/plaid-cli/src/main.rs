//! `plaid probe <file>`   – stream and format info as JSON (ffprobe-like keys)
//! `plaid packets <file>` – one line per packet

use std::fs::File;
use std::io::Write;
use std::process::ExitCode;

use plaid_core::{Demuxer, ErrorClass, Limits};
use serde_json::{Map, Value, json};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(cmd), Some(path)) = (args.first(), args.get(1)) else {
        eprintln!("usage: plaid <probe|packets> <file>");
        return ExitCode::from(2);
    };
    let demuxer = match File::open(path)
        .map_err(plaid_core::Error::from)
        .and_then(|f| plaid_demux::open(f, Limits::default()))
    {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    match cmd.as_str() {
        "probe" => probe(demuxer),
        "packets" => packets(demuxer),
        _ => {
            eprintln!("unknown command {cmd:?}");
            ExitCode::from(2)
        }
    }
}

fn probe(demuxer: Box<dyn Demuxer>) -> ExitCode {
    let streams: Vec<Value> = demuxer
        .streams()
        .iter()
        .map(|s| {
            let mut o = Map::new();
            o.insert("index".into(), json!(s.index));
            o.insert("codec_name".into(), json!(s.codec.name()));
            o.insert("codec_type".into(), json!(s.kind.name()));
            o.insert("container_codec".into(), json!(s.container_codec));
            o.insert("time_base".into(), json!(s.time_base.to_string()));
            o.insert("extradata_size".into(), json!(s.extradata.len()));
            if let Some(v) = &s.video {
                o.insert("width".into(), json!(v.width));
                o.insert("height".into(), json!(v.height));
            }
            if let Some(a) = &s.audio {
                o.insert("sample_rate".into(), json!(a.sample_rate));
                o.insert("channels".into(), json!(a.channels));
            }
            o.insert(
                "disposition".into(),
                json!({
                    "default": s.disposition.default,
                    "forced": s.disposition.forced,
                    "attached_pic": s.disposition.attached_pic,
                }),
            );
            o.insert("tags".into(), json!(s.tags));
            Value::Object(o)
        })
        .collect();
    let f = demuxer.format();
    let out = json!({
        "streams": streams,
        "format": {
            "format_name": f.name,
            "duration": f.duration_seconds,
            "tags": f.tags,
        },
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    ExitCode::SUCCESS
}

fn packets(mut demuxer: Box<dyn Demuxer>) -> ExitCode {
    let fmt = |v: Option<i64>| v.map_or("N/A".to_string(), |v| v.to_string());
    let mut out = std::io::stdout().lock();
    loop {
        match demuxer.read_packet() {
            Ok(Some(p)) => {
                let line = writeln!(
                    out,
                    "stream={} pts={} duration={} size={} pos={} key={}",
                    p.stream_index,
                    fmt(p.pts),
                    fmt(p.duration),
                    p.data.len(),
                    p.pos.map_or("N/A".to_string(), |v| v.to_string()),
                    u8::from(p.keyframe),
                );
                // Reader went away (e.g. piped into `head`).
                if line.is_err() {
                    return ExitCode::SUCCESS;
                }
            }
            Ok(None) => return ExitCode::SUCCESS,
            Err(e) if e.class() == ErrorClass::Corrupt => eprintln!("warning: {e}"),
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
}
