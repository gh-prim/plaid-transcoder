#!/usr/bin/env bash
# Generates a Matroska test corpus with FFmpeg for the differential harness.
# usage: scripts/gen-corpus.sh [OUT_DIR]   (default: corpus/generated)
#
# Files mkvmerge would produce (lacing, header stripping) are covered by the
# synthetic fixtures written by `PLAID_WRITE_FIXTURES=corpus/synthetic cargo test`.
set -euo pipefail

OUT=${1:-corpus/generated}
mkdir -p "$OUT"
D=3

ff() { ffmpeg -nostdin -v error -y "$@"; }
video() { echo -f lavfi -i "testsrc2=size=${1:-320x240}:rate=${2:-25}"; }
tone() { echo -f lavfi -i "sine=frequency=${1:-440}:sample_rate=${2:-48000}"; }
surround() { echo -f lavfi -i "sine=frequency=330:sample_rate=48000,aformat=channel_layouts=5.1"; }

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

cat >"$TMP/subs.srt" <<'EOF'
1
00:00:00,500 --> 00:00:01,500
Hello <i>world</i>

2
00:00:02,000 --> 00:00:02,800
Second line
EOF
ff -i "$TMP/subs.srt" "$TMP/subs.ass"
ff -i "$TMP/subs.srt" "$TMP/subs.vtt"
ff -f lavfi -i "testsrc2=size=200x200" -frames:v 1 "$TMP/cover.png"

echo "generating into $OUT"

# Mainstream combinations
ff $(video) $(tone) -t $D -c:v libx264 -bf 2 -g 25 -c:a aac "$OUT/h264_aac.mkv"
ff $(video 640x360) $(tone 440 48000) -t $D -c:v libx265 -pix_fmt yuv420p10le -x265-params log-level=none -c:a libopus "$OUT/hevc10_opus.mkv"
ff $(video) $(tone 440 44100) -t $D -c:v libvpx-vp9 -deadline realtime -c:a vorbis -strict -2 -ac 2 "$OUT/vp9_vorbis.webm"
ff $(video) $(tone) -t $D -c:v libsvtav1 -preset 12 -c:a flac "$OUT/av1_flac.mkv"
ff $(video) $(surround) -t $D -c:v mpeg2video -c:a ac3 "$OUT/mpeg2_ac3_51.mkv"
ff $(video) $(tone) -t $D -c:v libvpx -deadline realtime -c:a libopus "$OUT/vp8_opus.webm"

# Audio-only and less common audio
ff $(tone 1000 96000) -t $D -c:a pcm_s24le "$OUT/pcm24.mka"
ff $(tone 440 44100) -t $D -c:a libmp3lame "$OUT/mp3.mka"
ff $(surround) -t $D -c:a eac3 "$OUT/eac3_51.mka"
ff $(tone) -t $D -c:a dca -strict -2 "$OUT/dts.mka"
ff $(tone) -t $D -c:a truehd -strict -2 "$OUT/truehd.mka"

# Multi-track with languages, titles, dispositions and subtitles
ff $(video) $(tone 440) $(surround) -i "$TMP/subs.srt" -i "$TMP/subs.ass" -t $D \
  -map 0:v -map 1:a -map 2:a -map 3:s -map 4:s \
  -c:v libx264 -c:a:0 aac -c:a:1 ac3 -c:s:0 srt -c:s:1 ass \
  -metadata:s:a:0 language=eng -metadata:s:a:0 title="Stereo" \
  -metadata:s:a:1 language=fre -metadata:s:a:1 title="5.1 Surround" \
  -metadata:s:s:0 language=spa -metadata:s:s:1 language=und \
  -disposition:a:0 default -disposition:a:1 0 -disposition:s:0 forced \
  -metadata title="Multi track sample" "$OUT/multitrack_subs.mkv"

ff $(video) $(tone) -i "$TMP/subs.vtt" -t $D -map 0 -map 1 -map 2 -c:v libvpx-vp9 -deadline realtime \
  -c:a libopus -c:s webvtt "$OUT/webvtt.webm"
ff $(video) -i "$TMP/subs.vtt" -t $D -map 0 -map 1 -c:v libx264 -c:s webvtt "$OUT/webvtt.mkv"

# Attachments: a font, and cover art (image attachments become attached_pic streams)
FONT=$(ls /System/Library/Fonts/Supplemental/*.ttf /usr/share/fonts/truetype/*/*.ttf 2>/dev/null | head -1 || true)
if [[ -n "$FONT" ]]; then
  ff $(video) -i "$TMP/subs.ass" -t $D -map 0 -map 1 -c:v libx264 -c:s ass \
    -attach "$FONT" -metadata:s:t mimetype=application/x-truetype-font "$OUT/ass_font_attachment.mkv"
fi
ff $(tone) -t $D -c:a flac -attach "$TMP/cover.png" -metadata:s:t mimetype=image/png \
  -metadata:s:t filename=cover.png "$OUT/cover_art.mka"

# Structural edge cases
ff $(video) $(tone) -t $D -c:v libx264 -c:a aac -f matroska - >"$OUT/unseekable_output.mkv"
ff $(video) $(tone) -t $D -c:v libx264 -c:a aac -live 1 "$OUT/live.mkv"
ff $(video) $(tone) -t $D -c:v libx264 -c:a aac -cluster_time_limit 100 -cluster_size_limit 4096 "$OUT/tiny_clusters.mkv"
ff $(video 320x240 30000/1001) -t $D -c:v libx264 -c:a aac -timecode 01:00:00:00 "$OUT/ntsc_rate.mkv"
ff $(video) $(tone) -t $D -c:v libx264 -c:a aac -reserve_index_space 4096 -cues_to_front 1 "$OUT/cues_front.mkv"

# Damaged inputs
head -c 30000 "$OUT/h264_aac.mkv" >"$OUT/truncated.mkv"
python3 - "$OUT/h264_aac.mkv" "$OUT/garbage_inside.mkv" <<'EOF'
import sys
data = bytearray(open(sys.argv[1], "rb").read())
mid = len(data) // 2
data[mid:mid + 64] = bytes((i * 37) & 0xFF for i in range(64))
open(sys.argv[2], "wb").write(data)
EOF

ls -1 "$OUT" | wc -l | xargs echo "files:"
