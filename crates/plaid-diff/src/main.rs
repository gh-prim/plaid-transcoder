//! Differential harness: demux every file with plaid and with ffprobe, then
//! compare streams and packets field by field (docs/DESIGN.md §5).
//!
//! usage: plaid-diff [--ffprobe PATH] [--max-diffs N] FILE_OR_DIR...

mod compare;
mod ffprobe;

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use plaid_core::{Demuxer, ErrorClass, Limits, Packet};

const EXTENSIONS: &[&str] = &["mkv", "mka", "mks", "mk3d", "webm"];

struct Options {
    ffprobe: String,
    max_diffs: usize,
    paths: Vec<PathBuf>,
}

fn parse_args() -> Result<Options, String> {
    let mut opts = Options {
        ffprobe: "ffprobe".into(),
        max_diffs: 10,
        paths: Vec::new(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--ffprobe" => opts.ffprobe = args.next().ok_or("--ffprobe needs a path")?,
            "--max-diffs" => {
                opts.max_diffs = args
                    .next()
                    .and_then(|n| n.parse().ok())
                    .ok_or("--max-diffs needs a number")?;
            }
            "-h" | "--help" => return Err(String::new()),
            _ => opts.paths.push(arg.into()),
        }
    }
    if opts.paths.is_empty() {
        return Err("no input files".into());
    }
    Ok(opts)
}

/// Explicit files are always tested; directories are searched for Matroska extensions.
fn collect(path: &Path, out: &mut Vec<PathBuf>) {
    if !path.is_dir() {
        out.push(path.to_owned());
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        let matroska = p
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()));
        if p.is_dir() || matroska {
            collect(&p, out);
        }
    }
}

enum Outcome {
    Pass,
    Fail,
}

struct Demuxed {
    demuxer: Box<dyn Demuxer>,
    packets: Vec<Packet>,
    corrupt_errors: usize,
    elapsed: Duration,
}

fn demux(path: &Path) -> Result<Demuxed, plaid_core::Error> {
    let start = Instant::now();
    let mut demuxer = plaid_demux::open(File::open(path)?, Limits::default())?;
    let mut packets = Vec::new();
    let mut corrupt_errors = 0;
    loop {
        match demuxer.read_packet() {
            Ok(Some(p)) => packets.push(p),
            Ok(None) => break,
            Err(e) if e.class() == ErrorClass::Corrupt => corrupt_errors += 1,
            Err(e) => return Err(e),
        }
    }
    Ok(Demuxed {
        demuxer,
        packets,
        corrupt_errors,
        elapsed: start.elapsed(),
    })
}

fn run_file(opts: &Options, path: &Path) -> Outcome {
    let name = path.display();
    let theirs = ffprobe::run(&opts.ffprobe, path);
    let ours = demux(path);

    let (ours, theirs) = match (ours, theirs) {
        (Ok(o), Ok(t)) => (o, t),
        (Err(_), Err(_)) => {
            println!("PASS {name}  (both reject)");
            return Outcome::Pass;
        }
        (Err(e), Ok(_)) => {
            println!("FAIL {name}  plaid rejects, ffprobe accepts: {e}");
            return Outcome::Fail;
        }
        (Ok(_), Err(e)) => {
            println!("FAIL {name}  ffprobe rejects ({e}), plaid accepts");
            return Outcome::Fail;
        }
    };

    let mut report = compare::Report {
        max_examples: opts.max_diffs,
        ..Default::default()
    };
    compare::streams(&mut report, ours.demuxer.streams(), &theirs.streams);
    for (i, (o, t)) in ours.packets.iter().zip(&theirs.packets).enumerate() {
        compare::packet(&mut report, i, o, t);
    }
    if ours.corrupt_errors > 0 && theirs.packets.len() < ours.packets.len() {
        // FFmpeg turns a damaged block into a read error and ffprobe stops
        // there; plaid drops the block and carries on. Packets up to that
        // point were compared above.
        report.notes.push(format!(
            "plaid recovered past damage: {} packets vs ffprobe's {} before it stopped",
            ours.packets.len(),
            theirs.packets.len()
        ));
    } else if ours.packets.len() != theirs.packets.len() {
        let (o, t) = (ours.packets.len(), theirs.packets.len());
        *report.counts.entry("packet_count").or_default() += 1;
        report
            .examples
            .push(format!("packet count plaid={o} ffprobe={t}"));
    }

    let summary = format!(
        "streams={} packets={} plaid={:.1}ms{}",
        ours.demuxer.streams().len(),
        report.packets_compared,
        ours.elapsed.as_secs_f64() * 1e3,
        if ours.corrupt_errors > 0 {
            format!(" corrupt_skips={}", ours.corrupt_errors)
        } else {
            String::new()
        },
    );
    if report.is_clean() {
        println!("PASS {name}  {summary}");
        for note in &report.notes {
            println!("    note: {note}");
        }
        return Outcome::Pass;
    }
    let counts: Vec<String> = report
        .counts
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    println!("FAIL {name}  {summary}  diffs: {}", counts.join(" "));
    for example in &report.examples {
        println!("    {example}");
    }
    Outcome::Fail
}

fn main() -> ExitCode {
    let opts = match parse_args() {
        Ok(o) => o,
        Err(msg) => {
            if !msg.is_empty() {
                eprintln!("error: {msg}");
            }
            eprintln!("usage: plaid-diff [--ffprobe PATH] [--max-diffs N] FILE_OR_DIR...");
            return ExitCode::from(2);
        }
    };
    let mut files = Vec::new();
    for p in &opts.paths {
        collect(p, &mut files);
    }

    let (mut passed, mut failed) = (0, 0);
    for file in &files {
        match run_file(&opts, file) {
            Outcome::Pass => passed += 1,
            Outcome::Fail => failed += 1,
        }
    }
    println!("\n{} files: {passed} passed, {failed} failed", files.len());
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
