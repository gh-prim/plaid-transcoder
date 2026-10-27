# plaid-transcoder — Scoped FFmpeg replacement for a media server

## Context
Goal: replace FFmpeg inside a media server (Jellyfin/Plex-style: library scan, direct play, remux, transcode to HLS) with a purpose-built engine covering only the useful subset. Motivations: memory-safety of untrusted-file parsing, a cleaner GPU zero-copy pipeline, a controlled licence surface, and a codebase small enough to own. The repo is empty; this document is the pre-code design. It is a **strangler migration**: FFmpeg stays as a sandboxed fallback until each brick is proven, so we never ship a regression in coverage.

> **Note on "real usage data":** I have no telemetry from your server. The scope below is based on public signals: device decode support matrices, Jellyfin/Plex playback-method distributions discussed publicly, and the formats that dominate typical home libraries. **Milestone 0 replaces these assumptions with your numbers.** Every scope decision below is phrased as a hypothesis that M0 confirms or kills.

---

## 1. Scope (subset to cover)

### Usage model driving the scope
A media server does four jobs, in decreasing volume:
1. **Probe** every file at scan time (100% of files, runs constantly, most exposed to hostile input).
2. **Direct play** (no engine involvement beyond probe).
3. **Remux / direct stream**: container change (MKV → fMP4/HLS), video copied, often audio transcoded. Very common because browsers and Apple devices do not play MKV.
4. **Full transcode**: codec unsupported by the client, bitrate cap, HDR→SDR tonemap, or subtitle burn-in. Least frequent but the most CPU/GPU-expensive.

So the value is concentrated in: probe + demux + fMP4/HLS mux (jobs 1 and 3), then the HW transcode path for H.264/HEVC (job 4).

### Inputs (demux + decode)
| Area | In scope (native) | Long tail → FFmpeg fallback |
|---|---|---|
| Containers | Matroska/WebM, MP4/MOV, MPEG-TS/M2TS | AVI, FLV, OGG, WMV/ASF, VOB/ISO, RealMedia |
| Video | H.264 (8-bit), HEVC (8/10-bit, HDR10, HLG, Dolby Vision P5/P7/P8 metadata), AV1, VP9 | MPEG-2, VC-1, MPEG-4 Part 2/DivX, H.264 10-bit/4:2:2 |
| Audio | AAC, AC-3, E-AC-3, FLAC, Opus, MP3, Vorbis, PCM, DTS core, TrueHD (decode for downmix) | DTS-HD MA extensions, WMA, ALAC (cheap to add later) |
| Subtitles | SRT, WebVTT, ASS/SSA, PGS, VobSub | DVB-sub, CC608/708 extraction (phase 2) |

### Outputs
- **HLS** (RFC 8216) with **fMP4/CMAF segments** as the primary output, **MPEG-TS segments** for legacy clients (old Roku/Tizen/webOS firmware).
- Video: **H.264** (universal baseline), **HEVC** (Apple, modern TVs, ~40–50% bitrate savings), **AV1** (optional, recent clients only).
- Audio: **AAC-LC** stereo/5.1 encode; AC-3/E-AC-3 and FLAC/Opus **copy** in fMP4 when the client accepts them.
- Subtitles: WebVTT segmented sidecars; burn-in for PGS/VobSub/complex ASS.
- Filters: scale, deinterlace (yadif-class, GPU), **HDR→SDR tonemap** (BT.2390/Hable/Mobius), audio downmix + loudness, subtitle overlay. Nothing else.

### Hardware acceleration
| Tier | API | Why |
|---|---|---|
| 1 | NVIDIA NVDEC/NVENC | Most common dedicated transcode GPU in self-hosted servers |
| 1 | Intel QSV via **VA-API** (Linux) / oneVPL | Intel iGPUs (N100, Core) are the most common home-server hardware |
| 1 | Apple VideoToolbox | macOS servers, also your dev platform |
| 2 | AMD via VA-API (Linux) / AMF (Windows) | Smaller share, quality historically weaker |
| 3 | V4L2-M2M / Rockchip MPP | ARM boxes (RPi, RK3588); add if M0 shows demand |
| Watch | **Vulkan Video** | Could unify tiers 1–2 behind one API long-term; not yet mature for encode everywhere |

**Explicitly out of scope:** general filter graph, arbitrary format conversion, capture devices, network protocols beyond local file/HTTP range reads, encoders other than the three video + AAC, DRM.

---

## 2. Rewrite vs reuse per brick

| Brick | Decision | Reason |
|---|---|---|
| **Probe / demuxers** (MKV, MP4, TS) | **Rewrite** (Rust, `#![forbid(unsafe_code)]`) | #1 attack surface (runs on every untrusted file at scan time); moderately sized specs; full control over timestamp/edit-list behaviour that causes most HLS bugs |
| **Bitstream parsers** (SPS/PPS/VPS, OBU headers, HDR/DV SEI, ADTS, AC-3 sync) | **Rewrite** | Small, needed for codec strings (`avc1.640028`, `hvc1…`), keyframe detection, HDR signalling; pure safe Rust |
| **Muxers** (fMP4/CMAF, MPEG-TS, HLS playlists, WebVTT segmenter) | **Rewrite** | Where client compatibility lives; we must own every byte; small code |
| **Pipeline / scheduler / session manager** | **Rewrite** | This is the product: seeking, segment-on-demand, throttling, HW session limits |
| **Video decode (HW)** | **Reuse vendor SDKs** via thin Rust FFI | Zero-copy requires talking to the APIs directly; no reason to reimplement |
| **Video decode (SW) AV1** | **Reuse dav1d** (BSD-2), evaluate **rav1d** (Rust port) | Best-in-class, permissive; rav1d gives memory safety once perf parity is confirmed |
| **Video decode (SW) H.264/HEVC/VP9** | **Reuse libavcodec decoders only**, in a sandboxed worker (LGPL build, dynamic link) | Writing a conformant, fast H.264/HEVC decoder is years of work for zero product value; sandbox mitigates the C risk |
| **Video encode (HW)** | **Reuse vendor SDKs** | Same as decode |
| **Video encode (SW) H.264** | **Reuse x264** if GPL is acceptable, else **OpenH264** (BSD + Cisco patent coverage on their binary) | Licence decision, see open questions |
| **Video encode (SW) HEVC/AV1** | x265 (GPL) / **SVT-AV1** (BSD) | SW HEVC is rarely worth it on a server; SVT-AV1 is the pragmatic AV1 encoder |
| **Audio decode** | **Reuse Symphonia** (MPL-2.0) for AAC/FLAC/MP3/Vorbis/PCM; libavcodec (sandboxed) for AC-3/E-AC-3/DTS/TrueHD | Symphonia is pure Rust and conformant for the common formats; Dolby/DTS decoders do not exist in Rust at quality |
| **Audio encode** | AAC: **fdk-aac** (licence non-GPL-compatible but fine standalone) or FFmpeg native AAC; Opus: libopus | AAC quality matters at low bitrates; no Rust AAC encoder is competitive |
| **Resampling / downmix / loudness** | **Rewrite** (or `rubato` crate for resampling) | Simple DSP, safe Rust |
| **Scale / tonemap / deinterlace** | **Rewrite as GPU compute shaders** (Vulkan/Metal, CUDA where needed); vendor VPP where faster; **libplacebo** (LGPL) as reference/fallback | Must stay on GPU for zero-copy; tonemap quality is a user-visible differentiator |
| **ASS rendering** | **Reuse libass** (ISC) in sandbox | Complex typography, de-facto spec is the implementation |
| **PGS/VobSub decode** | **Rewrite** | Simple RLE bitmap formats |

Licence summary: core engine **MIT/Apache-2.0**; reused components are BSD/ISC/MPL/LGPL (dynamic). GPL (x264/x265) only as an optional, separately distributed plugin. Patents (H.264/HEVC/AAC/AC-3 pools) are not solved by any licence: rely on hardware codecs (the GPU vendor carries the licence) where possible and get legal advice for software encoders.

---

## 3. Sources to build from

### Official specifications
- Containers: **ISO/IEC 14496-12** (ISOBMFF), **14496-14** (MP4), **14496-15** (AVC/HEVC carriage), **23000-19** (CMAF); **RFC 9559** (Matroska) + **RFC 8794** (EBML); **ISO/IEC 13818-1** (MPEG-TS); **AV1 Codec ISO Media File Format Binding** (AOM); VP9/Opus/FLAC-in-ISOBMFF specs.
- Streaming: **RFC 8216** + **draft-pantos-hls-rfc8216bis**; **Apple HLS Authoring Specification for Apple Devices**; ETSI TS 102 366 annex for (E-)AC-3 in MP4; W3C **WebVTT**.
- Video: **ITU-T H.264**, **ITU-T H.265**, **AV1 Bitstream & Decoding Process Spec**, **VP9 Bitstream Spec**; HDR: **SMPTE ST 2084**, **ST 2086**, **ITU-R BT.2100**, **BT.2390** (tonemapping), CTA-861.3; **Dolby Vision** profile/level and ISOBMFF/Matroska docs.
- Audio: **ISO/IEC 14496-3** (AAC), **ETSI TS 102 366** (AC-3/E-AC-3), **RFC 6716** (Opus), **RFC 9639** (FLAC), ETSI TS 102 114 (DTS), ITU-R BS.1770 (loudness).

### Hardware SDKs
- **NVIDIA Video Codec SDK** (NVENC/NVDEC) + CUDA driver API.
- **libva / VA-API** + Intel media-driver; **Intel oneVPL** (QSV).
- **AMD AMF** (Windows), Mesa VA-API (Linux).
- **Apple VideoToolbox** + CoreVideo/IOSurface + Metal.
- **Vulkan Video** (`VK_KHR_video_decode_h264/h265/av1`, `VK_KHR_video_encode_*`).
- **V4L2 Stateful/Stateless codec API**, Rockchip MPP (tier 3).

### Existing (re)writes to study
| Project | What to learn |
|---|---|
| **dav1d** / **rav1d** | SIMD-heavy decoder architecture; rav1d = cost/perf of a C→Rust port |
| **rav1e** | Rust encoder structure, threading, testing approach |
| **Symphonia** | Pure-Rust demux/decode API design, probe registry, conformance testing |
| **gst-plugins-rs** (`fmp4mux`, `hlssink3`, `mp4mux`) | Production Rust fMP4/HLS muxing; edge cases already solved |
| **GStreamer core / gstreamer-rs** | Buffer/memory model (DMA-BUF, GL/CUDA memory), caps negotiation; also what to avoid (graph complexity) |
| **Mozilla `mp4parse-rust`** | Safe MP4 parser shipped in Firefox, fuzz-hardened |
| **ChromeOS `cros-codecs`** | Rust VA-API/V4L2 stateless decoding |
| **Software Mansion `vk-video`** | Rust Vulkan Video decode/encode |
| **jellyfin-ffmpeg** patches + Jellyfin transcode code | Exactly which FFmpeg features a media server needs; HW tonemap filters; device quirks |
| **Shaka Packager / Bento4** | Reference fMP4/HLS packaging, compliance details |
| **libplacebo** | Reference tonemapping quality and GPU colour pipeline |

---

## 4. Architecture

### Language
**Rust** for everything we write. C/C++ dependencies only behind FFI in sandboxed worker processes (except HW SDKs, which must live in the GPU process). `unsafe` confined to FFI crates, audited, and `forbid(unsafe_code)` in parser/muxer crates.

### Crate layout
```
plaid-core      types: Packet, Frame, Timestamp(Rational), StreamInfo, Error
plaid-demux     mkv, mp4, ts   (safe, no I/O assumptions; pull from a ByteSource)
plaid-bitstream h264/hevc/av1/vp9/aac/ac3 header parsers, codec strings, HDR/DV metadata
plaid-mux       fmp4/cmaf, ts, hls playlists, webvtt
plaid-hw        backends: nvidia, vaapi, videotoolbox, (amf, vulkan) behind one trait
plaid-sw        dav1d, libavcodec-sandboxed, symphonia, fdk-aac, x264/openh264 adapters
plaid-filter    gpu scale/tonemap/deinterlace/overlay, audio dsp
plaid-pipeline  graph builder (fixed topologies), scheduler, session/seek logic
plaid-sandbox   process spawn, seccomp/landlock/seatbelt, shared-memory IPC
plaid-cli       ffmpeg-like CLI for tests + drop-in shim for the media server
plaid-server-api library API the media server calls (probe, plan, start/seek/stop session)
```

### Pipeline model
- **Fixed topologies, not a generic filter graph**: `Probe`, `Remux`, `Remux+AudioTranscode`, `FullTranscode{decode → [deint] → [tonemap] → scale → [overlay] → encode}`. A **planner** chooses the topology from (source info × client capability profile × server HW), which also replaces the media server's ffmpeg command-line building.
- **Threading**: one OS thread per stage with **bounded channels** (backpressure). Codec libraries own their internal worker pools (dav1d, x264). **Tokio only at the edges** (HTTP serving, segment requests, file I/O), never in the media hot path.
- **Segment-on-demand**: transcoder runs ahead of the player by N segments then pauses (throttling); seek = restart at nearest keyframe with stable segment numbering so playlists stay valid.

### Zero-copy GPU path
- Frames are an enum: `Frame::Cpu(planes)` | `Frame::Gpu(GpuHandle)` where `GpuHandle` = `CUdeviceptr`/CUDA array, `VASurfaceID`/DMA-BUF fd, `CVPixelBuffer`/IOSurface, `VkImage`.
- Decode → filter → encode stays on one device; filters operate on native surfaces (CUDA kernels / VA VPP / Metal / Vulkan compute, interop via DMA-BUF or IOSurface).
- **Explicit transfer nodes** only when unavoidable (SW decode → HW encode upload; subtitle bitmap upload). The planner minimises them and logs each one (metric: bytes copied per frame).
- Per-device session limits and VRAM budget enforced by a `HwArbiter` (NVENC consumer session caps, iGPU memory).

### Error handling
- `thiserror` error types with a **recoverability class**: `Corrupt` (skip packet, conceal, continue), `StreamFatal` (drop that stream, keep others), `SessionFatal` (stop, fallback), `Resource` (HW full → retry on another backend or SW).
- **Fallback ladder** per session: native HW → native SW → **FFmpeg fallback process**. Every fallback is logged with the reason, feeding the coverage backlog.
- No panics across FFI; panics in workers are caught at the process boundary and treated as `SessionFatal`.

### Sandboxing of parsing
- **Probe and demux run in a sandboxed worker process** even though they're safe Rust (defence in depth, resource limits). C decoders (libavcodec, libass, dav1d) **always** in a sandbox.
- Linux: seccomp-bpf allowlist + Landlock (read-only on the single input fd) + namespaces, rlimits (memory, CPU time). macOS: `sandbox_init` profile. Windows: AppContainer / job object.
- IPC: shared-memory ring buffers for packets/frames, a small control channel (postcard/bincode). Input passed as a pre-opened fd; the worker has no filesystem or network access.
- Hard limits in parsers: max box/element size, max track count, max nesting depth, max allocation per file.
- GPU process cannot be fully sandboxed (driver access); it only ever receives **already-validated packets** from the demux worker.

---

## 5. Validation strategy

| Layer | Method | Pass criterion |
|---|---|---|
| Demux | **Differential vs `ffprobe -show_packets -show_streams`** on a large corpus (FFmpeg FATE samples, Matroska test files, your library) | Identical packet payload hashes, pts/dts/duration, keyframe flags, stream metadata (documented, reviewed exceptions only) |
| Bitstream parsers | ITU/JCT-VC/JVT conformance bitstreams, **Argon** AV1 suite, libaom test vectors | Every header field matches the reference decoder's trace |
| Decode | `ffmpeg -f framemd5` vs our decode | **Bit-exact** for SW decoders; HW decode: per-frame PSNR ≥ 50 dB vs reference (HW is not always bit-exact) |
| Mux / HLS | **Apple `mediastreamvalidator` + `hlsreport`**, `mp4box -info`/Bento4 checks, CMAF conformance, round-trip demux of our own output | Zero must-fix issues in Apple's validator |
| Playback | **Client matrix** (automated where possible): Safari macOS/iOS/tvOS (AVPlayer), Chrome/Edge/Firefox via **hls.js** and **Shaka**, Android **ExoPlayer/Media3**, Chromecast, Roku, Samsung Tizen, LG webOS, Fire TV, Xbox, Jellyfin/your native apps | Start, seek, audio switch, subtitle switch, 2h continuous play, no A/V drift > 40 ms |
| Fuzzing | **cargo-fuzz (libFuzzer) + AFL++** per demuxer/parser, structure-aware generators for EBML/ISOBMFF/TS, continuous run in CI, apply to **OSS-Fuzz** once public | No crash/hang/OOM; per-input time and memory caps enforced |
| Quality | **VMAF** (libvmaf, NEG model too), PSNR, SSIM; **BD-rate** vs FFmpeg with identical encoder settings | VMAF within ±0.5 of FFmpeg at equal bitrate for the same encoder; tonemap judged by VMAF on SDR reference + visual review set |
| Performance | fps per stream, **concurrent streams per GPU**, time-to-first-segment, seek-to-play latency, CPU %, RSS, VRAM, bytes copied per frame | ≥ FFmpeg throughput on every HW tier; TTFS ≤ FFmpeg; lower CPU in HW paths (no hidden copies) |
| Regression | Nightly differential run on the full corpus + perf dashboard; **shadow mode** in production (run plaid alongside FFmpeg on probe, compare results, serve FFmpeg's) | No new diffs without an accepted explanation |

---

## 6. Milestones (most profitable first)

**M0 — Measure (2 wks).** Instrument the media server: log probe results (container/codec/profile/HDR) across all libraries, playback decision per session (direct play / remux / transcode + reason), client device profiles, server HW. Build the test corpus from it. **Output:** a data-backed scope table replacing section 1's hypotheses, and the priority ordering for M1–M5.

**M1 — Safe probe (4–6 wks). Highest ROI.** `plaid-demux` + `plaid-bitstream` for MKV/MP4/TS, sandboxed probe worker, JSON output compatible with what the server consumes from ffprobe today. Differential + fuzzing harness from day one. *Why first:* runs on 100% of files, largest security exposure, no codec work, easiest to validate bit-exact, and it builds the corpus/differential infrastructure every later milestone reuses. Ship in **shadow mode**, then switch.

**M2 — Remux to HLS (6–8 wks).** fMP4/CMAF + TS muxers, HLS playlists, segment-on-demand, seeking, WebVTT subtitles, video+audio **copy** only; audio transcode delegated to an FFmpeg sidecar for now. *Why:* covers the large remux share entirely in safe Rust, near-zero CPU, and is where most client compatibility bugs live. Gate: Apple validator clean + client matrix green.

**M3 — Audio path (3–4 wks).** Symphonia decode + sandboxed libavcodec for Dolby/DTS, downmix/loudness, AAC encode. Remux sessions no longer need FFmpeg at all.

**M4 — HW transcode, tier 1 (8–12 wks).** NVDEC/NVENC, VA-API/QSV, VideoToolbox; H.264/HEVC decode→scale→encode fully zero-copy; HwArbiter; fallback ladder to FFmpeg. Gate: throughput ≥ FFmpeg, VMAF parity.

**M5 — HDR & subtitles burn-in (6–8 wks).** GPU tonemapping (BT.2390 + Hable), Dolby Vision P5/P8 handling, PGS/VobSub decode, libass overlay, GPU deinterlace.

**M6 — SW fallbacks & AV1 (4–6 wks).** dav1d/rav1d, sandboxed libavcodec SW decode, x264/OpenH264 & SVT-AV1 SW encode, AV1 output for capable clients. FFmpeg now only handles the long-tail formats.

**M7 — Long tail & tier 2/3 HW (ongoing, data-driven).** Add formats only when M0/fallback logs show demand (AVI, MPEG-2, VC-1, AMD AMF, V4L2/RKMPP, Vulkan Video). Remove FFmpeg when fallback rate < agreed threshold (e.g. 0.1% of sessions).

## Open decisions (need your input before M4/M6)
1. **GPL acceptable?** Decides x264/x265 vs OpenH264-only for SW encode.
2. **Target platforms**: Linux-only first, or macOS/Windows server too? Affects sandbox and HW tier work.
3. **Which media server** (Jellyfin fork, own server?): determines the API shape (`plaid-server-api` vs FFmpeg-compatible CLI shim).

## Verification of this plan
This is a design document; no code is produced yet. It is validated by M0's data: each scope row in section 1 must be confirmed (or dropped) by measured share of files/sessions before M1 starts.
