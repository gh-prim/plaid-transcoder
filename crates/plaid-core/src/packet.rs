/// One compressed frame (or one lace of a laced block) as stored in the
/// container. Timestamps are in the owning stream's `time_base`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub stream_index: usize,
    pub pts: Option<i64>,
    /// Only set when the container stores it. Matroska does not; the muxer
    /// side derives DTS from reordering.
    pub dts: Option<i64>,
    pub duration: Option<i64>,
    /// Byte offset of the packet's container structure in the input.
    pub pos: Option<u64>,
    pub keyframe: bool,
    pub data: Vec<u8>,
    pub side_data: Vec<SideData>,
}

/// Per-packet data the container stores next to the payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SideData {
    WebVttIdentifier(Vec<u8>),
    WebVttSettings(Vec<u8>),
}
