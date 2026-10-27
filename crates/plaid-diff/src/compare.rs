//! Field-by-field comparison of plaid output against ffprobe.

use std::collections::BTreeMap;
use std::fmt::Debug;

use md5::{Digest, Md5};
use plaid_core::{Packet, StreamInfo};

use crate::ffprobe;

/// Stream tags we compare. Others (DURATION, ENCODER...) come from the Tags
/// element, which the demuxer does not read yet.
const COMPARED_TAGS: &[&str] = &["language", "title", "filename", "mimetype"];

#[derive(Default)]
pub struct Report {
    /// Mismatch count per field name.
    pub counts: BTreeMap<&'static str, usize>,
    /// First few mismatches in human-readable form.
    pub examples: Vec<String>,
    pub max_examples: usize,
    pub packets_compared: usize,
    /// Differences that are expected and do not fail the file.
    pub notes: Vec<String>,
}

impl Report {
    pub fn is_clean(&self) -> bool {
        self.counts.is_empty()
    }

    fn note(&mut self, note: String) {
        self.notes.push(note);
    }

    fn mismatch(&mut self, field: &'static str, what: impl FnOnce() -> String) {
        *self.counts.entry(field).or_default() += 1;
        if self.examples.len() < self.max_examples {
            self.examples.push(what());
        }
    }

    fn check<T: PartialEq + Debug>(&mut self, field: &'static str, at: &str, ours: T, theirs: T) {
        if ours != theirs {
            self.mismatch(field, || {
                format!("{at}: {field} plaid={ours:?} ffprobe={theirs:?}")
            });
        }
    }
}

pub fn md5_hash(data: &[u8]) -> String {
    let digest = Md5::digest(data);
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("MD5:{hex}")
}

pub fn streams(report: &mut Report, ours: &[StreamInfo], theirs: &[ffprobe::Stream]) {
    report.check("stream_count", "file", ours.len(), theirs.len());
    for (s, t) in ours.iter().zip(theirs) {
        let at = format!("stream #{}", s.index);
        report.check("index", &at, Some(s.index as u64), t.index);
        report.check(
            "codec_type",
            &at,
            Some(s.kind.name()),
            t.codec_type.as_deref(),
        );
        // ffprobe omits codec_name for codecs it does not know.
        report.check(
            "codec_name",
            &at,
            s.codec.name(),
            t.codec_name.as_deref().unwrap_or("none"),
        );
        report.check(
            "time_base",
            &at,
            Some(s.time_base.to_string()),
            t.time_base.clone(),
        );
        if let Some(v) = &s.video {
            report.check("width", &at, Some(u64::from(v.width)), t.width);
            report.check("height", &at, Some(u64::from(v.height)), t.height);
        }
        if let Some(a) = &s.audio {
            report.check(
                "sample_rate",
                &at,
                Some(u64::from(a.sample_rate)),
                t.sample_rate,
            );
            report.check("channels", &at, Some(u64::from(a.channels)), t.channels);
        }
        if s.extradata.is_empty() && t.extradata_size > 0 {
            // The file has no codec config; ffprobe's comes from decoding the
            // first frames in find_stream_info, not from the demuxer.
            report.note(format!(
                "{at}: extradata derived by ffprobe ({} bytes)",
                t.extradata_size
            ));
        } else {
            report.check(
                "extradata_size",
                &at,
                s.extradata.len() as u64,
                t.extradata_size,
            );
            if let Some(hash) = &t.extradata_hash {
                report.check("extradata_hash", &at, &md5_hash(&s.extradata), hash);
            }
        }
        report.check("disposition.default", &at, s.disposition.default, t.default);
        report.check("disposition.forced", &at, s.disposition.forced, t.forced);
        report.check(
            "disposition.attached_pic",
            &at,
            s.disposition.attached_pic,
            t.attached_pic,
        );
        for &key in COMPARED_TAGS {
            let theirs = t
                .tags
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v.as_str());
            report.check("tags", &format!("{at} tag {key}"), s.tag(key), theirs);
        }
    }
}

/// Compares one packet. `index` is its position in file order.
pub fn packet(report: &mut Report, index: usize, ours: &Packet, theirs: &ffprobe::Packet) {
    report.packets_compared += 1;
    let at = format!("packet #{index} (stream {})", ours.stream_index);
    report.check(
        "stream_index",
        &at,
        Some(ours.stream_index as u64),
        theirs.stream_index,
    );
    report.check("pts", &at, ours.pts, theirs.pts);
    // Matroska stores no DTS; only compare when we produce one.
    if ours.dts.is_some() {
        report.check("dts", &at, ours.dts, theirs.dts);
    }
    // ffprobe prints a zero duration as absent.
    report.check(
        "duration",
        &at,
        ours.duration.filter(|&d| d != 0),
        theirs.duration.filter(|&d| d != 0),
    );
    report.check("size", &at, Some(ours.data.len() as u64), theirs.size);
    report.check("pos", &at, ours.pos, theirs.pos);
    report.check("keyframe", &at, ours.keyframe, theirs.keyframe);
    if let Some(hash) = &theirs.data_hash {
        report.check("data_hash", &at, &md5_hash(&ours.data), hash);
    }
}
