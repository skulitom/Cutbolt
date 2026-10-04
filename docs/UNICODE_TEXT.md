# Unicode text layout

The optional `unicode_v1` layout shapes text with explicitly supplied local TrueType fonts. It adds ligatures, kerning, combining marks, contextual forms, mixed writing directions, language-specific substitutions and word wrapping to the existing scene renderer. The default `ltr_scalar_v1` layout and existing recipes retain their behavior. The full acceptance suite verifies G01 extended within this contract; [the generated tracker](PROGRESS.md) remains authoritative.

## Select the layout

Add `layout` to a text graphic:

```json
{
  "kind": "text",
  "text": "A title",
  "fonts": [{"path":"fonts/title.ttf","sha256":"<actual lowercase digest>","bytes":12345}],
  "size": 32,
  "color": [240, 240, 240, 180],
  "rect": [12, 8, 480, 160],
  "line_height": 40,
  "letter_spacing": 0,
  "align": "left",
  "wrap": "word",
  "overflow": "reject",
  "layout": {"profile":"unicode_v1","direction":"auto","language":"und"}
}
```

Use an actual font identity and a box inside the layer canvas. [Text graphics](GRAPHICS.md) describe the complete scene-layer structure. No new command is needed: `scene.inspect` reports the layout, and `scene.render` compiles it into an ordinary editing asset.

`direction` is `auto`, `ltr` or `rtl`, defaulting to `auto`. It sets paragraph base direction; embedded runs retain their own direction. `language` defaults to `und`. It is an ASCII tag with 1–8-character subtags separated by hyphens, at most 63 characters overall; the first subtag contains letters, later subtags letters/digits. The font and shaper determine which language-specific substitutions exist. No text translation or automatic language detection occurs. For example, the acceptance font has an authored Cyrillic substitution selected by `sr`.

## Fonts and shaping

Font paths, identities and limits follow the existing graphics contract: 1–4 ordered fonts per text object, eight distinct font paths per scene, 8 MiB per file and 32 MiB of source font bytes in total. Every supplied font is validated, including unused fallbacks. There is no system-font search, implicit replacement typeface or runtime download.

This profile accepts static monochrome TrueType outlines with OpenType GSUB/GPOS tables. Font collections, CFF outlines, variable-font tables, color/bitmap/SVG glyph tables and alternate AAT substitution tables reject explicitly. It uses fontdue's scalar indexed rasterization with substitution glyph loading enabled. The renderer reports an unsupported font when substituted glyph indices or metrics cannot be represented by this profile. The earlier scalar profile retains its own existing font behavior.

Text is segmented into extended graphemes. Each complete grapheme uses the first supplied font that can shape it without a missing glyph, including supported canonical decomposition when a scalar lacks a direct font mapping. A base and its marks cannot be split between fonts. Missing coverage returns `MISSING_GLYPH`; the engine does not silently draw a replacement box. Adjacent pieces with the same font, script and writing direction shape together, allowing ligatures and kerning. Joining context is retained across font boundaries within a line; substitutions cannot span different font files.

Script runs use Unicode script properties. Common/inherited pieces use a preceding strong script, or a following strong script at the start. Whole-paragraph bidirectional analysis supplies visual runs for each line; the shaper handles glyph order, combining placement and mirroring within them. The engine preserves the original UTF-8 source string and reports shaping clusters, which can span a ligature or include removed direction controls. It does not reverse UTF-8 bytes or assign one glyph to every scalar.

LF separates paragraphs and preserves empty lines. Explicit bidi embeddings/isolates, direction marks, joining/nonjoining controls, zero-width spaces and word joiners are handled by the selected Unicode algorithms. Other control characters, CR/tabs, soft hyphens, U+2028/U+2029 paragraph separators, BOM and variation selectors reject. Automatic hyphenation, vertical writing, color emoji, arbitrary font variations and universal font/script coverage are not claimed. A script needs appropriate glyphs and layout tables in the supplied font.

## Wrapping, spacing and pixels

- `wrap: "none"` uses only LF breaks.
- `"character"` tries successive extended-grapheme boundaries and breaks at the first overflowing candidate. It never splits a grapheme. An oversized first grapheme stays on its line and is then subject to the overflow policy.
- `"word"` prefers the latest fitting Unicode line-break opportunity before that overflow. If no such opportunity exists, it falls back to a grapheme boundary. Spaces are preserved, including at line ends; there is no trimming or automatic hyphen insertion. This option requires `unicode_v1`.

Every selected line is reshaped with context limited to that line. Arabic joining therefore resets correctly at a break. The paragraph's bidi levels remain available for wrapped lines, so the paragraph base direction does not change merely because the next line starts with another script.

Shaping positions use a 1/64-pixel grid. Letter spacing is added between shaped clusters, excluding separate glyphs within one cluster. A ligature therefore receives no internal tracking. Nonzero spacing in a cursive script is an explicit visual spacing choice. Left/center/right alignment refers to advance width, including spaces; center alignment can produce a 1/128-pixel origin.

Baselines remain `box.y + size + line_index * line_height`. Horizontal placement relative to the box and vertical shaping offsets round to nearest pixels, with half ties away from zero. Glyph bearings and grayscale coverage then use the existing rasterizer. Positive shaping Y offsets move upward. `overflow: "reject"` rejects excessive advance, a baseline below the box, or any nonzero out-of-box glyph coverage. `"clip"` discards and counts that coverage. Transparent text is still fully validated.

Glyphs use the existing straight-alpha source-over operation inside the layer. Generated pixels then pass through scene transforms, masks, effects and blend modes. The scene's output still has an opaque background; this does not add a transparent delivery format.

## Inspection and existing workflows

The text report identifies its layout, shaper/rasterizer versions, requested direction/language and per-line UTF-8 ranges, advance widths, baselines and base direction. Each glyph reports its font index, glyph ID, original UTF-8 cluster, script, direction, line, pre-rounding X origin in 1/128 pixels, Y offset/advance in 1/64 pixels and final bitmap rectangle. Work/coverage counts and clipped pixels are also reported. Cluster positions are shaping-cluster starts, not guaranteed individual-character or grapheme boundaries.

Templates retain the layout object in their scene; existing text, font, size and color bindings work normally. For `captions.scene`, add the same object as `layouts.<style>.text_layout`. Caption text remains editable in the original document, while the compiled scene uses the same font/layout validation and sampled cue timing. Saved timeline edits, retries, undo/restoration and frame/range previews operate on the compiled asset. Keep the original recipe, caption document and external fonts for later text edits.

Existing text limits remain: 1–1,024 scalars and at most 4,096 UTF-8 bytes, size 1–128, line height 1–512, spacing 0–128 and a box inside the source canvas. Additional bounds are 1,048,576 shaped input scalars across fallback/wrapping attempts, 8,192 output glyphs, 512×512 glyph bitmaps, 512-pixel absolute advances, bounded bearings/offsets, 16 million cached coverage pixels and 32 million visited glyph-bitmap pixels per text layer. Excessive work fails explicitly. Scene canvas/layer/duration bounds remain in force.

## Verification and provenance

Run `python -X utf8 tests/unicode_text.py --output C:\DEV\CutboltData\new-unicode-check` for a retained fixture. It generates original abstract TrueType outlines and authored GSUB/GPOS tables outside the repository. Manual visual glyph plans and exact rational rectangle-area coverage form the independent reference; the oracle does not invoke the selected shaper. Complete decoded RGB and silent PCM are compared through scene rendering, templates, captions and saved edits. Fractional coverage allows one RGB unit, or two after animated blending, with measured differences recorded.

The fixture covers ligatures, kerning, combining fallback, canonical decomposition, Arabic joining across fonts and line breaks, Indic reordering/conjuncts, Thai marks, localized Cyrillic, Hebrew mirroring/numbers/isolates, supplementary characters, CJK and word wrapping. It also checks empty lines, spacing, clipping, transparency, legacy/Unicode font-cache interaction, typed MCP, retries/undo, previews, maximum text work/memory, malformed fonts, resource limits, changed-font rejection and no-overwrite publication. The default graphics regression remains a separate requirement.

The retained `C:\DEV\CutboltData\unicode-20261003-05` fixture and final verifier compare **144 decoded frames, 276,480 stereo sample frames and four previews**, with **35 rejection/failure cases**. Every observed RGB difference is at most one level, inside declared one-level coverage/two-level animated-compositing tolerances; silent PCM is exact. All 1,024 characters in the maximum-text fixture are actually rendered. Full-run inspection uses **14,581,760 bytes peak working set**, **0.672 seconds** and **387,043 shaped input scalars**, within the unchanged 256 MiB, 30-second and 1,048,576-work bounds. Manual glyph plans, authored original font tables and Fraction rectangle-area coverage form an independent pixel reference without invoking the shaper. Templates, styled captions, saved retries/undo/restoration, typed MCP, frame/range previews, malformed fonts, work limits, changed-font rejection and output/source preservation pass.

The complete frozen-source verifier passes 330 unique checks and earns G01 extended.

Runtime algorithms come from the pinned external [HarfRust](https://github.com/harfbuzz/harfrust), [Unicode bidi](https://docs.rs/unicode-bidi/0.3.18/unicode_bidi/), [grapheme segmentation](https://docs.rs/unicode-segmentation/1.13.3/unicode_segmentation/), [script properties](https://docs.rs/unicode-script/0.5.8/unicode_script/) and [line breaking](https://docs.rs/unicode-linebreak/0.1.5/unicode_linebreak/) libraries. The public [bidirectional specification](https://www.unicode.org/reports/tr9/) describes the relevant text model. The dependency ledger records exact versions/licenses. Original project code coordinates fonts, lines, placement, bounds and existing compositing; no third-party implementation, typeface or generated font is vendored.
