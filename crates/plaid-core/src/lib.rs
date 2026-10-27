//! Core types shared by every plaid crate.

mod codec;
mod error;
mod packet;
mod rational;
mod stream;

pub use codec::{CodecId, MediaKind};
pub use error::{Error, ErrorClass, Result};
pub use packet::{Packet, SideData};
pub use rational::Rational;
pub use stream::{AudioParams, Disposition, FormatInfo, StreamInfo, VideoParams};

/// Hard caps applied while parsing untrusted input. Every allocation sized by
/// the file is checked against one of these before it happens.
#[derive(Debug, Clone)]
pub struct Limits {
    /// Metadata elements read whole into memory (track headers, block groups...).
    pub max_element_size: u64,
    /// A single compressed frame, before and after content decoding.
    pub max_frame_size: u64,
    /// Attachments (fonts, cover art).
    pub max_attachment_size: u64,
    pub max_streams: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_element_size: 16 << 20,
            max_frame_size: 256 << 20,
            max_attachment_size: 64 << 20,
            max_streams: 1024,
        }
    }
}

/// A source of packets from a container.
pub trait Demuxer: Send {
    fn format(&self) -> &FormatInfo;

    fn streams(&self) -> &[StreamInfo];

    /// Returns the next packet in file order, `Ok(None)` at end of input.
    ///
    /// An `Err` of class [`ErrorClass::Corrupt`] means some data was skipped;
    /// calling again resumes after the damage.
    fn read_packet(&mut self) -> Result<Option<Packet>>;
}
