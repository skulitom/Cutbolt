# Direct-footage fixture corpus

11 October 2026. Preparation stage of [the direct-footage plan](plans/DIRECT_FOOTAGE_PLAN.md).
The engine still requires its existing prepared timeline sources. This corpus is a prerequisite
for direct-vs-conform tests, not evidence of direct decoding support.

```powershell
python -X utf8 tools/verify.py --only direct_sources
python -X utf8 tests/direct_sources.py --output C:\DEV\CutboltData\footage-corpus --long-source
```

Use a fresh external output directory (a POSIX path on Linux). Generation never overwrites an
existing source. No media is committed. `tests/footage_sources.py` authors moving pixel patterns,
frame-varying channel codes, a moving rectangle and two independent tones with explicit pauses.
FFmpeg encodes them. `sources/manifest.json` records full encoder/muxer commands, tool versions,
dimensions, clocks, formats and SHA-256/byte identities. There are no downloaded assets or fonts.

The short corpus has 14 files:

- H.264 High at 25, 30, 60 and 30000/1001 fps: GOP 60, three B-frames, fixed GOP decisions, AAC LC
  48 kHz stereo. A separate 30 fps case uses 44.1 kHz mono.
- An MP4 edit-list case starts picture at exactly 0.4 s and carries non-zero-origin AAC timing.
- H.264 with 4:3 sample aspect; another remux has an explicit 90-degree display matrix.
- A VFR file omits every seventh source frame at one offset and preserves the resulting time gaps.
- H.264 with a 250-frame GOP and B-frames.
- HEVC Main with closed and open GOPs, and Main10 in native `yuv420p10le`.
- ProRes 422 HQ (`prores_ks`, profile 3) in native `yuv422p10le` with PCM16 audio.

H.264 uses `libx264`, fast preset, CRF 18; HEVC uses `libx265`, fast preset, CRF 18, two pool/encoder
threads. Scene-cut adaptation is disabled to make GOP boundaries deliberate. Tags declare limited
BT.709. FFprobe independently checks native formats, frame counts, presented timestamps, B-frames,
keyframe spacing, variable intervals, SAR and the display matrix. Original file identities are
checked after probing. Full generated commands, rather than these summaries, are authoritative.

`--long-source` additionally creates a **20-minute 1920 x 1080, 30 fps H.264 file**. It stream-copies
600 repetitions of an original two-second closed GOP and encodes a continuous, newly generated AAC
soundtrack. An explicit 36,000-frame output bound avoids the two extra B-frames a time-only cutoff
can retain. Verification decodes all 36,000 presented frames and checks their exact cadence. The
picture repeats every two seconds; this fixture measures long-file indexing/seeking, not unique
content recognition. Its source audio WAV and movie consume several gigabytes outside Git.

The short matrix passes on Windows FFmpeg 7.0 and Linux FFmpeg 7.0.2; the complete long file has
been generated and checked on Windows. The report explicitly says `direct_decode_verified: false`
until direct timeline decoding and its conform oracle are implemented. No performance claim follows
from these preparation runs.
