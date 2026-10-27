//! Tracks and Attachments: turning Matroska headers into `StreamInfo`.

use std::collections::BTreeMap;

use plaid_core::{
    AudioParams, CodecId, Disposition, Error, Limits, MediaKind, Rational, Result, StreamInfo,
    VideoParams,
};

use super::ebml::{children, float, string, uint};
use super::ids;

const TRACK_TYPE_VIDEO: u64 = 1;
const TRACK_TYPE_AUDIO: u64 = 2;
const TRACK_TYPE_SUBTITLE: u64 = 0x11;
const TRACK_TYPE_METADATA: u64 = 0x21;

/// Per-frame transform declared by ContentEncodings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FrameEncoding {
    /// Bytes removed from the start of every frame by the muxer.
    HeaderStrip(Vec<u8>),
    Zlib,
}

/// Demux-time state for one track.
#[derive(Debug, Clone)]
pub(crate) struct Track {
    pub number: u64,
    pub stream_index: usize,
    pub default_duration_ns: u64,
    /// CodecDelay converted to the stream time base, subtracted from timestamps.
    pub codec_delay: i64,
    pub encodings: Vec<FrameEncoding>,
    /// WebM WebVTT (`D_WEBVTT/*`): frames carry `id\nsettings\npayload`.
    pub webm_webvtt: bool,
}

impl Track {
    pub fn decode_frame(&self, frame: &[u8], limits: &Limits) -> Result<Vec<u8>> {
        decode(&self.encodings, frame, limits)
    }
}

/// Undoes content encodings (applied by the muxer in list order, so we
/// reverse them).
fn decode(encodings: &[FrameEncoding], frame: &[u8], limits: &Limits) -> Result<Vec<u8>> {
    let mut data = frame.to_vec();
    for enc in encodings.iter().rev() {
        data = match enc {
            FrameEncoding::HeaderStrip(prefix) => [prefix.as_slice(), &data].concat(),
            FrameEncoding::Zlib => inflate(&data, limits.max_frame_size)?,
        };
        if data.len() as u64 > limits.max_frame_size {
            return Err(Error::resource("decoded frame exceeds limit"));
        }
    }
    Ok(data)
}

fn inflate(data: &[u8], limit: u64) -> Result<Vec<u8>> {
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(data, limit)
        .map_err(|e| Error::corrupt(format!("zlib content encoding: {e:?}")))
}

/// Codec mapping, matched by prefix in order like FFmpeg's `ff_mkv_codec_tags`
/// (so `A_AAC/MPEG4/LC` maps through `A_AAC`).
const CODECS: &[(&str, CodecId)] = &[
    ("V_MPEG4/ISO/AVC", CodecId::H264),
    ("V_MPEGH/ISO/HEVC", CodecId::Hevc),
    ("V_AV1", CodecId::Av1),
    ("V_VP9", CodecId::Vp9),
    ("V_VP8", CodecId::Vp8),
    ("V_MPEG1", CodecId::Mpeg1Video),
    ("V_MPEG2", CodecId::Mpeg2Video),
    ("V_MPEG4/ISO/ASP", CodecId::Mpeg4),
    ("V_MPEG4/ISO/AP", CodecId::Mpeg4),
    ("V_MPEG4/ISO/SP", CodecId::Mpeg4),
    ("V_THEORA", CodecId::Theora),
    ("V_PRORES", CodecId::Prores),
    ("V_FFV1", CodecId::Ffv1),
    ("V_MJPEG", CodecId::Mjpeg),
    ("V_UNCOMPRESSED", CodecId::RawVideo),
    ("A_AAC", CodecId::Aac),
    ("A_AC3", CodecId::Ac3),
    ("A_EAC3", CodecId::Eac3),
    ("A_DTS", CodecId::Dts),
    ("A_TRUEHD", CodecId::TrueHd),
    ("A_MLP", CodecId::Mlp),
    ("A_FLAC", CodecId::Flac),
    ("A_OPUS", CodecId::Opus),
    ("A_VORBIS", CodecId::Vorbis),
    ("A_MPEG/L3", CodecId::Mp3),
    ("A_MPEG/L2", CodecId::Mp2),
    ("A_MPEG/L1", CodecId::Mp1),
    ("A_ALAC", CodecId::Alac),
    ("A_WAVPACK4", CodecId::Wavpack),
    ("A_TTA1", CodecId::Tta),
    ("S_TEXT/UTF8", CodecId::Subrip),
    ("S_TEXT/ASCII", CodecId::Text),
    ("S_TEXT/ASS", CodecId::Ass),
    ("S_TEXT/SSA", CodecId::Ass),
    ("S_ASS", CodecId::Ass),
    ("S_SSA", CodecId::Ass),
    ("S_TEXT/WEBVTT", CodecId::WebVtt),
    ("D_WEBVTT/SUBTITLES", CodecId::WebVtt),
    ("D_WEBVTT/CAPTIONS", CodecId::WebVtt),
    ("D_WEBVTT/DESCRIPTIONS", CodecId::WebVtt),
    ("D_WEBVTT/METADATA", CodecId::WebVtt),
    ("S_HDMV/PGS", CodecId::HdmvPgs),
    ("S_HDMV/TEXTST", CodecId::HdmvText),
    ("S_VOBSUB", CodecId::DvdSub),
    ("S_DVBSUB", CodecId::DvbSub),
];

fn codec_for(codec_id: &str, bit_depth: Option<u64>) -> CodecId {
    match (codec_id, bit_depth) {
        ("A_PCM/INT/LIT", Some(8)) => return CodecId::PcmU8,
        ("A_PCM/INT/LIT", Some(24)) => return CodecId::PcmS24Le,
        ("A_PCM/INT/LIT", Some(32)) => return CodecId::PcmS32Le,
        ("A_PCM/INT/LIT", _) => return CodecId::PcmS16Le,
        ("A_PCM/INT/BIG", Some(24)) => return CodecId::PcmS24Be,
        ("A_PCM/INT/BIG", Some(32)) => return CodecId::PcmS32Be,
        ("A_PCM/INT/BIG", _) => return CodecId::PcmS16Be,
        ("A_PCM/FLOAT/IEEE", Some(64)) => return CodecId::PcmF64Le,
        ("A_PCM/FLOAT/IEEE", _) => return CodecId::PcmF32Le,
        _ => {}
    }
    CODECS
        .iter()
        .find(|(prefix, _)| codec_id.starts_with(prefix))
        .map_or(CodecId::Unknown, |&(_, c)| c)
}

/// Attachment MIME mapping, after FFmpeg's `ff_mkv_mime_tags` / `ff_mkv_image_mime_tags`.
fn attachment_codec(mime: &str) -> (CodecId, bool) {
    match mime {
        "image/gif" => (CodecId::Gif, true),
        "image/jpeg" => (CodecId::Mjpeg, true),
        "image/png" => (CodecId::Png, true),
        "image/tiff" => (CodecId::Tiff, true),
        "application/x-truetype-font"
        | "application/x-font"
        | "application/font-sfnt"
        | "font/ttf"
        | "font/sfnt"
        | "font/collection" => (CodecId::Ttf, false),
        "application/vnd.ms-opentype" | "font/otf" => (CodecId::Otf, false),
        "binary" => (CodecId::BinData, false),
        _ => (CodecId::Unknown, false),
    }
}

#[derive(Default)]
struct TrackEntry {
    number: u64,
    track_type: u64,
    flag_default: Option<u64>,
    flag_forced: u64,
    default_duration: u64,
    name: Option<String>,
    language: Option<String>,
    codec_id: String,
    codec_private: Vec<u8>,
    codec_delay: u64,
    video: VideoParams,
    sampling_frequency: Option<f64>,
    output_sampling_frequency: Option<f64>,
    channels: Option<u64>,
    bit_depth: Option<u64>,
    /// (ContentEncodingScope, encoding), in declaration order.
    encodings: Vec<(u64, FrameEncoding)>,
    unsupported_encoding: Option<String>,
}

fn parse_track_entry(data: &[u8]) -> Result<TrackEntry> {
    let mut t = TrackEntry::default();
    for child in children(data) {
        let (id, d) = child?;
        match id {
            ids::TRACK_NUMBER => t.number = uint(d)?,
            ids::TRACK_TYPE => t.track_type = uint(d)?,
            ids::FLAG_DEFAULT => t.flag_default = Some(uint(d)?),
            ids::FLAG_FORCED => t.flag_forced = uint(d)?,
            ids::DEFAULT_DURATION => t.default_duration = uint(d)?,
            ids::NAME => t.name = Some(string(d)),
            ids::LANGUAGE => t.language = Some(string(d)),
            ids::CODEC_ID => t.codec_id = string(d),
            ids::CODEC_PRIVATE => t.codec_private = d.to_vec(),
            ids::CODEC_DELAY => t.codec_delay = uint(d)?,
            ids::VIDEO => {
                for child in children(d) {
                    let (id, d) = child?;
                    let to_u32 = |d| uint(d).map(|v| u32::try_from(v).unwrap_or(u32::MAX));
                    match id {
                        ids::PIXEL_WIDTH => t.video.width = to_u32(d)?,
                        ids::PIXEL_HEIGHT => t.video.height = to_u32(d)?,
                        ids::DISPLAY_WIDTH => t.video.display_width = Some(to_u32(d)?),
                        ids::DISPLAY_HEIGHT => t.video.display_height = Some(to_u32(d)?),
                        _ => {}
                    }
                }
            }
            ids::AUDIO => {
                for child in children(d) {
                    let (id, d) = child?;
                    match id {
                        ids::SAMPLING_FREQUENCY => t.sampling_frequency = Some(float(d)?),
                        ids::OUTPUT_SAMPLING_FREQUENCY => {
                            t.output_sampling_frequency = Some(float(d)?)
                        }
                        ids::CHANNELS => t.channels = Some(uint(d)?),
                        ids::BIT_DEPTH => t.bit_depth = Some(uint(d)?),
                        _ => {}
                    }
                }
            }
            ids::CONTENT_ENCODINGS => parse_content_encodings(d, &mut t)?,
            _ => {}
        }
    }
    Ok(t)
}

fn parse_content_encodings(data: &[u8], t: &mut TrackEntry) -> Result<()> {
    for child in children(data) {
        let (id, d) = child?;
        if id != ids::CONTENT_ENCODING {
            continue;
        }
        let (mut scope, mut kind, mut algo, mut settings) = (1, 0, 0, Vec::new());
        for child in children(d) {
            let (id, d) = child?;
            match id {
                ids::CONTENT_ENCODING_SCOPE => scope = uint(d)?,
                ids::CONTENT_ENCODING_TYPE => kind = uint(d)?,
                ids::CONTENT_COMPRESSION => {
                    for child in children(d) {
                        let (id, d) = child?;
                        match id {
                            ids::CONTENT_COMP_ALGO => algo = uint(d)?,
                            ids::CONTENT_COMP_SETTINGS => settings = d.to_vec(),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        match (kind, algo) {
            (0, 0) => t.encodings.push((scope, FrameEncoding::Zlib)),
            (0, 3) => t
                .encodings
                .push((scope, FrameEncoding::HeaderStrip(settings))),
            (0, a) => t.unsupported_encoding = Some(format!("compression algorithm {a}")),
            _ => t.unsupported_encoding = Some("encryption".into()),
        }
    }
    Ok(())
}

/// Scope bit 1 covers frames, bit 2 covers CodecPrivate.
fn scoped(encodings: &[(u64, FrameEncoding)], bit: u64) -> Vec<FrameEncoding> {
    encodings
        .iter()
        .filter(|(scope, _)| scope & bit != 0)
        .map(|(_, e)| e.clone())
        .collect()
}

/// Builds streams for every usable track entry, in file order.
pub(crate) fn parse_tracks(
    data: &[u8],
    timestamp_scale: u64,
    limits: &Limits,
    streams: &mut Vec<StreamInfo>,
    tracks: &mut Vec<Track>,
) -> Result<()> {
    let time_base = Rational::new(
        i64::try_from(timestamp_scale).unwrap_or(i64::MAX),
        1_000_000_000,
    );
    for child in children(data) {
        let (id, d) = child?;
        if id != ids::TRACK_ENTRY {
            continue;
        }
        let mut t = parse_track_entry(d)?;
        let kind = match t.track_type {
            TRACK_TYPE_VIDEO => MediaKind::Video,
            TRACK_TYPE_AUDIO => MediaKind::Audio,
            TRACK_TYPE_SUBTITLE => MediaKind::Subtitle,
            // WebM writes WebVTT tracks as metadata tracks.
            TRACK_TYPE_METADATA if t.codec_id.starts_with("D_WEBVTT/") => MediaKind::Subtitle,
            // Logo, buttons, control, complex: nothing a media server plays.
            _ => continue,
        };
        // FFmpeg drops tracks with no number, no codec, or encodings it can't undo.
        if t.number == 0 || t.codec_id.is_empty() || t.unsupported_encoding.is_some() {
            continue;
        }
        if tracks.iter().any(|tr| tr.number == t.number) {
            continue;
        }
        let private_encodings = scoped(&t.encodings, 2);
        if !private_encodings.is_empty() {
            t.codec_private = decode(&private_encodings, &t.codec_private, limits)?;
        }
        let codec = codec_for(&t.codec_id, t.bit_depth);
        // FLAC CodecPrivate starts with the "fLaC" marker and a metadata block
        // header; FFmpeg (and MP4's dfLa) keep only what follows.
        if codec == CodecId::Flac
            && t.codec_private.len() >= 8
            && t.codec_private.starts_with(b"fLaC")
        {
            t.codec_private.drain(..8);
        }
        if streams.len() >= limits.max_streams {
            return Err(Error::resource(format!(
                "more than {} streams",
                limits.max_streams
            )));
        }

        let mut tags = BTreeMap::new();
        // Matroska's default language is "eng"; FFmpeg omits "und".
        let language = t.language.clone().unwrap_or_else(|| "eng".into());
        if language != "und" {
            tags.insert("language".into(), language);
        }
        if let Some(name) = t.name.clone() {
            tags.insert("title".into(), name);
        }

        let audio = (kind == MediaKind::Audio).then(|| {
            let rate = t
                .output_sampling_frequency
                .or(t.sampling_frequency)
                .unwrap_or(8000.0);
            AudioParams {
                sample_rate: rate as u32,
                channels: u32::try_from(t.channels.unwrap_or(1)).unwrap_or(u32::MAX),
                bits_per_sample: t.bit_depth.and_then(|b| u32::try_from(b).ok()),
            }
        });

        let index = streams.len();
        streams.push(StreamInfo {
            index,
            kind,
            codec,
            container_codec: t.codec_id.clone(),
            time_base,
            extradata: t.codec_private,
            video: (kind == MediaKind::Video).then_some(t.video),
            audio,
            disposition: Disposition {
                default: t.flag_default.unwrap_or(1) != 0,
                forced: t.flag_forced != 0,
                attached_pic: false,
            },
            tags,
        });

        let codec_delay = Rational::rescale(
            i64::try_from(t.codec_delay).unwrap_or(i64::MAX),
            Rational::new(1, 1_000_000_000),
            time_base,
        );
        tracks.push(Track {
            number: t.number,
            stream_index: index,
            default_duration_ns: t.default_duration,
            codec_delay,
            encodings: scoped(&t.encodings, 1),
            webm_webvtt: t.codec_id.starts_with("D_WEBVTT/"),
        });
    }
    Ok(())
}

/// A file attached to the segment, already mapped to a stream. Images become
/// cover-art video streams with one packet (`picture`).
pub(crate) struct Attachment {
    pub stream: StreamInfo,
    pub picture: Option<Vec<u8>>,
}

pub(crate) fn parse_attachments(data: &[u8]) -> Result<Vec<Attachment>> {
    let mut out = Vec::new();
    for child in children(data) {
        let (id, d) = child?;
        if id != ids::ATTACHED_FILE {
            continue;
        }
        let (mut name, mut mime, mut file) = (None, None, None);
        for child in children(d) {
            let (id, d) = child?;
            match id {
                ids::FILE_NAME => name = Some(string(d)),
                ids::FILE_MIME_TYPE => mime = Some(string(d)),
                ids::FILE_DATA => file = Some(d.to_vec()),
                _ => {}
            }
        }
        // FFmpeg skips incomplete attachments.
        let (Some(name), Some(mime), Some(file)) = (name, mime, file) else {
            continue;
        };
        if file.is_empty() {
            continue;
        }
        let (codec, is_image) = attachment_codec(&mime);
        let mut tags = BTreeMap::new();
        tags.insert("filename".into(), name);
        tags.insert("mimetype".into(), mime);
        let (kind, extradata, picture) = if is_image {
            (MediaKind::Video, Vec::new(), Some(file))
        } else {
            (MediaKind::Attachment, file, None)
        };
        out.push(Attachment {
            stream: StreamInfo {
                index: 0,
                kind,
                codec,
                container_codec: String::new(),
                time_base: Rational::new(1, 90_000),
                extradata,
                video: None,
                audio: None,
                disposition: Disposition {
                    attached_pic: is_image,
                    ..Disposition::default()
                },
                tags,
            },
            picture,
        });
    }
    Ok(out)
}

/// Splits a WebM WebVTT frame into (identifier, settings, payload), as
/// FFmpeg's `matroska_parse_webvtt` does. Trailing line breaks are trimmed
/// from the payload; an empty payload is invalid.
pub(crate) fn split_webm_webvtt(frame: &[u8]) -> Result<(&[u8], &[u8], &[u8])> {
    fn line(d: &[u8]) -> Result<(&[u8], &[u8])> {
        let end = d
            .iter()
            .position(|&b| b == b'\r' || b == b'\n')
            .ok_or_else(|| Error::corrupt("WebVTT frame: missing line break"))?;
        let skip = if d[end..].starts_with(b"\r\n") { 2 } else { 1 };
        Ok((&d[..end], &d[end + skip..]))
    }
    let (id, rest) = line(frame)?;
    let (settings, mut payload) = line(rest)?;
    while let [head @ .., b'\r' | b'\n'] = payload {
        payload = head;
    }
    if payload.is_empty() {
        return Err(Error::corrupt("WebVTT frame: empty cue"));
    }
    Ok((id, settings, payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codec_prefix_matching() {
        assert_eq!(codec_for("A_AAC/MPEG4/LC/SBR", None), CodecId::Aac);
        assert_eq!(codec_for("V_MPEG4/ISO/AVC", None), CodecId::H264);
        assert_eq!(codec_for("V_MPEG4/ISO/ASP", None), CodecId::Mpeg4);
        assert_eq!(codec_for("A_PCM/INT/LIT", Some(24)), CodecId::PcmS24Le);
        assert_eq!(codec_for("V_QUICKTIME", None), CodecId::Unknown);
    }

    #[test]
    fn webm_webvtt_frames() {
        assert_eq!(
            split_webm_webvtt(b"\n\nHello\n").unwrap(),
            (&b""[..], &b""[..], &b"Hello"[..])
        );
        assert_eq!(
            split_webm_webvtt(b"cue1\r\nline:0\nA\nB").unwrap(),
            (&b"cue1"[..], &b"line:0"[..], &b"A\nB"[..])
        );
        assert!(split_webm_webvtt(b"no breaks").is_err());
        assert!(split_webm_webvtt(b"\n\n\n").is_err());
    }

    #[test]
    fn header_strip_then_restore() {
        let track = Track {
            number: 1,
            stream_index: 0,
            default_duration_ns: 0,
            codec_delay: 0,
            encodings: vec![FrameEncoding::HeaderStrip(vec![0x0B, 0x77])],
            webm_webvtt: false,
        };
        assert_eq!(
            track.decode_frame(&[1, 2], &Limits::default()).unwrap(),
            vec![0x0B, 0x77, 1, 2]
        );
    }
}
