# Local editorial interchange

The original `otio-editorial-v1` adapter maps a bounded public OpenTimelineIO JSON subset into native editable tracks and back. It uses exact rational time, explicit local media bindings and reviewable loss reports. The Rust engine has no additional runtime dependency and never fetches a media URL. Import proposes a new snapshot; save it explicitly with `session.create` and continue with ordinary editing, preview, export and undo/history.

The generated [progress tracker](PROGRESS.md) determines verified coverage. Focused acceptance and the integrated full run pass. This adapter addresses I02 editorial interchange. It does not implement I03 native application projects.

## Commands

All three commands are available through JSON CLI, library and MCP stdio.

| Command | Required fields | Result |
| --- | --- | --- |
| `interchange.import` | `source`, `input_root`, `media_root`, `id`, `width`, `height`, `frame_rate`, `bindings` | Proposed `project` only when ready, names/mappings, losses and source identity |
| `interchange.export.inspect` | `project`, `input_root` | Proposed OTIO `document` and loss report; no files written |
| `interchange.export` | `project`, `input_root`, `output_root`, `output` | New file path, SHA-256, byte count and loss report when ready |

Import and export accept optional `acknowledged_losses`, an array of exact loss IDs returned by inspection. Inspection without acknowledgements remains useful: `ready:false` is a successful analysis result, not permission to use an incomplete project. Unknown, duplicate or blocking acknowledgements reject. Acknowledgements apply to the current request; rerun inspection after changing the document or project.

Each loss contains `id`, `path`, `code`, `message` and `blocking`. Every nonblocking loss must be acknowledged before import returns a project or export writes a file. Blocking losses cannot be overridden. Export inspection includes a proposed document when only nonblocking losses exist; blocking structure returns `document:null`. Import always withholds the proposed project while losses remain unacknowledged. `required_acknowledgements` lists only nonblocking losses.

An import request has this shape. The identity and asset fields must describe actual caller-owned files; placeholder hashes are not usable identities.

```json
{
  "command": "interchange.import",
  "source": {"path":"edit.otio", "sha256":"<document SHA-256>", "bytes":1234},
  "input_root":"C:/local/edits",
  "media_root":"C:/local/media",
  "id":"imported-edit", "width":1920, "height":1080,
  "frame_rate":{"num":25,"den":1},
  "bindings":[{
    "target_url":"clips/shot.mkv",
    "asset":{
      "id":"shot", "path":"clips/shot.mkv", "duration":{"num":10,"den":1},
      "identity":{"sha256":"<media SHA-256>","bytes":5678}
    }
  }],
  "acknowledged_losses":[]
}
```

`source.path` uses relative normal components under an existing absolute `input_root`. The `.otio` source must be at most 4 MiB and match its declared size/hash. `media_root` is a separate existing absolute directory. A binding's `target_url` is an exact logical string from the document: it is never interpreted as a download, filesystem permission or automatic path search. Its asset path may be absolute or relative to `media_root`; canonical resolution must remain inside that root. Used media and attached proxies must carry matching byte counts and SHA-256 identities. Ready imports resolve both into canonical absolute paths under the supplied media root so the returned snapshot works with existing rendering and proxy previews. Duplicate logical references reject; several different references can bind the same identical asset ID. Unused import bindings are not added to the project.

The caller supplies canvas dimensions and timeline frame rate because this profile does not infer them from editorial metadata. Import validates the resulting native project. Native rendering still requires its separately documented 25 fps reference-media profile; parsing an editorial document does not perform media conversion or broaden codec support.

## Supported mapping

- `Timeline.1` with an untrimmed, enabled `Stack.1`; up to 32 flat `Track.1` tracks, 1,000 clips and 1,000 bindings. Video order is bottom to top. Audio tracks sum. Track enabled state is preserved.
- `Clip.1` and `Clip.2`, including explicit selection of the active media reference. `ExternalReference.1` needs an `available_range` whose duration exactly equals the bound native asset duration. Missing media, unknown reference types and nested compositions block conversion.
- Clip `source_range` supplies the source window; an omitted range uses the entire available range. Available-source origins can be signed. The native source position is the exact difference between the clip and available-range starts. Export normalizes media origins to zero; original display labels are not reconstructed.
- `Gap.1` advances timeline position and becomes transparent video or silent audio. Gap source origins do not affect blank output and normalize to zero. Export writes explicit gaps for empty placements and pads shorter tracks to the arrangement duration. Positive empty arrangements use a gap-only video track.
- `SMPTE_Dissolve` maps to the native generic cross-dissolve between two adjacent enabled clips. `in_offset` is the portion before the cut; `out_offset` is after it. It adds no timeline duration. Source handles must exist, and the interval must fit the two adjacent clips. End/gap fades, transitions spanning split clips and multiple native effects on one cut block conversion.
- Rational values and rates must be finite exact safe integers, including JSON spellings such as `25.0`. Rates must be positive; durations and transition offsets are nonnegative. Fractional numeric spellings are rejected instead of rounded. Native frame/sample alignment and overflow checks also apply. Audio placements can retain individual 48 kHz sample boundaries.

Import generates unique native track/clip/transition IDs and returns a name-to-ID report. IDs, project revisions and saved history are not round-tripped. Export uses current IDs as editorial names. Native sequential projects are promoted in a temporary copy into picture/sound tracks. Their implicit linked editing becomes an explicit reported loss; source media, timing and output remain unchanged. No snapshot or saved session is mutated by this promotion.

## Reported omissions and boundaries

Nonempty metadata, effects, markers, display color and available image bounds require acknowledgement before omission. So do inactive media alternatives, nonzero global display/timecode origins and disabled clip definitions; disabled clips become empty intervals. Unsupported or disabled transition types become cuts only after acknowledgement. Effects are not evaluated or translated by this adapter.

Native track locks, editing links, registry metadata, unused assets, proxy attachments and selected proxy previews are reported before omission. Export always references original media. Reusable sequence definitions/instances and unknown fields/schema versions block conversion. The first structural error may prevent inspection of later objects; fix it and inspect again. Loss reports are not a claim of arbitrary editor compatibility or identical transition sampling in other renderers.

Export checks every registered asset, including unused assets, under its absolute `input_root`. It emits relative forward-slash media references, uses a newly created sibling temporary file and publishes with the existing no-overwrite hard-link operation. The output must have `.otio` extension, stay under an existing absolute `output_root`, and fit the same 4 MiB limit. Existing files are never replaced. Media identities are checked again before publication. Normal failures remove only the owned temporary file; abrupt process termination can leave that file for explicit cleanup. The checks do not defend against a hostile writer replacing files during filesystem operations.

## Independent acceptance

`tests/interchange.py --output <new-external-directory> --reference-python <external-python>` invokes the pinned OpenTimelineIO 0.18.1 library through the original `tests/interchange_reference.py` fixture generator. The library authors the input, reads engine exports and normalizes them for a second import. Original independently calculated RGB/PCM expectations cover overlapping/disabled video, asymmetric dissolves, one-sample audio placements, summed audio, explicit/trailing gaps, sequential promotion and saved edits/undo. Unknown constructs, loss acknowledgements, versioned clips, signed origins, multiple boundary effects, identities, roots and output preservation are checked separately. A moved directory with relative source/proxy bindings is imported, rendered at full quality and previewed at half size.

Set `CUTBOLT_OTIO_PYTHON` to the selected external Python for full verification. No package is installed by the verifier. Dependencies, generated timelines, media, failed attempts and hash records remain external. The [dependency ledger](DEPENDENCIES.md) pins the tested package and environment.

Public design references: [versioned OTIO file format](https://opentimelineio.readthedocs.io/en/v0.18.1/tutorials/otio-file-format-specification.html) and [timeline structure](https://opentimelineio.readthedocs.io/en/v0.18.1/tutorials/otio-timeline-structure.html). Engine code and fixture generators are original; no library implementation, application file or specification copy is included.
