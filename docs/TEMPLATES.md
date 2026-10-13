# Reusable scene templates

`graphics.instantiate` expands a portable original template with typed parameters, validates the complete resulting scene and external source identities, and returns an editable scene recipe. It is available through the CLI, Rust library and MCP stdio. The operation writes no files or saved sessions. Place the returned recipe as a [generated scene clip](GENERATED_CLIPS.md), or use `graphics.place` for one saved edit. `scene.render` remains available for a standalone compiled asset.

The binary includes 20 original designs, discoverable with `templates.list`. Original reusable examples are [lower-third-template.json](../examples/lower-third-template.json) and [title-card-template.json](../examples/title-card-template.json). Each exposes text, supplied fonts, text/panel colors, font size, duration and a shared fade curve. The lower-third also exposes a horizontal offset; the title card exposes a shared vertical entrance curve. Both use a 160 by 90 logical canvas, output scale 4, and a three-second default duration. Fonts are required external inputs. These recipes contain no typeface, media asset or third-party template.

## Request and result

| Request field | Meaning |
| --- | --- |
| `command` | `"graphics.instantiate"` (omit when using the MCP tool) |
| `template_id` or `template` | Exactly one built-in ID or complete inline template object; paths are not loaded implicitly |
| `project` | Optional target snapshot or saved-project reference; defaults canvas, scale 1 and native frame rate |
| `instance_id` | Nonblank scene/compiled-asset ID, at most 128 bytes |
| `values` | Map from declared parameter names to typed values; omit or use `{}` when every parameter has a default |
| `input_root` | Existing absolute directory for every external source used by the resolved scene |

The result contains `scene`, `inspection`, resolved `parameters`, sorted `defaulted_parameters`, `template_id`, `template_sha256` and `scene_sha256`. The scene digest agrees with a subsequent `scene.render` receipt. Digests cover native serialized recipes, not rendered pixel bytes. Whitespace and JSON object-key order do not alter them. Ordered arrays remain significant; rearranging parameter declarations can change the template digest even when the expanded scene is unchanged. Source identities in `inspection` identify the actual external files.

Each call clones the base scene and sets its ID to `instance_id`. Reusing a template, changing later values or failing validation cannot mutate an earlier returned scene. Instances have no hidden link to the template: changing a template later requires explicit re-instantiation and replacing its saved clip or compiled asset. Store both the editable template/values and concrete recipe when continued editing is required. The compiled timeline asset remains flattened.

## Built-in catalog and output fitting

Call `templates.list` with no arguments to list every ID, one-line purpose, typed parameter/default and output size; pass `template_id` to inspect one entry. The listing omits scene internals. All designs require external fonts, supplied by identity or an unambiguous installed name from [fonts.find](FONTS.md).

| IDs (each prefixed `cutbolt-` and suffixed `-v1`) | Designs |
| --- | --- |
| `lower-third`, `lower-third-rule`, `lower-third-marker` | Three lower thirds |
| `title-card`, `title-center`, `title-split` | Three title cards |
| `end-card`, `section-divider` | Closing message and section heading |
| `callout-arrow`, `callout-box`, `chapter-marker`, `progress-bar` | Annotations; progress uses an editable `fill_rect` |
| `caption-outline`, `caption-box`, `caption-accent` | Three caption styles |
| `quote-card`, `reminder`, `countdown`, `name-tag`, `list-reveal` | Quotation, original follow reminder, timed countdown, speaker tag and timed list |

Each built-in exposes `width`, `height` and `output_scale` as integers, `frame_rate` as a time rational, and `transparent` as a boolean. Width/height are logical pixels; final size multiplies them by output scale. Without a project, defaults produce 640 by 360 pixels at 25 fps, for three seconds (four for the countdown). Transparent overlays keep alpha; title, quote, countdown, divider and end cards have opaque backgrounds. The two existing examples retain their original opaque output.

With `project`, output defaults to its dimensions and native frame rate at scale 1. Transparency keeps the design's default because projects have no transparency setting. Explicit parameter values override these defaults. Built-ins fit their authored layout: rectangle edges/positions scale per axis; text, spacing and stroke sizes scale uniformly by the smaller canvas ratio within renderer limits. Aspect changes can alter whitespace and text wrapping; overflow remains an explicit error. Supplied pixel values are final logical coordinates and are not scaled again. Inline templates keep their authored layer coordinates even when project defaults change their output canvas.

Built-ins proportionally retime layer activation ranges and default curves when duration changes. Activation edges round to the nearest frame (half up); every layer must retain at least one frame. Native fractional rates use 1001/1000-second design beats by default, preserving whole frames, audio samples and milliseconds. Explicit durations must meet the usual native-clock and sample alignment rules. Supplied curves remain explicit and must fit their layer clocks. Inline templates retain their original explicit timing behavior.

A project-aware result additionally reports `target_project` (ID/revision) and sorted `project_defaulted_parameters`. The built-in template digest covers the fitted recipe, so output dimensions/rate/duration can change it; the scene digest covers the resolved scene as usual. Existing inline calls without a project retain their prior behavior.

For example, after choosing an available font:

```json
{
  "command": "graphics.instantiate",
  "template_id": "cutbolt-title-center-v1",
  "instance_id": "opening",
  "input_root": "C:/DEV/CutboltData/inputs",
  "values": {
    "fonts": {"type": "fonts", "value": ["Your exact installed font name"]},
    "label": {"type": "text", "value": "Opening title"}
  }
}
```

The font name and directory above are placeholders to replace with local inputs. This returns an expanded, inspectable scene. Inline templates can use [explicit scene shorthand](SCENE_SHORTHAND.md) as their base scene; parameter bindings apply to its expanded layers.

## Place a graphic in one job

`graphics.place` expands a built-in or inline template against a saved project and saves its
pinned scene as a [generated video clip](GENERATED_CLIPS.md) in one edit. Pictures render
directly in the timeline, with no intermediate movie. Through MCP, queue it with `job.start` and read
its result with `job.wait`. For an existing project and font file in a bound workspace:

```json
{
  "command": "job.start",
  "request_id": "title-job",
  "run": "graphics.place",
  "arguments": {
    "project_id": "edit",
    "expected_revision": 3,
    "request_id": "title-edit",
    "instance_id": "intro-title",
    "template_id": "cutbolt-lower-third-v1",
    "values": {
      "fonts": {"type": "fonts", "value": [{"path": "font.ttf"}]},
      "label": {"type": "text", "value": "Hello"},
      "duration": {"type": "time", "value": 1},
      "transparent": {"type": "boolean", "value": true}
    }
  }
}
```

Replace the project, revision and font with actual local values. Pass the returned `job_id` to
`job.wait` with `timeout_seconds: 120`. The result includes the new `revision`, `scene_id`,
`scene_sha256`, `clip_id`, `duration`, `compiled: false` and the ordinary session receipt.
The saved recipe is in the project; no media asset or output movie is created. The original synthetic title fixture uses
two MCP calls, 833 sent bytes on Windows / 830 on Linux including JSON-RPC envelopes, and no schema lookup.
This is a scripted workflow measurement, not a fresh-model trial.

`start` defaults to zero. `track_id` defaults to `graphics`; an absent track is added above the
existing tracks as enabled video `alpha_over`. An existing destination must have that type,
be enabled and unlocked, and have no overlapping clip. `instance_id` names the saved scene
and clip, and must not collide with an existing media asset ID. A sequential timeline is promoted with linked video/audio unchanged; an empty project
gets a timeline ending at the graphic's end. A nonempty timeline is never extended implicitly.
The template keeps its transparency choice, and scene soundtracks are rejected because this
command places video only. Dimensions and native frame rate must match the project.

Outside a workspace, provide `store_root`, `input_root` and `output_root`. The last field
remains an existing directory inside `input_root` for compatibility with earlier requests;
new placements leave it empty. A workspace supplies these roots. Fonts and other scene inputs
remain content-pinned external files. Saving the generated clip upgrades the new revision to
schema 2. Undo retains the recorded recipe for restoration.

Instantiation and source validation hold no database write lock. The final short transaction
rechecks the head revision and saves the track and clip together. Failed preparation,
collisions, revision conflicts and cancellation before commit leave the project unchanged.
Identical edit retries return the committed receipt even after later edits or undo; changed
arguments with that request ID fail. Concurrent identical requests commit once.

Retries of placements made by an older engine return their original compiled `asset` response,
including after undo. Their media is preserved for saved history. New generated placements
return the scene response above. Use `session.receipt` with the **inner edit request ID** to
resolve an interrupted job or ambiguous commit; job errors include that lookup. A new job may
retry the same inner request safely. This does not add automatic job retry or a new store format.

## Template schema

A template has `schema_version: 1`, a nonblank `id` up to 128 bytes, a `scene` using the ordinary [scene schema](SCENES.md), and `parameters`. Base fields replaced by bindings may be placeholders; the complete scene is validated after all values are applied. For example, the supplied examples use an empty base font list which the required `fonts` parameter must replace. All base layer IDs must be unique and nonblank before binding.

Each parameter has a unique `name`, a `definition`, and one or more `bindings`. Names use 1-64 ASCII letters, digits, underscores or hyphens. At most 64 parameters and 256 total bindings are accepted. Parameter definitions declare a `type`, optional nullable `default`, and the constraints below. Null or omitted defaults make the parameter required. Supplied values are explicitly tagged, such as `{"type":"text","value":"Opening title"}` or `{"type":"integer","value":20}`. There is no string-to-number conversion or implicit fallback on invalid input.

| Type | Definition fields besides `type` and `default` | Value and constraints |
| --- | --- | --- |
| `text` | `max_chars: 1..1024` | Nonempty string within this scalar limit and 4096 UTF-8 bytes |
| `integer` | `min`, `max` | Signed 32-bit integer within the inclusive declared range; min must not exceed max |
| `boolean` | None | Explicit `true` or `false` |
| `color` | None | Four 8-bit straight RGBA channels |
| `fonts` | None | 1-4 ordered [external font identities](GRAPHICS.md#text-and-external-fonts) |
| `curve` | None | Ordinary scene curve with 1-128 keys and exact rational times |
| `time` | None | Nonnegative exact `{num,den}` rational; frame alignment is checked in the resulting scene |
| `rect` | None | Integer `[x,y,width,height]`; coordinates -32768..32768, dimensions 1..4096 |

Defaults are checked against their type and declared constraints even when overridden. Property-specific constraints, media availability, glyph coverage, curve ranges and scene clocks are checked on the resolved instance. For example, a text box must fit its canvas, a supplied glyph must exist, and a layer's duration must fit the scene. Increasing an integer parameter's declared maximum does not remove the scene's own limits. A template scene can select the optional [Unicode layout](UNICODE_TEXT.md); instantiation retains that object and validates the resolved text and external fonts.

Every binding has `property` and, for layer properties, `layer` naming a base scene layer. Scene properties must omit `layer` (or use null). Bindings cannot use arbitrary JSON paths. The supported properties are:

| Property | Required parameter type | Target |
| --- | --- | --- |
| `scene_background` | `color` with alpha 255 | Scene RGB background |
| `scene_duration`, `scene_frame_rate` | `time` | Scene duration or one of eight native rates |
| `scene_width`, `scene_height`, `scene_output_scale` | `integer` | Logical canvas dimensions or integer output scale |
| `scene_transparent` | `boolean` | Straight-alpha output |
| `layer_start`, `layer_duration` | `time` | Layer activation range |
| `text` | `text` | Text graphic content |
| `text_color` | `color` | Text RGBA color |
| `fonts` | `fonts` | Text font list |
| `font_size`, `line_height`, `letter_spacing` | `integer` | Text layout field |
| `shape_fill` | `color` | Shape fill |
| `stroke_color`, `stroke_width` | `color`, `integer` respectively | An existing shape stroke; a missing stroke is an error |
| `rect` | `rect` | Text box or shape rectangle |
| `position_x`, `position_y`, `opacity` | `integer` | Static transform field |
| `position_x_curve`, `position_y_curve`, `opacity_curve` | `curve` | Corresponding animation curve; creates the animation object if needed |

One parameter may bind multiple compatible targets, such as a shared color or fade. A target can have only one binding across the template. Static and animated bindings for the same property conflict, and a static binding cannot be hidden by a base animation curve. Layer IDs, source types and binding types are checked explicitly. Templates do not execute scripts or expressions, resolve nested templates or interpolate strings. Only the built-in fitting path adjusts authored layout and default timing as described above.

For inline templates, changing duration is explicit. A parameter may bind both scene and layer durations, as in the examples, but existing fade/movement keys must still fit the new layer clocks. Supply corresponding new curves when shortening an instance. Retiming a curve uses the existing scene curve contract and does not change activation ranges.

## Local example

Choose an existing TrueType font beneath an input root, and an existing external output directory. This Python example loads the original lower-third recipe, supplies a font identity and title, and saves the returned editable scene with no overwrite:

```python
import hashlib, json, subprocess
from pathlib import Path

repo = Path(r"C:\DEV\Cutbolt")
inputs = Path(r"C:\DEV\CutboltData\my-inputs")
output = Path(r"C:\DEV\CutboltData\my-output")
font = inputs / "fonts" / "title.ttf"
template = json.loads((repo / "examples/lower-third-template.json").read_text())
font_identity = {
    "path": font.relative_to(inputs).as_posix(),
    "sha256": hashlib.sha256(font.read_bytes()).hexdigest(),
    "bytes": font.stat().st_size,
}
request = {
    "command": "graphics.instantiate", "template": template,
    "instance_id": "opening-title", "input_root": str(inputs),
    "values": {
        "label": {"type": "text", "value": "Opening title"},
        "fonts": {"type": "fonts", "value": [font_identity]},
    },
}
process = subprocess.run(
    [str(repo / "target/debug/cutbolt.exe")],
    input=json.dumps(request).encode(), capture_output=True, check=True,
)
reply = json.loads(process.stdout)
with (output / "opening-title-scene.json").open("x", encoding="utf-8") as file:
    json.dump(reply["result"]["scene"], file, indent=2)
```

Send that scene object to `scene.render` with the same input root and an unused output path. MCP callers use `cutbolt_graphics_instantiate` with the same fields except `command`, then compile the returned scene with `job.start`, `run: "scene.render"` and `arguments` containing the scene and output paths. `job.wait` returns the compiled asset for existing media/session operations. The blocking CLI/library path remains available.

## Failures and verification

Missing/unknown values fail with `MISSING_PARAMETER`/`UNKNOWN_PARAMETER`. Wrong value types and declared-bound violations fail with `INVALID_PARAMETER`. Unknown values name the undeclared parameter and its given type; value/binding type errors name the parameter and both types. Invalid definitions, duplicate or incompatible bindings and missing target fields fail with `INVALID_TEMPLATE`. Malformed JSON/unknown fields fail with `INVALID_JSON`. Resolved scenes preserve the existing detailed font, identity, timing, animation and layout errors. No partial scene or output is published on failure.

`python -X utf8 tests/templates.py --output C:\DEV\CutboltData\new-template-test` retains the original-font fixtures and instantiated recipes externally. It independently assembles expected scenes for two preset variants each and a custom multiline template, checks every RGB frame and silent PCM sample across 70 frames, and verifies compiled saved-session cuts, deterministic replay through MCP, parameter order, schema validation and isolation. Forty invalid cases cover types, constraints, missing media, incompatible targets, clock/layout errors and read-only failure behavior. Template example files participate in the verification fingerprint. This earns G02 extended; no expression or broader text-layout checkpoint is claimed.

`tests/builtin_templates.py` renders all 20 designs at their actual defaults, compares selected RGB/RGBA movie frames with stills and all silent PCM samples, and independently checks whole-pixel synthetic-glyph variants using analytic shape/glyph coverage and integer RGB compositing. It tests project fitting, output overrides, inline binding constraints, all eight native rates for countdown/list timing, and core/full MCP parity. Its alpha regression compares every alpha byte at every integer output scale; transparent export enlarges bytes in Rust to preserve exact preview/movie agreement. Authored catalog JSON participates in progress and cached-verification fingerprints.
