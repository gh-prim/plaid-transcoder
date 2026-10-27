# plaid-transcoder

A scoped, memory-safe replacement for FFmpeg inside a media server. Design, scope and milestones: [docs/DESIGN.md](docs/DESIGN.md).

Current state: **M1 in progress**: Matroska/WebM probe and demux, validated packet-for-packet against ffprobe.

## Layout

| Crate | Purpose |
|---|---|
| `plaid-core` | Packets, streams, time bases, error classes, parsing limits, `Demuxer` trait |
| `plaid-demux` | Demuxers (`#![forbid(unsafe_code)]`). Matroska/WebM today |
| `plaid-cli` | `plaid probe <file>` (JSON) and `plaid packets <file>` |
| `plaid-diff` | Differential harness against ffprobe |
| `fuzz/` | cargo-fuzz target (`mkv_demux`), outside the workspace |

## Testing

```bash
cargo test --workspace
```

Differential run against ffprobe (needs FFmpeg on `PATH`):

```bash
scripts/gen-corpus.sh
PLAID_WRITE_FIXTURES=$PWD/corpus/synthetic cargo test -p plaid-demux write_fixtures
cargo run --release -p plaid-diff -- corpus/generated corpus/synthetic /path/to/your/library
```

`plaid-diff` runs ffprobe with `-fflags +nofillin+noparse` so it sees raw demuxer output. For every stream it compares type, codec, time base, dimensions, sample rate, channels, extradata hash, disposition and tags. For every packet it compares stream, pts, duration, size, pos, key flag and MD5. Expected differences are printed as notes and don't fail the file:

- **Extradata ffprobe derived itself:** ffprobe builds it by decoding frames in `find_stream_info`, and plaid doesn't decode.
- **Damaged block:** plaid drops the block and keeps reading. FFmpeg stops reading the file at that point.

Fuzzing (nightly + `cargo install cargo-fuzz`):

```bash
cd fuzz && cargo +nightly fuzz run mkv_demux corpus/ ../corpus/generated ../corpus/synthetic
```

## Not yet handled (M1 backlog)

- MP4/MOV and MPEG-TS demuxers
- Sandboxed probe worker process
- Seeking (Cues), chapters, Tags element (per-track `DURATION` etc.)
- Block additions (HDR10+ / Dolby Vision RPU), legacy `A_AAC/*` IDs without CodecPrivate, `V_MS/VFW`/`A_MS/ACM` wrappers
