//! Matroska / WebM demuxer (RFC 9559, RFC 8794).
//!
//! Reads headers once at open, then streams clusters. Timestamps and packet
//! boundaries follow FFmpeg's matroskadec so the differential harness can
//! compare packet-for-packet.

mod block;
mod ebml;
mod ids;
mod tracks;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Seek};

use plaid_core::{
    Demuxer, Error, ErrorClass, FormatInfo, Limits, Packet, Result, SideData, StreamInfo,
};

use ebml::{ElementHeader, Reader, children, float, string, uint};
use tracks::Track;

/// Magic bytes at offset 0 of every Matroska/WebM file.
pub const MAGIC: [u8; 4] = [0x1A, 0x45, 0xDF, 0xA3];

const DEFAULT_TIMESTAMP_SCALE: u64 = 1_000_000;

#[derive(Debug, Clone, Copy)]
enum State {
    /// Between top-level elements of the segment.
    Segment,
    /// Inside a cluster; `end` is None for unknown-size (live) clusters.
    Cluster {
        end: Option<u64>,
    },
    /// Scanning for the next cluster after damage, from this offset.
    Resync {
        from: u64,
    },
    Done,
}

pub struct MkvDemuxer<R> {
    reader: Reader<R>,
    limits: Limits,
    format: FormatInfo,
    streams: Vec<StreamInfo>,
    tracks: Vec<Track>,
    track_by_number: HashMap<u64, usize>,
    timestamp_scale: u64,
    segment_end: Option<u64>,
    state: State,
    cluster_timestamp: u64,
    queue: VecDeque<Packet>,
    /// Error for a block that was skipped; reported on the next read.
    dropped_block: Option<Error>,
}

/// What the BlockGroup around a Block told us.
struct GroupInfo {
    duration: Option<u64>,
    has_reference: bool,
}

impl<R: Read + Seek + Send> MkvDemuxer<R> {
    pub fn open(input: R) -> Result<Self> {
        Self::with_limits(input, Limits::default())
    }

    pub fn with_limits(input: R, limits: Limits) -> Result<Self> {
        let mut reader = Reader::new(input)?;
        read_ebml_header(&mut reader, &limits)?;

        // Find the Segment, skipping anything (Void, junk) before it.
        let segment = loop {
            let h = reader
                .read_header()
                .map_err(|e| eof_as("no Segment element", e))?;
            if h.id == ids::SEGMENT {
                break h;
            }
            reader.skip(
                h.size
                    .ok_or_else(|| Error::corrupt("unknown-size element before Segment"))?,
            )?;
        };

        let mut demuxer = Self {
            reader,
            limits,
            format: FormatInfo {
                name: "matroska,webm",
                ..FormatInfo::default()
            },
            streams: Vec::new(),
            tracks: Vec::new(),
            track_by_number: HashMap::new(),
            timestamp_scale: DEFAULT_TIMESTAMP_SCALE,
            segment_end: segment.end(),
            state: State::Segment,
            cluster_timestamp: 0,
            queue: VecDeque::new(),
            dropped_block: None,
        };
        demuxer.read_segment_headers(&segment)?;
        Ok(demuxer)
    }

    /// Reads Info, Tracks and Attachments up to the first Cluster, following
    /// the SeekHead for any that are stored after the clusters.
    fn read_segment_headers(&mut self, segment: &ElementHeader) -> Result<()> {
        let mut seek_targets: Vec<(u32, u64)> = Vec::new();
        let (mut info, mut tracks, mut attachments) = (None, None, None);
        let mut first_cluster = None;

        loop {
            if self.segment_end.is_some_and(|end| self.reader.pos() >= end) {
                break;
            }
            let h = match self.reader.read_header() {
                Ok(h) => h,
                Err(e) if e.is_truncated() => break,
                Err(e) => return Err(e),
            };
            match h.id {
                ids::CLUSTER => {
                    first_cluster = Some(h.start);
                    break;
                }
                ids::INFO | ids::TRACKS | ids::SEEK_HEAD | ids::ATTACHMENTS => {
                    let size = h
                        .size
                        .ok_or_else(|| Error::corrupt("unknown-size header element"))?;
                    let limit = if h.id == ids::ATTACHMENTS {
                        self.limits.max_attachment_size
                    } else {
                        self.limits.max_element_size
                    };
                    let data = self.reader.read_data(size, limit, "header element")?;
                    match h.id {
                        ids::INFO => info = info.or(Some(data)),
                        ids::TRACKS => tracks = tracks.or(Some(data)),
                        ids::ATTACHMENTS => attachments = attachments.or(Some(data)),
                        _ => seek_targets.extend(parse_seek_head(&data, segment.data_start)),
                    }
                }
                ids::SEGMENT | ids::EBML => break,
                _ => match h.size {
                    Some(size) => {
                        if let Err(e) = self.reader.skip(size) {
                            if e.is_truncated() {
                                break;
                            }
                            return Err(e);
                        }
                    }
                    None => break,
                },
            }
        }

        // Late headers, typically Attachments after the clusters.
        let resume = self.reader.pos();
        for (id, pos) in seek_targets {
            let slot = match id {
                ids::INFO => &mut info,
                ids::TRACKS => &mut tracks,
                ids::ATTACHMENTS => &mut attachments,
                _ => continue,
            };
            if slot.is_some() || pos >= self.reader.len() {
                continue;
            }
            if let Ok(data) = self.read_element_at(pos, id) {
                *slot = Some(data);
            }
        }

        if let Some(info) = info {
            self.parse_info(&info)?;
        }
        let Some(tracks) = tracks else {
            return Err(Error::unsupported("Matroska file without Tracks"));
        };
        tracks::parse_tracks(
            &tracks,
            self.timestamp_scale,
            &self.limits,
            &mut self.streams,
            &mut self.tracks,
        )?;
        self.track_by_number = self
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| (t.number, i))
            .collect();

        if let Some(attachments) = attachments {
            for att in tracks::parse_attachments(&attachments)? {
                if self.streams.len() >= self.limits.max_streams {
                    return Err(Error::resource(format!(
                        "more than {} streams",
                        self.limits.max_streams
                    )));
                }
                let mut stream = att.stream;
                stream.index = self.streams.len();
                if let Some(picture) = att.picture {
                    // Cover art is delivered before any cluster data, like FFmpeg's attached_pic.
                    self.queue.push_back(Packet {
                        stream_index: stream.index,
                        pts: None,
                        dts: None,
                        duration: None,
                        pos: None,
                        keyframe: true,
                        data: picture,
                        side_data: Vec::new(),
                    });
                }
                self.streams.push(stream);
            }
        }

        match first_cluster {
            Some(start) => self.reader.seek(start)?,
            None if resume != self.reader.pos() => self.reader.seek(resume)?,
            None => {}
        }
        self.state = if first_cluster.is_some() {
            State::Segment
        } else {
            State::Done
        };
        Ok(())
    }

    fn read_element_at(&mut self, pos: u64, expected: u32) -> Result<Vec<u8>> {
        self.reader.seek(pos)?;
        let h = self.reader.read_header()?;
        if h.id != expected {
            return Err(Error::corrupt("SeekHead points at the wrong element"));
        }
        let size = h
            .size
            .ok_or_else(|| Error::corrupt("unknown-size header element"))?;
        let limit = if expected == ids::ATTACHMENTS {
            self.limits.max_attachment_size
        } else {
            self.limits.max_element_size
        };
        self.reader.read_data(size, limit, "header element")
    }

    fn parse_info(&mut self, data: &[u8]) -> Result<()> {
        let mut duration = None;
        for child in children(data) {
            let (id, d) = child?;
            match id {
                ids::TIMESTAMP_SCALE => self.timestamp_scale = uint(d)?,
                ids::DURATION => duration = Some(float(d)?),
                ids::TITLE => {
                    self.format.tags.insert("title".into(), string(d));
                }
                _ => {}
            }
        }
        if self.timestamp_scale == 0 {
            self.timestamp_scale = DEFAULT_TIMESTAMP_SCALE;
        }
        self.format.duration_seconds = duration
            .filter(|d| d.is_finite() && *d > 0.0)
            .map(|d| d * self.timestamp_scale as f64 / 1e9);
        Ok(())
    }

    /// Advances by one element. Returns false at end of input.
    fn step(&mut self) -> Result<bool> {
        match self.state {
            State::Done => return Ok(false),
            State::Resync { from } => {
                self.reader.seek(from)?;
                self.state = if self.reader.scan_for(ids::CLUSTER)? {
                    State::Segment
                } else {
                    State::Done
                };
                return Ok(true);
            }
            State::Cluster { end: Some(end) } if self.reader.pos() >= end => {
                self.state = State::Segment;
                return Ok(true);
            }
            _ => {}
        }
        if self.segment_end.is_some_and(|end| self.reader.pos() >= end)
            || self.reader.pos() >= self.reader.len()
        {
            self.state = State::Done;
            return Ok(false);
        }

        let h = self.reader.read_header()?;
        if let State::Cluster { end } = self.state {
            let leaves_cluster = end.is_none() && ids::is_segment_level(h.id);
            if !leaves_cluster {
                self.cluster_child(&h, end)?;
                return Ok(true);
            }
            self.state = State::Segment;
        }

        match (h.id, h.size) {
            (ids::CLUSTER, size) => {
                // Timestamp is mandatory; FFmpeg treats a missing one as 0.
                self.cluster_timestamp = 0;
                self.state = State::Cluster {
                    end: size.map(|s| h.data_start.saturating_add(s)),
                };
            }
            // A second segment or EBML header (chained files): not supported, stop here.
            (ids::SEGMENT | ids::EBML, _) => self.state = State::Done,
            (id, Some(size))
                if ids::is_segment_level(id) || id == ids::VOID || id == ids::CRC32 =>
            {
                self.reader.skip(size)?
            }
            _ => {
                return Err(Error::corrupt(format!(
                    "unexpected element 0x{:X} at offset {}",
                    h.id, h.start
                )));
            }
        }
        Ok(true)
    }

    fn cluster_child(&mut self, h: &ElementHeader, cluster_end: Option<u64>) -> Result<()> {
        let size = h.size.ok_or_else(|| {
            Error::corrupt(format!(
                "unknown-size element inside cluster at offset {}",
                h.start
            ))
        })?;
        if cluster_end.is_some_and(|end| h.data_start.saturating_add(size) > end) {
            return Err(Error::corrupt(format!(
                "element at offset {} overruns its cluster",
                h.start
            )));
        }
        match h.id {
            ids::CLUSTER_TIMESTAMP => {
                let d = self.reader.read_data(size, 8, "cluster timestamp")?;
                self.cluster_timestamp = uint(&d)?;
            }
            // A damaged block was fully read, so the file structure is intact:
            // drop just that block and report it, without resyncing.
            ids::SIMPLE_BLOCK => {
                let data =
                    self.reader
                        .read_data(size, self.limits.max_frame_size, "SimpleBlock")?;
                self.dropped_block = self.block(&data, h.data_start, None).err();
            }
            ids::BLOCK_GROUP => {
                let data = self
                    .reader
                    .read_data(size, self.limits.max_frame_size, "BlockGroup")?;
                self.dropped_block = self.block_group(&data, h.data_start).err();
            }
            _ => self.reader.skip(size)?,
        }
        Ok(())
    }

    fn block_group(&mut self, data: &[u8], data_start: u64) -> Result<()> {
        let mut group = GroupInfo {
            duration: None,
            has_reference: false,
        };
        let mut block = None;
        let mut offset = 0u64;
        // Children are walked by hand to know each Block's file offset.
        let mut rest = data;
        while !rest.is_empty() {
            let Some((id, il)) = ebml::parse_id(rest) else {
                break;
            };
            let Some((size, sl)) = ebml::parse_size(&rest[il..]) else {
                break;
            };
            let start = il + sl;
            let len = size.map_or(rest.len() - start, |s| (s as usize).min(rest.len() - start));
            let d = &rest[start..start + len];
            match id {
                ids::BLOCK => block = Some((d, data_start + offset + start as u64)),
                ids::BLOCK_DURATION => group.duration = Some(uint(d)?),
                ids::REFERENCE_BLOCK => group.has_reference = true,
                _ => {}
            }
            offset += (start + len) as u64;
            rest = &rest[start + len..];
        }
        match block {
            Some((d, pos)) => self.block(d, pos, Some(&group)),
            None => Ok(()),
        }
    }

    fn block(&mut self, data: &[u8], pos: u64, group: Option<&GroupInfo>) -> Result<()> {
        let header = block::parse_header(data)?;
        let Some(&ti) = self.track_by_number.get(&header.track) else {
            return Ok(()); // Block for a track we dropped or never declared.
        };
        let track = &self.tracks[ti];
        let keyframe = group.map_or(header.keyframe(), |g| !g.has_reference);
        let frames = block::frames(data, &header)?;
        let laces = frames.len() as u64;

        let block_duration = match group.and_then(|g| g.duration) {
            Some(d) if d > 0 => d,
            _ => track.default_duration_ns.saturating_mul(laces) / self.timestamp_scale,
        };
        let mut pts = i64::try_from(self.cluster_timestamp)
            .ok()
            .and_then(|c| c.checked_add(i64::from(header.rel_timestamp)))
            .and_then(|t| t.checked_sub(track.codec_delay));

        for (n, range) in frames.into_iter().enumerate() {
            let n = n as u64;
            let lace_duration = block_duration * (n + 1) / laces - block_duration * n / laces;
            let lace_duration = i64::try_from(lace_duration).ok().filter(|&d| d > 0);
            let mut packet = Packet {
                stream_index: track.stream_index,
                pts,
                dts: None,
                duration: lace_duration,
                pos: Some(pos),
                keyframe,
                data: track.decode_frame(&data[range], &self.limits)?,
                side_data: Vec::new(),
            };
            if track.webm_webvtt {
                let (id, settings, payload) = tracks::split_webm_webvtt(&packet.data)?;
                if !id.is_empty() {
                    packet
                        .side_data
                        .push(SideData::WebVttIdentifier(id.to_vec()));
                }
                if !settings.is_empty() {
                    packet
                        .side_data
                        .push(SideData::WebVttSettings(settings.to_vec()));
                }
                packet.data = payload.to_vec();
                // FFmpeg rebuilds these packets without the key flag.
                packet.keyframe = false;
            }
            self.queue.push_back(packet);
            pts = pts.zip(lace_duration).and_then(|(p, d)| p.checked_add(d));
        }
        Ok(())
    }
}

impl<R: Read + Seek + Send> Demuxer for MkvDemuxer<R> {
    fn format(&self) -> &FormatInfo {
        &self.format
    }

    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }

    fn read_packet(&mut self) -> Result<Option<Packet>> {
        loop {
            if let Some(packet) = self.queue.pop_front() {
                return Ok(Some(packet));
            }
            if let Some(e) = self.dropped_block.take() {
                return Err(e);
            }
            let element_start = self.reader.pos();
            match self.step() {
                Ok(true) => {}
                Ok(false) => return Ok(None),
                // Truncated file: whatever was complete has been delivered.
                Err(e) if e.is_truncated() => {
                    self.state = State::Done;
                    return Ok(None);
                }
                Err(e) if e.class() == ErrorClass::Corrupt => {
                    // Damaged structure: restart one byte past where this element began
                    // so every error makes forward progress.
                    self.state = State::Resync {
                        from: element_start + 1,
                    };
                    return Err(e);
                }
                Err(e) => {
                    self.state = State::Done;
                    return Err(e);
                }
            }
        }
    }
}

fn eof_as(message: &str, e: Error) -> Error {
    if e.is_truncated() {
        Error::unsupported(message)
    } else {
        e
    }
}

fn read_ebml_header<R: Read + Seek>(reader: &mut Reader<R>, limits: &Limits) -> Result<()> {
    let h = reader
        .read_header()
        .map_err(|e| eof_as("not a Matroska file", e))?;
    if h.id != ids::EBML {
        return Err(Error::unsupported("not a Matroska file"));
    }
    let size = h
        .size
        .ok_or_else(|| Error::corrupt("unknown-size EBML header"))?;
    let data = reader.read_data(size, limits.max_element_size, "EBML header")?;
    let mut doc_type = String::from("matroska");
    for child in children(&data) {
        let (id, d) = child?;
        match id {
            ids::DOC_TYPE => doc_type = string(d),
            ids::EBML_MAX_ID_LENGTH if uint(d)? > 4 => {
                return Err(Error::unsupported("EBMLMaxIDLength > 4"));
            }
            ids::EBML_MAX_SIZE_LENGTH if uint(d)? > 8 => {
                return Err(Error::unsupported("EBMLMaxSizeLength > 8"));
            }
            _ => {}
        }
    }
    if doc_type != "matroska" && doc_type != "webm" {
        return Err(Error::unsupported(format!("EBML DocType {doc_type:?}")));
    }
    Ok(())
}

/// SeekHead entries as (element ID, absolute offset).
fn parse_seek_head(data: &[u8], segment_data_start: u64) -> Vec<(u32, u64)> {
    let mut out = Vec::new();
    for (id, seek) in children(data).flatten() {
        if id != ids::SEEK {
            continue;
        }
        let (mut target, mut position) = (None, None);
        for (id, d) in children(seek).flatten() {
            match id {
                ids::SEEK_ID if d.len() <= 4 => {
                    target = Some(d.iter().fold(0u32, |a, &b| (a << 8) | u32::from(b)))
                }
                ids::SEEK_POSITION => position = uint(d).ok(),
                _ => {}
            }
        }
        if let (Some(t), Some(p)) = (target, position) {
            out.push((t, segment_data_start.saturating_add(p)));
        }
    }
    out
}
