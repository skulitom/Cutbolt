# Working color

Stage six is in development. Saved projects can now explicitly select working
precision with `project.working_space`. The default remains the existing 8-bit
encoded renderer. The bounded banding references pass; the complete stage gate
is still open.

## Saved selection

Apply `{"op":"project.working_space","working_space":{"bits":16,"light":"linear"}}`
through `timeline.apply` or `session.apply`. Bits may be 8 or 16; light may be
`encoded` or `linear`. All four combinations are accepted. Null removes the setting;
an omitted setting or explicit 8-bit encoded selection retains the existing renderer.
Setting the field upgrades only the new snapshot to schema 2. Receipts describe
the change; retries, undo and restore preserve the recorded older snapshots.

Nondefault modes store straight RGB and coverage at the selected depth. Eight-bit
values use multiples of 257 in a 16-bit carrier. Every working store rounds to the
declared depth. Blending, interpolation, transition means and shutter samples use
the chosen light. Pixel effects retain their continuous linear equations, with
continuous encoded sRGB for HSL, LUTs and keys, then store in the selected context.

`project.transfer` interprets native footage RGB and the encoded working space;
its nondefault-mode fallback is BT.709. Scene image and graphics inputs remain
sRGB and convert into that context. Set the project transfer to sRGB explicitly
when native reference footage uses sRGB. H.264/PNG export and scopes use the same
fallback; an export `input_transfer` that contradicts it is rejected.

Saved normalization recipes retain their own `working_transfer`; changing the
project declaration does not rewrite or re-normalize those assets. Automatic
recipes default to BT.709. Supply a matching explicit recipe when preparing
footage for an sRGB project, as required by the existing normalization contract.

Sequential edits use an ephemeral track arrangement during rendering without
rewriting saved clips. Generated sources and recursive sequence frames retain
working pixels until final delivery. Their shared caches include the working
context. Source identities, clocks, audio windows and publication checks keep
their existing contracts. The active frame-buffer estimate remains bounded to
1 GiB, including recursive children, wide frames and proxy property canvases.

Frame/range previews, reference renders and PNG delivery encode the project
transfer at the final byte store. H.264 encodes directly to BT.709 there, including
joined render chunks, avoiding a prior 8-bit sRGB store. Final RGB8 delivery uses
the ordered 8 by 8 dither below. Child scenes/sequences never use delivery dither.
Explicit `box_floor` sampling still floors block means at the declared working
depth; explicitly authored `byte_alpha` still rounds effective alpha to 8 bits.
Ordinary `coverage` opacity retains the selected working depth.

The Rust library's `color::working::Pixel` stores straight linear SDR RGB and alpha
as four unsigned 16-bit values. This is the foundation for retaining precision
through pixel effects, spatial sampling, generated pictures and nested sequences.
It does not change the default encoded 8-bit compositor.

## Arithmetic contract

- Source import decodes the declared sRGB or BT.709 transfer once. For encoded
  premultiplied input, it removes premultiplication before decoding. Alpha is
  coverage and never passes through a transfer function. Invalid premultiplied
  samples are rejected.
- A floating-point effect result can be stored directly as linear UNORM16. Values
  must be finite; the SDR interval is clipped to 0..1 and rounded to nearest, with
  half ties upward. There is no intervening encoded 8-bit value.
- Source-over handles transparent and opaque backdrops. Normal, multiply and
  screen operate in linear light. Source alpha, 16-bit opacity, the existing
  0..65536 mask weight and backdrop alpha retain all product bits until each
  output channel is rounded to UNORM16. Products use bounded unsigned 128-bit
  arithmetic; no intermediate coverage round is needed.
- Interpolation weights associated linear color and alpha, then returns straight
  RGB. Hidden color contributes nothing. Endpoints preserve their input exactly.
- Final delivery transfer-encodes RGB and quantizes to 8 bits. Ordered delivery
  uses an 8 by 8 Bayer tile: recursively expand `[[0,2],[3,1]]`, and use threshold
  `(rank + 0.5) / 64` in `floor(255 * encoded + threshold)`. The canvas coordinates
  fix its phase across frames and tiles. All RGB channels share the threshold;
  alpha uses nearest rounding without dither. A nearest-only library option
  supports comparisons.

Each stored working value has 1/65535 spacing. Quantizing to that representation
can move an encoded half-code tie to either side; it does not promise byte identity
with an unquantized floating-point pipeline. The existing default path remains
separate and keeps its byte-identity contract.

## Effect and layer components

The next component, `effects::working::Effects`, validates an ordered stack and
loads its identity-bound LUTs under an explicit input root. Its sampled `Frame`
processes working pixels directly. Grades and tone curves stay in floating-point
linear light through the stack; the final store rounds to UNORM16 once. Hue and
saturation, HSL qualifiers, LUTs and chroma-key distance use continuous encoded sRGB
values. Key coverage, unmix and spill never require an intermediate byte. Correction
masks keep their declared integer source-canvas coordinates and feather semantics.
The existing byte processor retains its original qualifier and key-distance rules.

`animation::Sampler` now exposes the validated curve evaluator to Rust callers.
The original `sample` returns the same rounded integer. `sample_scaled` multiplies
the exact eased rational value before rounding (scale 257 turns a 0..255 opacity
curve into 0..65535 coverage). `sample_f64` supplies continuous effect controls.
Key selection, easing and retiming still use exact rational time. No duration-sized
frame table is allocated.

`tests/working_effects.py` compares this component with independent Fraction clocks,
f64 effect equations, Python HSL conversion and closed-form LUT references. It
checks ordering, masks, animated controls, all LUT interpolation modes, source
identities, neutral/transparent pixels, and coverage levels within a byte interval.
These are component checks; saved-timeline checks are described separately below.

`color::working::layer::Layer` now validates the same source-canvas transform,
animation and mask contract as native clip properties. A prepared layer draws
borrowed straight-linear pixels onto a possibly transparent working canvas.
Decoded image offsets and per-frame anchors remain source-canvas coordinates.
The layer reuses the existing exact clocks and spatial mappings, including
fitting, mirroring, viewports and stabilization compensation. Integer transforms
use the working source-over primitive. Filtered spatial transforms accumulate
associated color, alpha, mask coverage and fixed bilinear weights before the final
blend and UNORM16 store. No filtered tap or layer opacity passes through a byte.
Source and destination buffers together are bounded to 1 GiB; the caller owns
decoding, delivery and its full render-plan memory accounting.

The independent `working_layers` fixture uses Decimal geometry, Fraction taps and
mask coverage, and f64 compositing equations. It checks complete layer frames,
transparent layer stacks, animated pixel effects and bounds. Its 4,096-level graded
gradient retains distinct levels at sub-byte opacity. This validates the layer
component; it is not the plan's final saved-timeline grade/fade and delivery gate.

## Prepared scene frames

`scene::working::Source` accepts the existing full scene recipe and prepares its
original inputs for linear UNORM16 frames. It shares source validation, exact
clocks, expression evaluation, geometry and soundtracks with the default path.
PNG and rasterized graphics are decoded into working pixels once; binary mattes
are exact zero/keep operations before this decode. A shared picture has a separate
cached interpretation when layers declare different alpha modes. Held frames,
offsets, anchors, tiles and generated graphics retain their source semantics.

Opacity curves and graph bindings reach UNORM16 without a byte-sized control in
between. Geometry and position still follow their existing bounded contracts.
Only the conservatively proven static layer prefix is cached; comparing rounded
byte controls could wrongly freeze a changing precise fade. Effects and filtered
layers use the working components described above, with shared identity-bound LUTs.

The 3D path shares ray intersections, clipping, texture coordinates and stable
depth order with the default renderer. Its declared Lambert material keeps its
encoded-color lighting response continuously, then stores linear UNORM16 without
an intervening byte. Source-over blends in linear light. Shutter samples average
associated linear color and coverage, with one nearest UNORM16 store. Integer
enlargement preserves pixels exactly; reduced proxies use associated box means.
Hidden RGB never contributes to those means. All supported native scene clocks
use exact rational exposure times.

This source returns working pixels and bounded PCM windows. It neither encodes
nor publishes media. The caller must invoke its full source verification before
publication. Prepared image, LUT and audio data stay pinned even if an input
changes; verification rejects changed content. Reports distinguish actual wide
opacity from the retained legacy byte controls and identify linear integration.
Retained payload, conversion overlap, and frame buffers have separate 1 GiB bounds;
these are conservative payload estimates, not a process-memory guarantee. A parent
render still has to charge its whole generated-source pool and concurrent buffers.

`working_scenes` compares complete prepared frames against independent Fraction
clocks/coverage, Decimal forward polygon projection and f64 color equations. It
covers source alpha, binary mattes, geometry, lighting, expressions, fades, temporal
samples, proxies, tiles, graphics and audio windows, with a declared tolerance of
two UNORM16 codes per final channel. It does not render through a saved timeline.

## Verification and remaining work

`tests/working_color.py` builds a bounded adapter to the actual library primitives
and compares them with independent Python f64 equations. It covers all legal
premultiplied byte-channel/alpha pairs for both transfers, transparent source-over,
interpolation, delivery, and recursive dither construction. The declared tolerance
is one UNORM16 code per working operation and exact encoded delivery. A 1,024-level
ramp requires strictly increasing tile means and an error of at most 1/128 delivery
code. This arithmetic fixture is registered in the ordinary verifier.

`working_timelines` compares saved sequential edits, grades, opacity, generated
clips, recursive sequences and all four transitions with independent f64 equations
for every nondefault mode. Frame and range previews must match render windows
exactly. The fixture checks H.264's actual encoder-input digest against a direct
BT.709 conversion and PNG against the reference frames. Declared tolerance is one
delivered RGB8 code; observed errors are recorded rather than inferred from the
component tests. Explicit default and cleared settings must remain byte-identical.

`working_banding` checks a 1,024-frame temporal ramp and 33 frames of a slow
256-level spatial gradient through a grade and saved opacity fade. Linear16
matches the independent reference bytes exactly on Windows and Linux: all 1,024
temporal tile means and all 256 spatial levels remain distinct, with no added
plateau. This is a bounded precision result; encoded and linear light intentionally
differ in brightness. [Recorded evidence](../verification/working-timelines-2026-10-13.json)
includes numerical reports and source hashes.

`working_footage` adds complete-frame comparisons for direct H.264/HEVC and
identity-bound cached 10-bit normalization in each nondefault mode. It covers
source-clock grades, LUTs, keys, masks, spatial properties and blends; independent
Fraction references cover block-floor shrink, authored byte alpha and native
straight-alpha compositing. Reference-master proxies retain source coordinates
and apply delivery dither on the reduced canvas. Generated PCM, queue retries,
cache context keys and changed-source publication checks exercise the shared
workflow boundaries. Quick reports retain these numerical results in their
`working_footage` field. Proxy generation retains its documented reference-master
requirement; source normalization still outputs native RGB8.

The complete default-path quick gate and final performance audit remain required
before closing stage six. This checkpoint makes no HDR, speed or points claim. Source normalization
still supplies the supported native RGB8 inputs; wider working storage does not
recover source detail already quantized during normalization.
