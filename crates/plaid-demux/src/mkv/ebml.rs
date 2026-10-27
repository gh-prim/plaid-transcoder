//! EBML primitives (RFC 8794): variable-size integers, element headers and
//! typed values. Slice helpers parse in-memory masters; [`Reader`] walks the
//! file for the parts too large to load (segment, clusters).

use std::io::{BufReader, Read, Seek, SeekFrom};

use plaid_core::{Error, Result};

/// Length of a vint from its first byte; `None` for the invalid 0x00.
fn vint_len(first: u8) -> Option<usize> {
    (first != 0).then(|| first.leading_zeros() as usize + 1)
}

/// Raw vint value with the length marker removed.
pub(crate) fn parse_vint(buf: &[u8]) -> Option<(u64, usize)> {
    let first = *buf.first()?;
    let len = vint_len(first)?;
    let bytes = buf.get(..len)?;
    let mut value = u64::from(first) & (0xFF_u64 >> len);
    for &b in &bytes[1..] {
        value = (value << 8) | u64::from(b);
    }
    Some((value, len))
}

/// Element ID: 1–4 bytes, marker kept (the spec writes IDs that way).
pub(crate) fn parse_id(buf: &[u8]) -> Option<(u32, usize)> {
    let len = vint_len(*buf.first()?)?;
    if len > 4 {
        return None;
    }
    let id = buf
        .get(..len)?
        .iter()
        .fold(0u32, |acc, &b| (acc << 8) | u32::from(b));
    Some((id, len))
}

/// Element data size; `Some((None, _))` is the reserved "unknown size".
pub(crate) fn parse_size(buf: &[u8]) -> Option<(Option<u64>, usize)> {
    let (value, len) = parse_vint(buf)?;
    let unknown = (1u64 << (7 * len)) - 1;
    Some(((value != unknown).then_some(value), len))
}

/// Signed vint used by EBML lacing: the raw value minus half its range.
pub(crate) fn parse_signed_vint(buf: &[u8]) -> Option<(i64, usize)> {
    let (value, len) = parse_vint(buf)?;
    let bias = (1i64 << (7 * len - 1)) - 1;
    Some((value as i64 - bias, len))
}

/// Iterates the children of an in-memory master element.
pub(crate) struct Children<'a> {
    data: &'a [u8],
    pos: usize,
}

pub(crate) fn children(data: &[u8]) -> Children<'_> {
    Children { data, pos: 0 }
}

impl<'a> Iterator for Children<'a> {
    type Item = Result<(u32, &'a [u8])>;

    fn next(&mut self) -> Option<Self::Item> {
        let rest = self.data.get(self.pos..).filter(|r| !r.is_empty())?;
        let header = parse_id(rest)
            .and_then(|(id, il)| parse_size(&rest[il..]).map(|(size, sl)| (id, size, il + sl)));
        let Some((id, size, header_len)) = header else {
            self.pos = self.data.len();
            return Some(Err(Error::corrupt("invalid EBML element header")));
        };
        let avail = rest.len() - header_len;
        // A child overrunning its parent (or of unknown size) is clamped to
        // the parent's end, as FFmpeg does, rather than rejecting the parent.
        let len = match size {
            Some(s) if s <= avail as u64 => s as usize,
            _ => avail,
        };
        self.pos += header_len + len;
        Some(Ok((id, &rest[header_len..header_len + len])))
    }
}

pub(crate) fn uint(d: &[u8]) -> Result<u64> {
    if d.len() > 8 {
        return Err(Error::corrupt("unsigned integer longer than 8 bytes"));
    }
    Ok(d.iter().fold(0u64, |acc, &b| (acc << 8) | u64::from(b)))
}

pub(crate) fn float(d: &[u8]) -> Result<f64> {
    match d.len() {
        0 => Ok(0.0),
        4 => Ok(f64::from(f32::from_be_bytes(
            d.try_into().unwrap_or_default(),
        ))),
        8 => Ok(f64::from_be_bytes(d.try_into().unwrap_or_default())),
        n => Err(Error::corrupt(format!("float of invalid length {n}"))),
    }
}

/// String/UTF-8 element. Trailing NUL padding is allowed by the spec.
pub(crate) fn string(d: &[u8]) -> String {
    let end = d.iter().position(|&b| b == 0).unwrap_or(d.len());
    String::from_utf8_lossy(&d[..end]).into_owned()
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ElementHeader {
    pub id: u32,
    pub size: Option<u64>,
    /// Offset of the ID's first byte.
    pub start: u64,
    pub data_start: u64,
}

impl ElementHeader {
    pub fn end(&self) -> Option<u64> {
        self.size.map(|s| self.data_start.saturating_add(s))
    }
}

/// Position-tracking buffered reader over the input.
pub(crate) struct Reader<R> {
    inner: BufReader<R>,
    pos: u64,
    len: u64,
}

impl<R: Read + Seek> Reader<R> {
    pub fn new(mut inner: R) -> Result<Self> {
        let len = inner.seek(SeekFrom::End(0))?;
        inner.seek(SeekFrom::Start(0))?;
        Ok(Self {
            inner: BufReader::with_capacity(64 * 1024, inner),
            pos: 0,
            len,
        })
    }

    pub fn pos(&self) -> u64 {
        self.pos
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    fn remaining(&self) -> u64 {
        self.len.saturating_sub(self.pos)
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> Result<()> {
        self.inner.read_exact(buf)?;
        self.pos += buf.len() as u64;
        Ok(())
    }

    fn read_u8(&mut self) -> Result<u8> {
        let mut b = [0u8];
        self.read_exact(&mut b)?;
        Ok(b[0])
    }

    pub fn read_header(&mut self) -> Result<ElementHeader> {
        let start = self.pos;
        let mut buf = [0u8; 8];

        buf[0] = self.read_u8()?;
        let id_len = vint_len(buf[0])
            .filter(|&l| l <= 4)
            .ok_or_else(|| Error::corrupt(format!("invalid element ID at offset {start}")))?;
        self.read_exact(&mut buf[1..id_len])?;
        let (id, _) = parse_id(&buf[..id_len])
            .ok_or_else(|| Error::corrupt(format!("invalid element ID at offset {start}")))?;

        buf[0] = self.read_u8()?;
        let size_len = vint_len(buf[0])
            .ok_or_else(|| Error::corrupt(format!("invalid element size at offset {start}")))?;
        self.read_exact(&mut buf[1..size_len])?;
        let (size, _) = parse_size(&buf[..size_len])
            .ok_or_else(|| Error::corrupt(format!("invalid element size at offset {start}")))?;

        Ok(ElementHeader {
            id,
            size,
            start,
            data_start: self.pos,
        })
    }

    /// Reads `size` bytes after checking them against `limit` and the file length,
    /// so a forged size can never trigger a large allocation.
    pub fn read_data(&mut self, size: u64, limit: u64, what: &str) -> Result<Vec<u8>> {
        if size > limit {
            return Err(Error::resource(format!(
                "{what} of {size} bytes exceeds limit of {limit}"
            )));
        }
        if size > self.remaining() {
            return Err(Error::truncated(format!("{what} extends past end of file")));
        }
        let mut data = vec![0u8; size as usize];
        self.read_exact(&mut data)?;
        Ok(data)
    }

    pub fn skip(&mut self, n: u64) -> Result<()> {
        if n > self.remaining() {
            self.seek(self.len)?;
            return Err(Error::truncated("element extends past end of file"));
        }
        // n <= file length, which always fits in i64 for real files.
        self.inner.seek_relative(n as i64)?;
        self.pos += n;
        Ok(())
    }

    pub fn seek(&mut self, pos: u64) -> Result<()> {
        self.inner.seek(SeekFrom::Start(pos))?;
        self.pos = pos;
        Ok(())
    }

    /// Scans forward for the 4-byte `id` and leaves the reader on it.
    /// Returns false at end of file.
    pub fn scan_for(&mut self, id: u32) -> Result<bool> {
        let mut window = 0u32;
        let mut seen = 0;
        loop {
            match self.read_u8() {
                Ok(b) => {
                    window = (window << 8) | u32::from(b);
                    seen += 1;
                    if seen >= 4 && window == id {
                        self.seek(self.pos - 4)?;
                        return Ok(true);
                    }
                }
                Err(e) if e.is_truncated() => return Ok(false),
                Err(e) => return Err(e),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vints() {
        assert_eq!(parse_vint(&[0x81]), Some((1, 1)));
        assert_eq!(parse_vint(&[0x40, 0x02]), Some((2, 2)));
        assert_eq!(parse_vint(&[0x00]), None);
        assert_eq!(parse_vint(&[0x40]), None, "truncated");
        assert_eq!(parse_size(&[0xFF]), Some((None, 1)), "unknown size");
        assert_eq!(
            parse_size(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]),
            Some((None, 8))
        );
        assert_eq!(parse_id(&[0x1A, 0x45, 0xDF, 0xA3]), Some((0x1A45DFA3, 4)));
        assert_eq!(
            parse_id(&[0x08, 0, 0, 0, 0]),
            None,
            "IDs are at most 4 bytes"
        );
        assert_eq!(parse_signed_vint(&[0xBF]), Some((0, 1)));
        assert_eq!(parse_signed_vint(&[0x80]), Some((-63, 1)));
    }

    #[test]
    fn children_clamp_overruns() {
        // Child claims 5 bytes but only 2 remain in the parent.
        let data = [0x83, 0x85, 0xAA, 0xBB];
        let kids: Vec<_> = children(&data).collect::<Result<_>>().unwrap();
        assert_eq!(kids, vec![(0x83, &[0xAA, 0xBB][..])]);
    }
}
