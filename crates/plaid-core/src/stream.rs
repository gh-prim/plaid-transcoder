use std::collections::BTreeMap;

use crate::{CodecId, MediaKind, Rational};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VideoParams {
    pub width: u32,
    pub height: u32,
    pub display_width: Option<u32>,
    pub display_height: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AudioParams {
    pub sample_rate: u32,
    pub channels: u32,
    pub bits_per_sample: Option<u32>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Disposition {
    pub default: bool,
    pub forced: bool,
    /// Cover art carried as a single still-image packet.
    pub attached_pic: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StreamInfo {
    pub index: usize,
    pub kind: MediaKind,
    pub codec: CodecId,
    /// The container's own codec identifier (e.g. Matroska `V_MPEG4/ISO/AVC`).
    pub container_codec: String,
    /// Unit of every timestamp and duration on this stream's packets.
    pub time_base: Rational,
    /// Codec configuration (avcC, hvcC, AudioSpecificConfig...) or attachment data.
    pub extradata: Vec<u8>,
    pub video: Option<VideoParams>,
    pub audio: Option<AudioParams>,
    pub disposition: Disposition,
    /// Metadata using FFmpeg's keys: `language`, `title`, `filename`, `mimetype`.
    pub tags: BTreeMap<String, String>,
}

impl StreamInfo {
    pub fn tag(&self, key: &str) -> Option<&str> {
        self.tags.get(key).map(String::as_str)
    }
}

/// Container-level information.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FormatInfo {
    /// Same spelling as FFmpeg's `format_name` (e.g. `matroska,webm`).
    pub name: &'static str,
    pub duration_seconds: Option<f64>,
    pub tags: BTreeMap<String, String>,
}
