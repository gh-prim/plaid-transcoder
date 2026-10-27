//! Memory-safe demuxers. Parsers in this crate never use `unsafe` and check
//! every file-controlled size against [`plaid_core::Limits`] before allocating.

pub mod mkv;

use std::io::{Read, Seek, SeekFrom};

use plaid_core::{Demuxer, Error, Limits, Result};

/// Detects the container from its magic bytes and opens a demuxer for it.
pub fn open<R>(mut input: R, limits: Limits) -> Result<Box<dyn Demuxer>>
where
    R: Read + Seek + Send + 'static,
{
    let mut magic = [0u8; 4];
    let n = input.read(&mut magic)?;
    input.seek(SeekFrom::Start(0))?;
    if n == 4 && magic == mkv::MAGIC {
        return Ok(Box::new(mkv::MkvDemuxer::with_limits(input, limits)?));
    }
    Err(Error::unsupported("unrecognised container"))
}
