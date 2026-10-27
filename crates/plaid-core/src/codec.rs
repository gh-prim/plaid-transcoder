/// What a stream carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediaKind {
    Video,
    Audio,
    Subtitle,
    Data,
    Attachment,
}

impl MediaKind {
    /// Same spelling as FFmpeg's `codec_type`.
    pub fn name(self) -> &'static str {
        match self {
            MediaKind::Video => "video",
            MediaKind::Audio => "audio",
            MediaKind::Subtitle => "subtitle",
            MediaKind::Data => "data",
            MediaKind::Attachment => "attachment",
        }
    }
}

macro_rules! codecs {
    ($($variant:ident => $name:literal,)*) => {
        /// Codec identifiers. Only formats a media server meets in practice
        /// are listed; anything else is `Unknown` and goes to the fallback.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        pub enum CodecId {
            $($variant,)*
            Unknown,
        }

        impl CodecId {
            /// Short name, identical to FFmpeg's `codec_name` so the
            /// differential harness can compare without a mapping table.
            pub fn name(self) -> &'static str {
                match self {
                    $(CodecId::$variant => $name,)*
                    CodecId::Unknown => "none",
                }
            }
        }
    };
}

codecs! {
    // Video
    H264 => "h264",
    Hevc => "hevc",
    Av1 => "av1",
    Vp9 => "vp9",
    Vp8 => "vp8",
    Mpeg1Video => "mpeg1video",
    Mpeg2Video => "mpeg2video",
    Mpeg4 => "mpeg4",
    Vc1 => "vc1",
    Theora => "theora",
    Prores => "prores",
    Ffv1 => "ffv1",
    Mjpeg => "mjpeg",
    Png => "png",
    Gif => "gif",
    Tiff => "tiff",
    RawVideo => "rawvideo",
    // Audio
    Aac => "aac",
    Ac3 => "ac3",
    Eac3 => "eac3",
    Dts => "dts",
    TrueHd => "truehd",
    Mlp => "mlp",
    Flac => "flac",
    Opus => "opus",
    Vorbis => "vorbis",
    Mp1 => "mp1",
    Mp2 => "mp2",
    Mp3 => "mp3",
    Alac => "alac",
    Wavpack => "wavpack",
    Tta => "tta",
    PcmU8 => "pcm_u8",
    PcmS16Le => "pcm_s16le",
    PcmS16Be => "pcm_s16be",
    PcmS24Le => "pcm_s24le",
    PcmS24Be => "pcm_s24be",
    PcmS32Le => "pcm_s32le",
    PcmS32Be => "pcm_s32be",
    PcmF32Le => "pcm_f32le",
    PcmF64Le => "pcm_f64le",
    // Subtitles
    Subrip => "subrip",
    Ass => "ass",
    WebVtt => "webvtt",
    Text => "text",
    HdmvPgs => "hdmv_pgs_subtitle",
    HdmvText => "hdmv_text_subtitle",
    DvdSub => "dvd_subtitle",
    DvbSub => "dvb_subtitle",
    // Attachments
    Ttf => "ttf",
    Otf => "otf",
    BinData => "bin_data",
}
