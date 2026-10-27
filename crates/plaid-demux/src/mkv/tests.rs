//! Synthetic Matroska files covering what FFmpeg's muxer never writes
//! (lacing, header stripping, zlib, BlockGroup references, late attachments,
//! unknown sizes). Run with `PLAID_WRITE_FIXTURES=<dir>` to also write them to
//! disk so `plaid-diff` can check them against ffprobe.

use std::io::Cursor;

use plaid_core::{Demuxer, ErrorClass, Packet, SideData, StreamInfo};

use super::{MkvDemuxer, ids};

// ---- EBML builder -------------------------------------------------------

fn id_bytes(id: u32) -> Vec<u8> {
    id.to_be_bytes()
        .into_iter()
        .skip_while(|&b| b == 0)
        .collect()
}

/// Element with an 8-byte size field (valid EBML, keeps offsets predictable).
fn el(id: u32, payload: &[u8]) -> Vec<u8> {
    let mut v = id_bytes(id);
    v.push(0x01);
    v.extend(&(payload.len() as u64).to_be_bytes()[1..]);
    v.extend(payload);
    v
}

fn unknown_size(id: u32, payload: &[u8]) -> Vec<u8> {
    let mut v = id_bytes(id);
    v.extend([0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
    v.extend(payload);
    v
}

fn u(id: u32, v: u64) -> Vec<u8> {
    el(id, &v.to_be_bytes())
}

fn s(id: u32, v: &str) -> Vec<u8> {
    el(id, v.as_bytes())
}

fn f(id: u32, v: f64) -> Vec<u8> {
    el(id, &v.to_be_bytes())
}

fn cat(parts: &[Vec<u8>]) -> Vec<u8> {
    parts.concat()
}

const TRACK_UID: u32 = 0x73C5;
const MUXING_APP: u32 = 0x4D80;
const WRITING_APP: u32 = 0x5741;
const CONTENT_ENCODING_ORDER: u32 = 0x5031;
const FILE_UID: u32 = 0x46AE;

fn ebml_header() -> Vec<u8> {
    el(
        ids::EBML,
        &cat(&[s(ids::DOC_TYPE, "matroska"), u(0x4287, 4), u(0x4285, 2)]),
    )
}

fn info() -> Vec<u8> {
    el(
        ids::INFO,
        &cat(&[
            u(ids::TIMESTAMP_SCALE, 1_000_000),
            f(ids::DURATION, 10_000.0),
            s(MUXING_APP, "plaid-test"),
            s(WRITING_APP, "plaid-test"),
        ]),
    )
}

/// 8 kHz mono 16-bit PCM: decodable by ffprobe whatever bytes we put in.
fn pcm_track(number: u64, extra: &[Vec<u8>]) -> Vec<u8> {
    let mut parts = vec![
        u(ids::TRACK_NUMBER, number),
        u(TRACK_UID, number),
        u(ids::TRACK_TYPE, 2),
        s(ids::CODEC_ID, "A_PCM/INT/LIT"),
        el(
            ids::AUDIO,
            &cat(&[
                f(ids::SAMPLING_FREQUENCY, 8000.0),
                u(ids::CHANNELS, 1),
                u(ids::BIT_DEPTH, 16),
            ]),
        ),
    ];
    parts.extend_from_slice(extra);
    el(ids::TRACK_ENTRY, &cat(&parts))
}

fn srt_track(number: u64) -> Vec<u8> {
    el(
        ids::TRACK_ENTRY,
        &cat(&[
            u(ids::TRACK_NUMBER, number),
            u(TRACK_UID, number),
            u(ids::TRACK_TYPE, 0x11),
            s(ids::CODEC_ID, "S_TEXT/UTF8"),
        ]),
    )
}

fn compression(algo: u64, settings: &[u8]) -> Vec<u8> {
    let mut comp = vec![u(ids::CONTENT_COMP_ALGO, algo)];
    if !settings.is_empty() {
        comp.push(el(ids::CONTENT_COMP_SETTINGS, settings));
    }
    el(
        ids::CONTENT_ENCODINGS,
        &el(
            ids::CONTENT_ENCODING,
            &cat(&[
                u(CONTENT_ENCODING_ORDER, 0),
                u(ids::CONTENT_ENCODING_SCOPE, 1),
                u(ids::CONTENT_ENCODING_TYPE, 0),
                el(ids::CONTENT_COMPRESSION, &cat(&comp)),
            ]),
        ),
    )
}

fn block_body(track: u8, rel: i16, flags: u8, payload: &[u8]) -> Vec<u8> {
    let mut v = vec![0x80 | track];
    v.extend(rel.to_be_bytes());
    v.push(flags);
    v.extend(payload);
    v
}

fn simple(track: u8, rel: i16, flags: u8, payload: &[u8]) -> Vec<u8> {
    el(ids::SIMPLE_BLOCK, &block_body(track, rel, flags, payload))
}

fn group(
    track: u8,
    rel: i16,
    payload: &[u8],
    duration: Option<u64>,
    reference: Option<i16>,
) -> Vec<u8> {
    let mut parts = vec![el(ids::BLOCK, &block_body(track, rel, 0, payload))];
    if let Some(d) = duration {
        parts.push(u(ids::BLOCK_DURATION, d));
    }
    if let Some(r) = reference {
        parts.push(el(ids::REFERENCE_BLOCK, &r.to_be_bytes()));
    }
    el(ids::BLOCK_GROUP, &cat(&parts))
}

fn cluster(timestamp: Option<u64>, blocks: &[Vec<u8>]) -> Vec<u8> {
    let mut parts: Vec<Vec<u8>> = timestamp
        .map(|t| u(ids::CLUSTER_TIMESTAMP, t))
        .into_iter()
        .collect();
    parts.extend_from_slice(blocks);
    el(ids::CLUSTER, &cat(&parts))
}

fn file(tracks: &[Vec<u8>], clusters: &[Vec<u8>]) -> Vec<u8> {
    let mut seg = vec![info(), el(ids::TRACKS, &cat(tracks))];
    seg.extend_from_slice(clusters);
    cat(&[ebml_header(), el(ids::SEGMENT, &cat(&seg))])
}

// Lace payload builders. Frames are distinct so data mix-ups show.

fn frame(len: usize, tag: u8) -> Vec<u8> {
    (0..len).map(|i| tag.wrapping_add(i as u8)).collect()
}

fn xiph(frames: &[Vec<u8>]) -> Vec<u8> {
    let mut v = vec![(frames.len() - 1) as u8];
    for f in &frames[..frames.len() - 1] {
        let mut n = f.len();
        while n >= 255 {
            v.push(255);
            n -= 255;
        }
        v.push(n as u8);
    }
    v.extend(frames.concat());
    v
}

fn ebml_lace(frames: &[Vec<u8>]) -> Vec<u8> {
    // Sizes kept < 63 so every vint is one byte.
    let mut v = vec![(frames.len() - 1) as u8, 0x80 | frames[0].len() as u8];
    for pair in frames[..frames.len() - 1].windows(2) {
        let delta = pair[1].len() as i16 - pair[0].len() as i16;
        v.push(0x80 | (delta + 63) as u8);
    }
    v.extend(frames.concat());
    v
}

fn fixed_lace(frames: &[Vec<u8>]) -> Vec<u8> {
    let mut v = vec![(frames.len() - 1) as u8];
    v.extend(frames.concat());
    v
}

// ---- Fixtures -----------------------------------------------------------

const KEY: u8 = 0x80;
const XIPH: u8 = 0x02;
const FIXED: u8 = 0x04;
const EBML_LACING: u8 = 0x06;

fn lacing_fixture() -> Vec<u8> {
    let a = [frame(4, 1), frame(300, 2), frame(2, 3)];
    let b = [frame(8, 4), frame(6, 5), frame(4, 6)];
    let c = [frame(4, 7), frame(4, 8)];
    let d = [frame(6, 9), frame(2, 10)];
    file(
        &[
            pcm_track(1, &[u(ids::DEFAULT_DURATION, 10_000_000)]),
            pcm_track(2, &[]),
        ],
        &[cluster(
            Some(0),
            &[
                simple(1, 0, KEY | XIPH, &xiph(&a)),
                simple(1, 30, KEY | EBML_LACING, &ebml_lace(&b)),
                simple(1, 60, KEY | FIXED, &fixed_lace(&c)),
                simple(2, 0, KEY | XIPH, &xiph(&d)),
            ],
        )],
    )
}

fn block_group_fixture() -> Vec<u8> {
    file(
        &[
            srt_track(1),
            pcm_track(2, &[u(ids::DEFAULT_DURATION, 20_000_000)]),
        ],
        &[
            cluster(
                Some(1000),
                &[
                    group(1, 0, b"Hello", Some(1500), None),
                    group(2, 0, &frame(4, 1), None, Some(-20)),
                    group(2, 20, &frame(4, 2), Some(0), None),
                    simple(1, 2000, KEY, b"World"),
                ],
            ),
            cluster(None, &[simple(2, 5, KEY, &frame(4, 3))]),
            cluster(Some(5000), &[simple(2, -10, KEY, &frame(4, 4))]),
        ],
    )
}

fn encodings_fixture() -> Vec<u8> {
    let zlib = miniz_oxide::deflate::compress_to_vec_zlib(&frame(64, 9), 6);
    file(
        &[
            pcm_track(1, &[compression(3, &[0xAA, 0xBB])]),
            pcm_track(2, &[compression(0, &[])]),
            pcm_track(3, &[u(ids::CODEC_DELAY, 6_500_000)]),
        ],
        &[cluster(
            Some(100),
            &[
                simple(1, 0, KEY, &frame(2, 1)),
                simple(1, 10, KEY | XIPH, &xiph(&[frame(2, 2), frame(4, 3)])),
                simple(2, 0, KEY, &zlib),
                simple(3, 0, KEY, &frame(4, 4)),
            ],
        )],
    )
}

fn unknown_sizes_fixture() -> Vec<u8> {
    let cluster_unknown = |t: u64, rel: i16| {
        unknown_size(
            ids::CLUSTER,
            &cat(&[
                u(ids::CLUSTER_TIMESTAMP, t),
                simple(1, rel, KEY, &frame(4, t as u8)),
            ]),
        )
    };
    let seg = cat(&[
        info(),
        el(ids::TRACKS, &pcm_track(1, &[])),
        cluster_unknown(0, 0),
        cluster_unknown(100, 5),
        cluster_unknown(200, 0),
        el(ids::TAGS, &[]),
    ]);
    cat(&[ebml_header(), unknown_size(ids::SEGMENT, &seg)])
}

/// Attachments stored after the clusters, found through the SeekHead.
fn late_attachments_fixture() -> Vec<u8> {
    let build = |attachments_pos: u64| {
        let seek = el(
            ids::SEEK,
            &cat(&[
                el(ids::SEEK_ID, &id_bytes(ids::ATTACHMENTS)),
                u(ids::SEEK_POSITION, attachments_pos),
            ]),
        );
        el(ids::SEEK_HEAD, &seek)
    };
    let body = cat(&[
        info(),
        el(ids::TRACKS, &pcm_track(1, &[])),
        cluster(Some(0), &[simple(1, 0, KEY, &frame(4, 1))]),
    ]);
    let attachments = el(
        ids::ATTACHMENTS,
        &el(
            ids::ATTACHED_FILE,
            &cat(&[
                s(ids::FILE_NAME, "notes.txt"),
                s(ids::FILE_MIME_TYPE, "text/plain"),
                el(ids::FILE_DATA, b"plaid attachment"),
                u(FILE_UID, 42),
            ]),
        ),
    );
    let seek_head = build((build(0).len() + body.len()) as u64);
    cat(&[
        ebml_header(),
        el(ids::SEGMENT, &cat(&[seek_head, body, attachments])),
    ])
}

/// A block with impossible lacing between two good ones.
fn corrupt_block_fixture() -> Vec<u8> {
    let mut bad = vec![1u8, 200]; // Xiph: 2 frames, first claims 200 bytes.
    bad.extend(frame(4, 0));
    file(
        &[pcm_track(1, &[])],
        &[cluster(
            Some(0),
            &[
                simple(1, 0, KEY, &frame(4, 1)),
                simple(1, 10, KEY | XIPH, &bad),
                simple(1, 20, KEY, &frame(4, 2)),
            ],
        )],
    )
}

fn fixtures() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("lacing.mkv", lacing_fixture()),
        ("block_groups.mkv", block_group_fixture()),
        ("encodings.mkv", encodings_fixture()),
        ("unknown_sizes.mkv", unknown_sizes_fixture()),
        ("late_attachments.mkv", late_attachments_fixture()),
        ("corrupt_block.mkv", corrupt_block_fixture()),
    ]
}

// ---- Helpers ------------------------------------------------------------

struct Demuxed {
    streams: Vec<StreamInfo>,
    packets: Vec<Packet>,
    errors: usize,
}

fn demux(bytes: Vec<u8>) -> Demuxed {
    let mut d = MkvDemuxer::open(Cursor::new(bytes)).expect("open");
    let mut packets = Vec::new();
    let mut errors = 0;
    loop {
        match d.read_packet() {
            Ok(Some(p)) => packets.push(p),
            Ok(None) => break,
            Err(e) => {
                assert_eq!(e.class(), ErrorClass::Corrupt, "{e}");
                errors += 1;
            }
        }
    }
    Demuxed {
        streams: d.streams().to_vec(),
        packets,
        errors,
    }
}

fn timing(packets: &[Packet], stream: usize) -> Vec<(Option<i64>, Option<i64>, usize)> {
    packets
        .iter()
        .filter(|p| p.stream_index == stream)
        .map(|p| (p.pts, p.duration, p.data.len()))
        .collect()
}

// ---- Tests --------------------------------------------------------------

#[test]
fn lacing_splits_frames_and_spreads_duration() {
    let d = demux(lacing_fixture());
    assert_eq!(
        timing(&d.packets, 0),
        vec![
            (Some(0), Some(10), 4),
            (Some(10), Some(10), 300),
            (Some(20), Some(10), 2),
            (Some(30), Some(10), 8),
            (Some(40), Some(10), 6),
            (Some(50), Some(10), 4),
            (Some(60), Some(10), 4),
            (Some(70), Some(10), 4),
        ]
    );
    assert_eq!(d.packets[1].data, frame(300, 2));
    // No DefaultDuration: later laces have no timestamp.
    assert_eq!(
        timing(&d.packets, 1),
        vec![(Some(0), None, 6), (None, None, 2)]
    );
}

#[test]
fn block_groups_and_cluster_timestamps() {
    let d = demux(block_group_fixture());
    let p = &d.packets;
    assert_eq!(
        (p[0].pts, p[0].duration, p[0].keyframe),
        (Some(1000), Some(1500), true)
    );
    assert_eq!(p[0].data, b"Hello");
    assert!(!p[1].keyframe, "ReferenceBlock means not a keyframe");
    assert_eq!(
        p[1].duration,
        Some(20),
        "DefaultDuration applies to BlockGroups"
    );
    assert_eq!(
        p[2].duration,
        Some(20),
        "BlockDuration 0 falls back to DefaultDuration"
    );
    assert_eq!((p[3].pts, p[3].duration), (Some(3000), None));
    assert_eq!(p[4].pts, Some(5), "cluster without Timestamp counts from 0");
    assert_eq!(p[5].pts, Some(4990), "negative relative timestamp");
}

#[test]
fn content_encodings_and_codec_delay() {
    let d = demux(encodings_fixture());
    let p = &d.packets;
    assert_eq!(p[0].data, [&[0xAA, 0xBB][..], &frame(2, 1)].concat());
    assert_eq!(
        p[1].data,
        [&[0xAA, 0xBB][..], &frame(2, 2)].concat(),
        "stripping applies per lace"
    );
    assert_eq!(p[2].data, [&[0xAA, 0xBB][..], &frame(4, 3)].concat());
    assert_eq!(p[3].data, frame(64, 9), "zlib");
    assert_eq!(p[4].pts, Some(100 - 7), "6.5 ms codec delay rounds to 7");
}

#[test]
fn unknown_size_segment_and_clusters() {
    let d = demux(unknown_sizes_fixture());
    assert_eq!(
        d.packets.iter().map(|p| p.pts).collect::<Vec<_>>(),
        vec![Some(0), Some(105), Some(200)]
    );
    assert_eq!(d.errors, 0);
}

#[test]
fn attachments_after_clusters_via_seek_head() {
    let d = demux(late_attachments_fixture());
    assert_eq!(d.streams.len(), 2);
    let att = &d.streams[1];
    assert_eq!(att.kind.name(), "attachment");
    assert_eq!(
        att.codec.name(),
        "none",
        "FFmpeg maps no codec to text/plain"
    );
    assert_eq!(att.tag("filename"), Some("notes.txt"));
    assert_eq!(att.extradata, b"plaid attachment");
    assert_eq!(d.packets.len(), 1);
}

#[test]
fn corrupt_block_is_dropped_alone() {
    let d = demux(corrupt_block_fixture());
    assert_eq!(d.errors, 1);
    assert_eq!(
        d.packets.iter().map(|p| p.pts).collect::<Vec<_>>(),
        vec![Some(0), Some(20)]
    );
}

#[test]
fn webm_webvtt_side_data() {
    let track = el(
        ids::TRACK_ENTRY,
        &cat(&[
            u(ids::TRACK_NUMBER, 1),
            u(TRACK_UID, 1),
            u(ids::TRACK_TYPE, 0x11),
            s(ids::CODEC_ID, "D_WEBVTT/SUBTITLES"),
        ]),
    );
    let d = demux(file(
        &[track],
        &[cluster(
            Some(0),
            &[simple(1, 0, KEY, b"c1\nalign:start\nHi\n")],
        )],
    ));
    assert_eq!(d.packets[0].data, b"Hi");
    assert_eq!(
        d.packets[0].side_data,
        vec![
            SideData::WebVttIdentifier(b"c1".to_vec()),
            SideData::WebVttSettings(b"align:start".to_vec())
        ]
    );
}

#[test]
fn rejects_non_matroska() {
    assert!(MkvDemuxer::open(Cursor::new(b"RIFF....AVI ".to_vec())).is_err());
    assert!(MkvDemuxer::open(Cursor::new(Vec::new())).is_err());
    let webm_less = cat(&[
        el(ids::EBML, &s(ids::DOC_TYPE, "mp4")),
        el(ids::SEGMENT, &[]),
    ]);
    assert_eq!(
        MkvDemuxer::open(Cursor::new(webm_less))
            .err()
            .map(|e| e.class()),
        Some(ErrorClass::Unsupported)
    );
}

/// Damaged inputs must never panic or loop: every truncation point, plus
/// pseudo-random byte flips, of every fixture.
#[test]
fn survives_truncation_and_mutation() {
    let mut rng = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let run = |bytes: Vec<u8>| {
        let Ok(mut d) = MkvDemuxer::open(Cursor::new(bytes)) else {
            return;
        };
        for _ in 0..100_000 {
            match d.read_packet() {
                Ok(None) => return,
                Ok(Some(_)) => {}
                Err(e) if e.class() == ErrorClass::Corrupt || e.class() == ErrorClass::Resource => {
                }
                Err(_) => return,
            }
        }
        panic!("demuxer did not terminate");
    };

    for (_, bytes) in fixtures() {
        for len in 0..bytes.len() {
            run(bytes[..len].to_vec());
        }
        for _ in 0..2_000 {
            let mut m = bytes.clone();
            for _ in 0..1 + next() % 4 {
                let i = (next() % m.len() as u64) as usize;
                m[i] = next() as u8;
            }
            run(m);
        }
    }
}

#[test]
fn write_fixtures_for_ffprobe() {
    let Ok(dir) = std::env::var("PLAID_WRITE_FIXTURES") else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    for (name, bytes) in fixtures() {
        std::fs::write(std::path::Path::new(&dir).join(name), bytes).unwrap();
    }
}
