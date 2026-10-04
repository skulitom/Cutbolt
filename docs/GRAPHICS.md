# Text and shapes in scenes

Scene layers accept original text, rectangles and ellipses. Text reads explicitly supplied external TrueType font files; shape coverage and layout are calculated locally. These layers use existing crop, scale, quarter-turn rotation, blend modes, masks and position/opacity animation. `scene.inspect` reports layout and selected fonts; `scene.render` compiles a lossless asset for ordinary saved-session editing. No new command, service or font download is required.

Keep the scene recipe and external fonts to edit the text later. The compiled timeline asset is flattened. Recompile to a new output after changing a recipe. Existing PNG scene recipes keep their behavior. See [scene commands and bounds](SCENES.md) and the [dependency ledger](DEPENDENCIES.md).

## Layer contract

Choose exactly one source: nonempty `frames` or a `graphics` object with `frames: []`. A graphics layer requires `timing: "strict"`, `end: "hold_last"` and straight alpha (the default). It remains visible for its half-open layer start/duration, with generated canvas anchor `[0,0]` and offset `[0,0]`. Generated pixels pass through the ordinary scene mask/transform/compositor. The current scene duration, canvas, layer and output limits still apply.

For example, this complete layer creates a translucent ellipse with an inside stroke:

```json
{
  "id": "badge",
  "canvas": [96, 64],
  "start": {"num": 0, "den": 1},
  "duration": {"num": 1, "den": 1},
  "frames": [],
  "graphics": {
    "kind": "shape", "shape": "ellipse", "rect": [12, 8, 60, 40],
    "fill": [50, 160, 220, 180],
    "stroke": {"width": 2, "color": [240, 240, 240, 255]}
  },
  "timing": "strict", "end": "hold_last",
  "transform": {
    "position": [0, 0], "crop": [0, 0, 96, 64],
    "scale": 1, "quarter_turns": 0, "opacity": 255
  }
}
```

Shapes use `shape: "rectangle" | "ellipse"`, integer `rect: [x,y,width,height]`, RGBA `fill` and optional/null `stroke`. Coordinates range from -32768 to 32768; dimensions from 1 to 4096; stroke width from 1 to 256. Coverage uses pixel centers with hard edges. Rectangles include left/top and exclude right/bottom. An ellipse uses the inscribed ellipse equation. A stroke paints over the fill inside the difference between the outer shape and the same shape inset by its width; an empty inner shape makes all covered pixels stroke pixels. Shapes clip to their source canvas before transforms. General paths, gradients and antialiased shape edges remain open.

## Text and external fonts

The contract below describes the default scalar layout. The optional [Unicode layout](UNICODE_TEXT.md) adds shaping, mixed writing directions, grapheme fallback and word wrapping through an explicit `layout` object. Its separate rendered acceptance fixture verifies the broader text checkpoint.

Replace the `graphics` object with this shape of text parameters, supplying actual external font identity values:

```json
{
  "kind": "text",
  "text": "Title",
  "fonts": [{"path": "fonts/title.ttf", "sha256": "<actual lowercase SHA-256>", "bytes": 12345}],
  "size": 20, "color": [240, 230, 210, 255],
  "rect": [2, 1, 92, 62], "line_height": 24, "letter_spacing": 1,
  "align": "left", "wrap": "character", "overflow": "reject"
}
```

Font paths are relative normal components under the explicit absolute `input_root`, with a lowercase SHA-256 digest and exact nonzero byte count. They follow the same containment and before-publication identity checks as image inputs. A text object takes 1-4 ordered fonts; each scalar uses the first supplied font containing it. All declared fonts are validated, including unused fallbacks. Missing glyphs fail with `MISSING_GLYPH`; there is no silent substitute font. Conflicting identities for the same font path fail. The profile accepts a single TrueType outline font, excluding font collections, WOFF and CFF/OpenType containers. It uses default outlines without variation selection, hinting, kerning, ligatures, color glyphs or automatic substitutions.

Text requires 1-1024 Unicode scalars and at most 4096 UTF-8 bytes, size 1-128 pixels, line height 1-512, nonnegative letter spacing 0-128 and a positive box wholly inside the source canvas. Size is the font em size. Every line's baseline is `box.y + size + line_index * line_height`; font ascender values do not move that baseline automatically. Choose enough top/bottom space for the actual glyphs. LF starts a new line, preserving empty lines. Spaces retain their font advances. Letter spacing is added only between scalars on the same line.

`wrap: "none"` uses only explicit newlines. `"character"` starts another line before a scalar whose advance plus spacing would exceed the box width, except that the first scalar always stays on its line. This is scalar wrapping, not word or language-aware line breaking. `align: "left" | "center" | "right"` aligns each line's advance width inside the box. Glyph origins round to the nearest pixel with half ties away from zero; glyph raster coverage remains grayscale. The fixed fontdue 0.9.4 scalar rasterizer supplies glyph coverage; the engine's layout and shape code are original.

`overflow: "reject"` fails when a line's advance exceeds the box, its baseline passes the box bottom, or any nonzero glyph coverage lies outside the box. `"clip"` discards out-of-box glyph coverage and reports its count. Transparent text is still laid out and validated. Overlapping glyphs and inside strokes use straight-alpha source-over within the layer; this intermediate 8-bit result then enters scene compositing. Output still has the scene's opaque background.

This first layout profile, `ltr_scalar_v1`, accepts LF plus these inclusive Unicode scalar ranges: U+0020-007E, U+00A0-024F, U+0370-0482, U+048A-052F, U+2010-2027, U+2030-205E, U+3000-3029, U+3030-303F, U+3041-3096, U+30A1-30FA, U+3400-9FFF and U+AC00-D7A3. This permits bounded Latin, Greek, Cyrillic, CJK and precomposed kana/Hangul inputs if the supplied fonts contain them. Other scalars, tabs, CR, combining marks, bidi controls, RTL scripts and emoji are rejected with `UNSUPPORTED_TEXT`. There is no normalization or complex shaping. The optional [Unicode profile](UNICODE_TEXT.md) provides broader shaping and layout; the scalar profile alone does not earn the extended text checkpoint.

## Inspection, animation and limits

Each graphics layer's timing report includes a `graphics` object. Text reports per-glyph scalar, font index, line, bitmap rectangle, baseline and advance, plus line widths and clipped coverage count. The report identifies its layout and rasterizer version. A graphics layer's `selected_frames` array contains 0 for active frames and null for inactive frames. Its source cycle duration equals the declared layer duration. Source identities include fonts, and the scene digest includes every text/shape parameter.

Use ordinary `animation.position_x`, `position_y` and `opacity`, or animated rectangular masks, to move/fade/reveal text and shapes. These share the exact rational scene clock, easing, timing remap and validation. Their text, font size, shape geometry and base colors are static within a layer; changing those needs separate layers or a new recipe. Optional [grading effects](GRADING.md) can animate exposure, contrast and white balance over the generated pixels while preserving their alpha. [Reusable templates](TEMPLATES.md) now generate independent recipes from typed values and validated bindings. [Timed captions](CAPTIONS.md) add exact cue editing, SRT/WebVTT sidecars and explicitly sampled caption layers using this same text renderer.

At most eight distinct font paths, 8 MiB per font and 32 MiB total are accepted per scene. Each glyph bitmap is at most 512 squared, advances are finite and 0-512 pixels, bearings are bounded, and a text layer's glyph coverage cache is limited to 16 million pixels. Generated canvases count toward the scene's 16-million decoded-pixel budget. Fonts remain external, and source/output overwrites are refused.

## Verification

Run `python -X utf8 tests/graphics.py --output C:\DEV\CutboltData\new-graphics-test` for a retained fixture. It generates original abstract TrueType glyphs externally with FontTools, compares all 100 decoded RGB frames and 192,000 silent stereo sample frames, and exercises fallback order, multiline alignment/wrapping, clipping, transparency, strokes, blends, mask/position/opacity animation, replay and saved-session editing. The independent oracle uses known outline rectangles, analytic pixel-area coverage, rational layout/blending and forward image transforms. Integer-aligned cases match exactly; fractional glyph edges permit at most one 8-bit RGB level and report the measured maximum. Thirty-five invalid cases verify explicit rejection and no output publication. Full verification includes this fixture and existing scene regressions.
