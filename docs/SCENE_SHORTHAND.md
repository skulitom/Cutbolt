# Scene shorthand

`scene.inspect`, `scene.still` and `scene.render` accept an explicit `"form":"short"`
recipe for text, rectangles and ellipses. An inline template's `scene` accepts the same form
with ordinary typed parameter bindings. Full scenes retain their existing syntax and defaults.
Shorthand expands before font resolution and identity binding, then uses the existing scene
validator and renderer. It adds no rendering semantics or approximate timing.

The original [title example](../examples/title-card-short.json) is under 1 KB. Put an external
TrueType font at `font.ttf` under the input root, then use the file's JSON as `scene`:

```json
{
  "command": "scene.inspect",
  "scene": {
    "form": "short", "id": "title", "size": [640, 360], "duration": 3,
    "background": "#142030",
    "layers": [{
      "id": "label", "anchor": "center",
      "content": {"type": "text", "text": "Opening title", "font": {"path": "font.ttf"}},
      "animation": {"opacity": {"from": 0, "to": 255, "duration": 0.4, "ease": "ease_out"}}
    }]
  },
  "input_root": "C:/DEV/CutboltData/inputs"
}
```

Replace the input directory and font with local inputs. In a bound workspace, omit `input_root`.
`font` can also be an unambiguous installed family or full name, such as `"Inter Regular"`
when that exact name exists. [fonts.find](FONTS.md) discovers available names and identities;
ambiguity is an error. No fonts are bundled or copied.

Successful scene commands return `expanded_scene`, with resolved font identities. For queued
`scene.render`, `job.start` returns the expansion and saves that exact recipe in the job;
`job.wait` returns the render receipt and scene digest. A later project edit or font-name change
cannot reinterpret the queued recipe. `graphics.instantiate` returns the fully bound `scene`
as usual. Save the expanded recipe when editing fields outside the shorthand vocabulary.

## Canvas and time

`size` is `[width,height]` in logical pixels, or `"project"`. For the latter, supply a `project`
snapshot or saved reference beside `scene`; `graphics.place` uses its requested saved revision.
The shorthand's `frame_rate` defaults to that project's native clock when provided, otherwise
25. Output scale is 1. Explicit dimensions and frame rate override project defaults. Templates
may bind the usual output fields after expansion.

`duration` is required. Seconds may be integer/decimal numbers or exact `{num,den}` rationals,
using the engine's existing decimal conversion. Scene/layer bounds must still lie on whole
frames and required sample boundaries. There is no implicit duration rounding at fractional
rates. Each layer's `start` defaults to zero and its `duration` to the scene's remaining time.

`background` defaults to opaque black. Hex colors accept `#RGB`, `#RGBA`, `#RRGGBB` or
`#RRGGBBAA`, case-insensitively, with straight alpha. A background must be opaque;
`transparent:true` selects transparent output using the existing scene profile.

## Layout and animation

`safe_area` is the inset in pixels on every edge, defaulting to floor(min(width,height)/20).
It must leave a nonempty rectangle. Each layer may give a `box_size:[width,height]` fitting
that safe rectangle. Its `anchor` selects layout:

| Anchor | Box placement and default size | Default text alignment |
| --- | --- | --- |
| `top_left` (default) | Top-left of safe area; entire safe rectangle | Left/top |
| `center` | Center of canvas, rounding position down; entire safe rectangle | Center/middle |
| `lower_third` | Bottom-left of safe area; safe width and floor(safe height/3), minimum 1 | Left/top |

Content is one of `text` (with `text` and `font`), `rectangle` (with `fill`), or `ellipse`
(with `fill`). Shapes fill the box. Text defaults to 32-pixel em size, opaque white,
ceil(size*6/5) line height, zero letter spacing, character wrapping and overflow rejection.
`size`, `color`, `line_height` and horizontal `align` can override those text defaults.

Every layer expands to a full-canvas source with crop covering the canvas, scale 1, no rotation
and position `[0,0]`. Layer `position` translates that layout; `opacity` defaults to 255.
Optional `animation.position_x`, `position_y` and `opacity` each take `from`, `to`, positive
`duration`, optional `delay` (zero) and `ease` (`ease_out`). Named easings are the existing
`hold`, `linear`, quadratic `ease_in`, `ease_out` and `ease_in_out`. These expand to two keys
on the layer-local clock, holding endpoints outside their interval. Values and key times
retain the full scene's normal range and duration checks.

Use the full scene form for PNG sources, audio, masks, effects, arbitrary transforms,
multi-key curves, Unicode shaping and other advanced fields. Unknown shorthand fields fail;
they are never silently discarded. Existing frame, pixel-work, font and layer limits apply.
