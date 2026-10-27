//! Block / SimpleBlock payload layout (RFC 9559 §10): track number, relative
//! timestamp, flags, then one frame or several laced frames.

use std::ops::Range;

use plaid_core::{Error, Result};

use super::ebml::{parse_signed_vint, parse_vint};

pub(crate) struct BlockHeader {
    pub track: u64,
    pub rel_timestamp: i16,
    pub flags: u8,
    pub len: usize,
}

impl BlockHeader {
    pub fn keyframe(&self) -> bool {
        self.flags & 0x80 != 0
    }

    fn lacing(&self) -> u8 {
        (self.flags >> 1) & 0x03
    }
}

pub(crate) fn parse_header(data: &[u8]) -> Result<BlockHeader> {
    let (track, n) =
        parse_vint(data).ok_or_else(|| Error::corrupt("block: invalid track number"))?;
    let rest = data
        .get(n..n + 3)
        .ok_or_else(|| Error::corrupt("block: header truncated"))?;
    Ok(BlockHeader {
        track,
        rel_timestamp: i16::from_be_bytes([rest[0], rest[1]]),
        flags: rest[2],
        len: n + 3,
    })
}

/// Byte ranges of each frame within `data`.
pub(crate) fn frames(data: &[u8], header: &BlockHeader) -> Result<Vec<Range<usize>>> {
    let mut p = header.len;
    if header.lacing() == 0 {
        // One frame (a single range, not the range expanded).
        return Ok(std::iter::once(p..data.len()).collect());
    }

    let count = usize::from(
        *data
            .get(p)
            .ok_or_else(|| Error::corrupt("block: missing lace count"))?,
    ) + 1;
    p += 1;

    let mut sizes = Vec::with_capacity(count);
    match header.lacing() {
        // Xiph: each size is a run of 255s plus a final byte.
        1 => {
            for _ in 0..count - 1 {
                let mut size = 0usize;
                loop {
                    let b = *data
                        .get(p)
                        .ok_or_else(|| Error::corrupt("block: Xiph lacing truncated"))?;
                    p += 1;
                    size += usize::from(b);
                    if b != 255 {
                        break;
                    }
                }
                sizes.push(size);
            }
        }
        // Fixed: equal split of the remainder.
        2 => {
            let remaining = data.len() - p;
            if remaining % count != 0 {
                return Err(Error::corrupt("block: fixed lacing does not divide evenly"));
            }
            sizes.resize(count - 1, remaining / count);
        }
        // EBML: first size unsigned, then signed differences.
        _ => {
            if count > 1 {
                let (first, n) = parse_vint(&data[p..])
                    .ok_or_else(|| Error::corrupt("block: EBML lacing truncated"))?;
                p += n;
                let mut size = i64::try_from(first)
                    .map_err(|_| Error::corrupt("block: EBML lace too large"))?;
                sizes.push(usize::try_from(size).unwrap_or(usize::MAX));
                for _ in 1..count - 1 {
                    let (delta, n) = parse_signed_vint(&data[p..])
                        .ok_or_else(|| Error::corrupt("block: EBML lacing truncated"))?;
                    p += n;
                    size = size
                        .checked_add(delta)
                        .filter(|&s| s >= 0)
                        .ok_or_else(|| Error::corrupt("block: negative lace size"))?;
                    sizes.push(size as usize);
                }
            }
        }
    }

    let mut ranges = Vec::with_capacity(count);
    for size in sizes {
        let end = p
            .checked_add(size)
            .filter(|&e| e <= data.len())
            .ok_or_else(|| Error::corrupt("block: lace sizes exceed block"))?;
        ranges.push(p..end);
        p = end;
    }
    ranges.push(p..data.len());
    Ok(ranges)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames_of(data: &[u8]) -> Vec<Vec<u8>> {
        let h = parse_header(data).unwrap();
        frames(data, &h)
            .unwrap()
            .into_iter()
            .map(|r| data[r].to_vec())
            .collect()
    }

    #[test]
    fn no_lacing() {
        assert_eq!(
            frames_of(&[0x81, 0x00, 0x05, 0x80, 1, 2, 3]),
            vec![vec![1, 2, 3]]
        );
    }

    #[test]
    fn xiph_lacing() {
        // 3 frames: sizes 2, 256 (255+1), remainder 1.
        let mut d = vec![0x81, 0, 0, 0x02, 2, 2, 255, 1];
        d.extend([7; 2]);
        d.extend([8; 256]);
        d.push(9);
        let f = frames_of(&d);
        assert_eq!(f.iter().map(Vec::len).collect::<Vec<_>>(), vec![2, 256, 1]);
        assert_eq!(f[2], vec![9]);
    }

    #[test]
    fn fixed_lacing() {
        let d = [0x81, 0, 0, 0x04, 1, 1, 2, 3, 4];
        assert_eq!(frames_of(&d), vec![vec![1, 2], vec![3, 4]]);
    }

    #[test]
    fn ebml_lacing() {
        // 3 frames: 3, then 3 + (-1) = 2, remainder 1.
        let d = [0x81, 0, 0, 0x06, 2, 0x83, 0xBE, 1, 1, 1, 2, 2, 3];
        assert_eq!(frames_of(&d), vec![vec![1, 1, 1], vec![2, 2], vec![3]]);
    }

    #[test]
    fn rejects_bad_lacing() {
        let d = [0x81, 0, 0, 0x02, 1, 200, 1, 2];
        let h = parse_header(&d).unwrap();
        assert!(frames(&d, &h).is_err());
        let d = [0x81, 0, 0, 0x04, 1, 1, 2, 3];
        let h = parse_header(&d).unwrap();
        assert!(
            frames(&d, &h).is_err(),
            "3 bytes cannot split into 2 equal frames"
        );
    }
}
