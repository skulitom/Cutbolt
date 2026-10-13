# Working color: arithmetic foundation

Stage six is in development. Project snapshots, scenes, previews and exports still
use their existing rendering path. There is no `working_space` setting yet.

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
These are component checks: project settings and renderer integration are still
required before the feature is usable through scene or timeline commands.

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

## Verification and remaining work

`tests/working_color.py` builds a bounded adapter to the actual library primitives
and compares them with independent Python f64 equations. It covers all legal
premultiplied byte-channel/alpha pairs for both transfers, transparent source-over,
interpolation, delivery, and recursive dither construction. The declared tolerance
is one UNORM16 code per working operation and exact encoded delivery. A 1,024-level
ramp requires strictly increasing tile means and an error of at most 1/128 delivery
code. This arithmetic fixture is registered in the ordinary verifier.

Renderer integration remains open: saved project settings and history, wiring
working layer frames into scenes and tracks, transitions, generated and nested pictures,
memory accounting, previews, proxies and delivery. A final conversion must happen
only at delivery; a child sequence or generated source cannot be rounded through
the old compositor first. The source interpretation and all supported combinations
of depth and light must be explicit. The plan's gradient-through-grade-and-opacity
test, default-path equivalence, complete quick gate, and final performance audit
remain required. This checkpoint makes no stage-six, banding, HDR, speed or points
claim.
