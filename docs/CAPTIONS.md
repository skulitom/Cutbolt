# Timed captions and subtitle files

Cutbolt imports a bounded UTF-8 SRT/WebVTT subset, edits immutable caption snapshots, exports standalone subtitle files, and converts a selected time window into ordinary text layers. `captions.draft` writes a document from transcripts of a timeline's sources. `captions.render` burns a whole document into one transparent overlay. Seven `captions.*` commands are MCP tools; `captions.render` runs through `job.start`. Files, fonts and rendered media stay local. There is no speech recognition, font download or caption-specific saved-session store.

## Commands

Caption scene layouts also accept optional `text_layout` with the [Unicode text profile](UNICODE_TEXT.md), for shaping and mixed writing directions using explicit local fonts. Omitting it preserves the scalar layout. The Unicode fixture verifies this integration independently of the caption-sidecar fixture.

| Command | Required fields | Result |
| --- | --- | --- |
| `captions.draft` | `project`, `transcripts` | Native `document` drafted from what the timeline says, inspection and word counts |
| `captions.import` | `source`, `input_root`, `format`, `id`, `overlap` | Native `document`, inspection and source identity |
| `captions.inspect` | `document` | Validated cue/style usage, end time and maximum simultaneous cues |
| `captions.apply` | `document`, `expected_revision`, `operations` | New document, inspection and per-cue before/after changes |
| `captions.encode` | `document`, `format` | UTF-8 text, byte count/digests, cue ID mapping and loss report; no file write |
| `captions.export` | `document`, `format`, `loss_policy`, `output_root`, `output` | The encoding report and a newly published file |
| `captions.scene` | `document`, `scene`, `scene_id`, `offset`, `layouts`, `sampling`, `layer_prefix`, `input_root` | New scene, full inspection and sampled/skipped cue report |
| `captions.render` | `document`, `layouts`, `project`, `input_root`, `output_root`, `output` | A transparent overlay asset covering the timeline, queued with `job.start` |

Formats are `srt` and `webvtt`. Import uses a relative `{path, bytes, sha256}` source identity beneath an explicit absolute input root, as in [scene identities](SCENES.md). It verifies the actual bytes and never changes the source. Export requires an absolute destination within an existing output root, with `.srt` or `.vtt` respectively. Existing paths are rejected. A temporary file is synced and read back before publication without overwrite. Successful repeated exports to the same path fail with `OUTPUT_EXISTS`; use the returned digest to reconcile a lost response.

## Drafting captions from transcripts

`captions.draft` is read-only. It takes a `project` and `transcripts` of its source media, as `timeline.outline` does. The caption text is the whole words inside audio clips on enabled audio tracks, at their timeline times; muted clips and child sequences are skipped, and `track_ids` narrows placed timelines to chosen tracks. A word a clip edge cuts through is left out and counted in `cut_words`. `start` and `end` caption only the words that start in that range.

Words join the current cue in time order, unless one of four things starts a new cue:
- the silence before the word is at least `pause` (default 0.5 s);
- the previous word ends a sentence: its last character, after closing quotes and brackets, is `.`, `?`, `!` or `…`;
- the cue would last longer than `max_duration` (default 6 s) from its first word's start to the word's end;
- its words would need more than `lines` (default 2) lines of `line_chars` (default 42) characters, filling each line in turn. A single longer word gets a line to itself.

Each cue keeps that greedy line count. Among the ways to break its words into that many lines, it takes the one whose longest line is shortest; ties go to the shortest first line, then the shortest second. Words are joined by single spaces with none at line ends, whatever whitespace the transcript keeps around them (older recognized documents start each word with a space).

A cue starts at its first word's start, rounded down to the millisecond. It lasts at least `min_duration` (default 1 s), but never past the next cue's start, and ends no earlier than its last word, rounded up to the millisecond. Times are then exact milliseconds, so the draft exports to SRT or WebVTT unchanged. Every cue uses the one style `default` (`color`, default white) and `align` (default `center`). The document's overlap policy is `reject` unless overlapping speech made two cues overlap.

Correct the text with `captions.apply`, export a sidecar for upload with `captions.export`, or burn the captions in with `captions.scene`.

## Native document and editing

```json
{
  "schema_version": 1,
  "id": "captions",
  "revision": 0,
  "overlap": "allow",
  "styles": {"warm": {"color": [237, 99, 32]}},
  "cues": [
    {
      "id": "opening",
      "start": {"num": 1, "den": 1000},
      "end": {"num": 81, "den": 1000},
      "text": "First line\nSecond line",
      "style": "warm",
      "align": "left",
      "speaker": "Guide"
    }
  ]
}
```

Times are exact nonnegative rational seconds, with half-open intervals `[start,end)`. The document supports subframe times independently of video. Cues must have nondecreasing start times, positive duration and an end no later than 24 hours. Equal starts retain their document order. `overlap: reject` rejects any simultaneous cues; `allow` preserves them and inspection reports their maximum concurrency. A cue ending exactly when another starts does not overlap it.

Limits are 4,096 cues, 32 styles, 1 MiB total text and 2 MiB per imported/encoded file. Document/cue/style IDs use 1-64 ASCII letters, digits, `_` or `-`; style IDs must start with a letter or `_`. `STYLE`, `NOTE` and `REGION` are reserved. Revision is an exact JSON integer up to 9,007,199,254,740,991. Each cue requires a defined style, left/center/right alignment, 1-1,024 Unicode scalars and at most 4,096 UTF-8 bytes. Text permits LF between nonblank lines, but no trailing LF or other control characters. Speaker metadata is optional (`null`), trimmed, 1-128 bytes and cannot contain controls, `<`, `>` or `&`.

Sidecar text preserves Unicode, including scripts and emoji that the current graphics rasterizer cannot render. Rendering uses either the default [scalar layout](GRAPHICS.md) or the optional [Unicode layout](UNICODE_TEXT.md), subject to the supplied fonts and each profile's bounds. Unrenderable visible text fails scene inspection explicitly.

| Operation | Additional fields | Behavior |
| --- | --- | --- |
| `cue.add` | `cue` | Add a complete cue with a new ID |
| `cue.replace` | `cue` | Replace an existing cue matching its ID |
| `cue.remove` | `cue_id` | Remove an existing cue |
| `style.set` | `style_id`, `style` | Add or replace a color definition |
| `style.remove` | `style_id` | Remove a definition; final cues must not reference it |
| `overlap.set` | `overlap` | Set `allow` or `reject` |
| `cues.shift` | `cue_ids`, `offset`, `backward` | Shift 1-4,096 unique existing cues by exact nonnegative time |

Apply accepts 1-256 operations and checks the supplied revision. It edits a clone, sorts cues stably by start, increments the revision and validates the complete final document. A batch can remove a style and replace its references together. Invalid operations or final state return no new document. Pure retries against the same original snapshot are deterministic. The revision guard is against the supplied snapshot, not an authoritative store: callers must save the returned document and coordinate concurrent changes. The change list contains changed cues; style/overlap changes are visible in the full returned document. Compiled video assets use ordinary saved sessions for persistent timeline edits.

## Supported interchange profile

Both imports require UTF-8, with optional initial BOM and LF, CRLF or CR line endings. Whitespace-only lines separate blocks. Import does not infer encodings, repair malformed intervals or ignore unsupported markup.

SRT requires a positive numeric cue counter, `HH:MM:SS,mmm --> HH:MM:SS,mmm`, and nonblank plain text. There are exactly three millisecond digits; hours have at least two digits and intervals remain within 24 hours. Duplicate cue IDs fail. The plain-text profile rejects `<`, `>` and `&` to avoid ambiguous markup/entity interpretation; the error names the offending cue ID. Use WebVTT for escaped text containing those characters. This profile does not preserve SRT formatting extensions.

WebVTT requires a standalone `WEBVTT` header followed by a blank line. It supports:

- Optional bounded cue IDs; missing IDs receive deterministic `cue-N` values without colliding with explicit IDs.
- `HH:MM:SS.mmm` or `MM:SS.mmm` timestamps and one optional `align:left`, `align:center` or `align:right` setting; default center.
- Plain text or one whole-cue `<c.name>...</c>` color class, optionally wrapped in one whole-cue `<v Speaker>...</v>` voice span. The wrappers can appear in either order; the voice closing tag may be omitted. Class closing tags are required.
- `STYLE` blocks before all cues containing only `::cue(.name) { color: #rrggbb; }` rules. One six-digit hexadecimal color per unique class; an optional final semicolon is accepted.
- Standard undeclared foreground classes `white`, `lime`, `cyan`, `red`, `yellow`, `magenta`, `blue` and `black`. Other classes need definitions. Unclassed text stays white even when a class named `default` has another color; the importer creates a separate white style when needed.
- Character references `&amp;`, `&lt;`, `&gt;`, `&nbsp;`, `&lrm;` and `&rlm;`. Export escapes literal ampersands and angle brackets.

Header metadata, NOTE/REGION blocks, positioning/size/vertical settings, additional CSS, multiple/mixed inline classes, bold/italic/ruby and embedded timestamps are outside this profile and rejected. WebVTT's general syntax is broader; acceptance of this subset is not a claim of full WebVTT conformance or identical layout in every player. Whole-cue color, alignment and speaker metadata round trip. Rendering uses the explicit Cutbolt layout below, not a browser's automatic caption placement.

Export requires exact millisecond-aligned endpoints and returns `UNALIGNED_TIME` instead of rounding. WebVTT retains cue IDs, styles, alignment and speaker metadata. SRT renumbers cues from 1 and reports any changed IDs, removed nondefault style definitions, color/style, alignment or speaker fields. `captions.encode` previews those losses without writing. `captions.export` with `loss_policy: reject` refuses a lossy conversion; `allow_reported` explicitly permits only the reported losses. Text characters outside the SRT profile still fail. Empty WebVTT documents are supported; SRT export requires at least one cue. Document ID, revision and overlap policy are native editing metadata, not sidecar fields.

## Rendering a caption window

`captions.scene` appends text layers to a supplied base scene and gives the result `scene_id`. The caller supplies an explicit layout for each visible style:

```json
{
  "fonts": [{"path": "font.ttf", "bytes": 1234, "sha256": "replace-with-actual-sha256"}],
  "size": 20,
  "rect": [2, 1, 90, 60],
  "line_height": 24,
  "letter_spacing": 0,
  "wrap": "none",
  "overflow": "reject"
}
```

Use actual external font identities. Layout has the same size, box, spacing, wrapping and overflow constraints as graphics text. Optional `valign` (`top` by default, `middle` or `bottom`) places each cue's lines in the box; `bottom` keeps one- and two-line cues on the same bottom line, as subtitles usually are. Optional `background` and `outline` add the [text decorations](GRAPHICS.md) to every cue in that style. Combined with a [transparent scene](SCENES.md#transparent-output), this gives readable burned-in captions on an `alpha_over` track. Cue color and alignment come from the document. The `layouts` object maps style IDs to layouts; unknown styles are rejected. A style with no sampled visible cue does not need a layout. The base scene's audio and existing layers are retained; generated captions are normal straight-alpha layers above them. Later document cues paint over earlier cues. There is no automatic collision avoidance or stacking; choose explicit boxes/styles for simultaneous speakers. Speaker labels are metadata and are not inserted into visible text.

The only sampling policy is `sample_start`: output frame `n` uses caption time `offset + n/frame_rate` on the base scene's clock (25 fps by default). `offset` may fall between video frames and remains exact; the document is unchanged. Active cue intervals are clipped to the selected window and sampled at its frame starts. The report identifies `sampled` cues with inclusive `first_frame` and exclusive `end_frame`, `outside_window` cues, and `no_sampled_frame` cues that lie entirely between samples. For example, `[0.081,0.082)` has no sample in a zero-offset 25 fps scene, but its original subtitle timing remains intact.

Scene duration is 1 frame up to ten seconds at the scene's rate; offset must not exceed 24 hours. Each visible cue becomes one layer named `layer_prefix-cue_id`. The prefix is a bounded ID of at most 32 bytes and generated names must not collide with base layers. All normal scene limits still apply, including **16 layers total**, external font validation, canvas/output dimensions and decoded-memory limits. Repeated cues count as separate layers even when they do not overlap. Each caption layer's canvas is the whole scene, so at 1920 x 1080 a cue uses about 2.07 million of the 64 million decoded-pixel budget; up to the 16-layer limit fits. Select smaller windows when necessary. Scene conversion is read-only; call `scene.render` to compile the returned scene, then add its asset to a saved session. This does not extend the general scene duration; for a whole caption track, use `captions.render`.

## Rendering a whole caption track

`captions.render`, queued with `job.start`, renders a caption document over a timeline as one transparent asset. The asset is straight-alpha FFV1 `bgra` with silent PCM audio, the same profile as a transparent scene, ready for an `alpha_over` video track. It matches the `project`'s canvas and frame rate and covers `[start, start + duration)`, by default the whole timeline. Frame `n` shows the cues active at `start + n / frame_rate`, as `captions.scene` samples them, laid out with `layouts`.

The track is compiled as consecutive caption windows. Each window is an ordinary transparent caption scene with one invisible layer, which keeps a window without cues valid. Windows are as long as ten seconds allows, in whole multiples of a step that keeps them exact in 48 kHz samples and milliseconds (1 frame at 25 fps, 30 frames at 29.97 fps). A window ends early, on a step, before its 16th cue would start, so no scene exceeds 16 layers. The windows are joined losslessly with their exact lengths, and the result is checked for frame count, samples and alpha before it is published. Because windows are whole steps, the asset can run a few frames past the requested duration; `covers` gives the requested length. Place the clip with that duration.

More than 15 cues starting within one step is rejected with `LIMIT_EXCEEDED`. Layout errors, such as a style without a layout, come from `captions.scene` unchanged. No partial output or scratch file remains after a failure.

## Runnable local fixture and workflow

Build the engine and retain the original fixtures outside the repository:

```powershell
cargo build --locked
python -X utf8 tests/captions.py --output C:\DEV\CutboltData\new-caption-test
```

This creates subtitle files, original tiny test fonts and a background image. After the fixture succeeds, this Python example imports its SRT, shifts every cue forward exactly 80 ms, previews export losses, and writes a new sidecar:

```python
from pathlib import Path
import hashlib, json, subprocess

exe = Path(r"C:\DEV\Cutbolt\target\debug\cutbolt.exe")
root = Path(r"C:\DEV\CutboltData\new-caption-test")
source = root / "sources" / "original.srt"
def call(command, **args):
    result = subprocess.run([str(exe)], input=json.dumps(dict(command=command, **args)),
                            text=True, encoding="utf-8", capture_output=True, check=True)
    return json.loads(result.stdout)["result"]
identity = {"path": source.name, "bytes": source.stat().st_size,
            "sha256": hashlib.sha256(source.read_bytes()).hexdigest()}
document = call("captions.import", source=identity, input_root=str(source.parent),
                format="srt", id="edited-captions", overlap="allow")["document"]
document = call("captions.apply", document=document, expected_revision=0, operations=[
    {"op": "cues.shift", "cue_ids": [c["id"] for c in document["cues"]],
     "offset": {"num": 2, "den": 25}, "backward": False}])["document"]
plan = call("captions.encode", document=document, format="srt")
assert plan["losses"] == []
call("captions.export", document=document, format="srt", loss_policy="reject",
     output_root=str(root / "output"), output=str(root / "output" / "shifted.srt"))
```

Save the native document alongside editable scene recipes when further caption changes are needed. Subtitle files provide independent timed-text delivery; embedding subtitle streams in delivery containers and production transcription/review workflows remain open.

## Evidence and sources

The fixture independently demuxes exported SRT/WebVTT with external FFprobe 7.0, checks exact timestamps, text, Unicode, formatting-loss reports and round trips. A Fraction-based clock and original known glyph geometry check every RGB frame across four scene windows and a saved-session cut: **47 frames and 90,240 silent stereo sample frames**. It exercises multiline alignment, fallback, overlapping colors, subframe/unsampled cues, a one-hour boundary, atomic edits, six MCP commands, schema validation, 43 invalid/output-preservation cases and unchanged source bytes. This verifies G03 basic and extended within the documented profile; it adds no speech-recognition or broader Unicode-rendering point.

The implementation is original and uses the public [WebVTT Candidate Recommendation Draft, 20 May 2026](https://www.w3.org/TR/2026/CRD-webvtt1-20260520/) and [Library of Congress SRT format description](https://www.loc.gov/preservation/digital/formats/fdd/fdd000569.shtml) as format references. No specification copies, third-party fixtures or parser implementations are bundled. Existing dependency versions and licenses are unchanged.
