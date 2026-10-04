# Reusable scene templates

`graphics.instantiate` expands a portable original template with typed parameters, validates the complete resulting scene and external source identities, and returns an editable scene recipe. It is available through the CLI, Rust library and MCP stdio. The operation writes no files or saved sessions. Use `scene.render` to compile the returned recipe to a new asset, then add it to an ordinary timeline or saved session.

Original reusable examples are [lower-third-template.json](../examples/lower-third-template.json) and [title-card-template.json](../examples/title-card-template.json). Each exposes text, supplied fonts, text/panel colors, font size, duration and a shared fade curve. The lower-third also exposes a horizontal offset; the title card exposes a shared vertical entrance curve. Both use a 160 by 90 logical canvas, output scale 4, and a three-second default duration. Fonts are required external inputs. These recipes contain no typeface, media asset or third-party template.

## Request and result

| Request field | Meaning |
| --- | --- |
| `command` | `"graphics.instantiate"` (omit when using the MCP tool) |
| `template` | Complete template JSON object; file paths are not loaded implicitly |
| `instance_id` | Nonblank scene/compiled-asset ID, at most 128 bytes |
| `values` | Map from declared parameter names to typed values; use `{}` when every parameter has a default |
| `input_root` | Existing absolute directory for every external source used by the resolved scene |

The result contains `scene`, `inspection`, resolved `parameters`, sorted `defaulted_parameters`, `template_id`, `template_sha256` and `scene_sha256`. The scene digest agrees with a subsequent `scene.render` receipt. Digests cover native serialized recipes, not rendered pixel bytes. Whitespace and JSON object-key order do not alter them. Ordered arrays remain significant; rearranging parameter declarations can change the template digest even when the expanded scene is unchanged. Source identities in `inspection` identify the actual external files.

Each call clones the base scene and sets its ID to `instance_id`. Reusing a template, changing later values or failing validation cannot mutate an earlier returned scene. Instances have no hidden link to the template: changing a template later requires explicit re-instantiation and recompilation. Store both the editable template/values and concrete recipe when continued editing is required. The compiled timeline asset remains flattened.

## Template schema

A template has `schema_version: 1`, a nonblank `id` up to 128 bytes, a `scene` using the ordinary [scene schema](SCENES.md), and `parameters`. Base fields replaced by bindings may be placeholders; the complete scene is validated after all values are applied. For example, the supplied examples use an empty base font list which the required `fonts` parameter must replace. All base layer IDs must be unique and nonblank before binding.

Each parameter has a unique `name`, a `definition`, and one or more `bindings`. Names use 1-64 ASCII letters, digits, underscores or hyphens. At most 64 parameters and 256 total bindings are accepted. Parameter definitions declare a `type`, optional nullable `default`, and the constraints below. Null or omitted defaults make the parameter required. Supplied values are explicitly tagged, such as `{"type":"text","value":"Opening title"}` or `{"type":"integer","value":20}`. There is no string-to-number conversion or implicit fallback on invalid input.

| Type | Definition fields besides `type` and `default` | Value and constraints |
| --- | --- | --- |
| `text` | `max_chars: 1..1024` | Nonempty string within this scalar limit and 4096 UTF-8 bytes |
| `integer` | `min`, `max` | Signed 32-bit integer within the inclusive declared range; min must not exceed max |
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
| `scene_duration` | `time` | Scene duration |
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

One parameter may bind multiple compatible targets, such as a shared color or fade. A target can have only one binding across the template. Static and animated bindings for the same property conflict, and a static binding cannot be hidden by a base animation curve. Layer IDs, source types and binding types are checked explicitly. Templates do not execute scripts or expressions, resolve nested templates, interpolate strings, infer layout or automatically retime media/keyframes.

Changing duration is explicit. A parameter may bind both scene and layer durations, as in the examples, but existing fade/movement keys must still fit the new layer clocks. Supply corresponding new curves when shortening an instance. Retiming a curve uses the existing scene curve contract and does not change activation ranges.

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

Send that scene object to `scene.render` with the same input root and an unused output path. MCP callers use `cutbolt_graphics_instantiate` with the same fields except `command`, then retain the returned scene for explicit CLI/library compilation. There is no template-specific saved-session format or render queue; compiled assets reuse existing media/session operations.

## Failures and verification

Missing/unknown values fail with `MISSING_PARAMETER`/`UNKNOWN_PARAMETER`. Wrong value types and declared-bound violations fail with `INVALID_PARAMETER`. Invalid definitions, duplicate or incompatible bindings and missing target fields fail with `INVALID_TEMPLATE`. Malformed JSON/unknown fields fail with `INVALID_JSON`. Resolved scenes preserve the existing detailed font, identity, timing, animation and layout errors. No partial scene or output is published on failure.

`python -X utf8 tests/templates.py --output C:\DEV\CutboltData\new-template-test` retains the original-font fixtures and instantiated recipes externally. It independently assembles expected scenes for two preset variants each and a custom multiline template, checks every RGB frame and silent PCM sample across 70 frames, and verifies compiled saved-session cuts, deterministic replay through MCP, parameter order, schema validation and isolation. Forty invalid cases cover types, constraints, missing media, incompatible targets, clock/layout errors and read-only failure behavior. Template example files participate in the verification fingerprint. This earns G02 extended; no expression or broader text-layout checkpoint is claimed.
