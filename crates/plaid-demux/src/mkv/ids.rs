//! Matroska element IDs (RFC 9559). Only the ones the demuxer reads.

pub const EBML: u32 = 0x1A45DFA3;
pub const DOC_TYPE: u32 = 0x4282;
pub const EBML_MAX_ID_LENGTH: u32 = 0x42F2;
pub const EBML_MAX_SIZE_LENGTH: u32 = 0x42F3;

pub const SEGMENT: u32 = 0x18538067;
pub const SEEK_HEAD: u32 = 0x114D9B74;
pub const SEEK: u32 = 0x4DBB;
pub const SEEK_ID: u32 = 0x53AB;
pub const SEEK_POSITION: u32 = 0x53AC;

pub const INFO: u32 = 0x1549A966;
pub const TIMESTAMP_SCALE: u32 = 0x2AD7B1;
pub const DURATION: u32 = 0x4489;
pub const TITLE: u32 = 0x7BA9;

pub const TRACKS: u32 = 0x1654AE6B;
pub const TRACK_ENTRY: u32 = 0xAE;
pub const TRACK_NUMBER: u32 = 0xD7;
pub const TRACK_TYPE: u32 = 0x83;
pub const FLAG_DEFAULT: u32 = 0x88;
pub const FLAG_FORCED: u32 = 0x55AA;
pub const DEFAULT_DURATION: u32 = 0x23E383;
pub const NAME: u32 = 0x536E;
pub const LANGUAGE: u32 = 0x22B59C;
pub const CODEC_ID: u32 = 0x86;
pub const CODEC_PRIVATE: u32 = 0x63A2;
pub const CODEC_DELAY: u32 = 0x56AA;
pub const VIDEO: u32 = 0xE0;
pub const PIXEL_WIDTH: u32 = 0xB0;
pub const PIXEL_HEIGHT: u32 = 0xBA;
pub const DISPLAY_WIDTH: u32 = 0x54B0;
pub const DISPLAY_HEIGHT: u32 = 0x54BA;
pub const AUDIO: u32 = 0xE1;
pub const SAMPLING_FREQUENCY: u32 = 0xB5;
pub const OUTPUT_SAMPLING_FREQUENCY: u32 = 0x78B5;
pub const CHANNELS: u32 = 0x9F;
pub const BIT_DEPTH: u32 = 0x6264;
pub const CONTENT_ENCODINGS: u32 = 0x6D80;
pub const CONTENT_ENCODING: u32 = 0x6240;
pub const CONTENT_ENCODING_SCOPE: u32 = 0x5032;
pub const CONTENT_ENCODING_TYPE: u32 = 0x5033;
pub const CONTENT_COMPRESSION: u32 = 0x5034;
pub const CONTENT_COMP_ALGO: u32 = 0x4254;
pub const CONTENT_COMP_SETTINGS: u32 = 0x4255;

pub const ATTACHMENTS: u32 = 0x1941A469;
pub const ATTACHED_FILE: u32 = 0x61A7;
pub const FILE_NAME: u32 = 0x466E;
pub const FILE_MIME_TYPE: u32 = 0x4660;
pub const FILE_DATA: u32 = 0x465C;

pub const CLUSTER: u32 = 0x1F43B675;
pub const CLUSTER_TIMESTAMP: u32 = 0xE7;
pub const SIMPLE_BLOCK: u32 = 0xA3;
pub const BLOCK_GROUP: u32 = 0xA0;
pub const BLOCK: u32 = 0xA1;
pub const BLOCK_DURATION: u32 = 0x9B;
pub const REFERENCE_BLOCK: u32 = 0xFB;

pub const VOID: u32 = 0xEC;
pub const CRC32: u32 = 0xBF;

pub const CUES: u32 = 0x1C53BB6B;
pub const CHAPTERS: u32 = 0x1043A770;
pub const TAGS: u32 = 0x1254C367;

/// Children of Segment. Seeing one of these inside an unknown-size cluster
/// means that cluster has ended.
pub fn is_segment_level(id: u32) -> bool {
    matches!(
        id,
        SEEK_HEAD | INFO | TRACKS | CLUSTER | CUES | ATTACHMENTS | CHAPTERS | TAGS | EBML | SEGMENT
    )
}
