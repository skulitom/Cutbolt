# Typed property expressions

Scenes support an optional `expressions` graph and the read-only `expression.inspect` command. Both X03 checkpoints pass full verification.

The graph computes position and opacity from exact rational time, ordinary keyframed values, links to other layer properties and reproducible seeded values. Rendering uses the same evaluated bindings reported by inspection. The result remains an ordinary compiled asset for local timeline editing, saved sessions and previews.

## Request and graph

`expression.inspect` accepts `scene` and `times`, an array of exact nonnegative rational scene times. Times may be fractional and include the scene's final endpoint. Inspection validates the scene's structural contract and relevant base curves, evaluates all graph nodes and reports exact values plus rounded property bindings. It reads no media and writes no files. It does not validate image/font identities, spatial transforms, effects or other media-dependent scene semantics; use `scene.inspect` for complete compilation validation.

The optional scene field has this structure:

```json
{
  "schema_version": 1,
  "seed": 17,
  "nodes": [
    {"id":"seconds","kind":"scalar","expression":{"op":"time"}},
    {"id":"rate","kind":"scalar","expression":{"op":"literal","value":{"type":"scalar","value":{"num":5,"den":1}}}},
    {"id":"dx","kind":"scalar","expression":{"op":"multiply","a":"seconds","b":"rate"}},
    {"id":"zero","kind":"scalar","expression":{"op":"literal","value":{"type":"scalar","value":{"num":0,"den":1}}}},
    {"id":"offset","kind":"vector2","expression":{"op":"vector","x":"dx","y":"zero"}},
    {"id":"base","kind":"vector2","expression":{"op":"base","layer":"title","property":"position"}},
    {"id":"position","kind":"vector2","expression":{"op":"add","a":"base","b":"offset"}}
  ],
  "bindings": [{"layer":"title","property":"position","node":"position"}]
}
```

Node IDs must be unique, nonempty labels of at most 128 UTF-8 bytes without control characters. References use IDs, so declaration order does not affect results. A binding selects an existing layer and either `position` (`vector2`) or `opacity` (`scalar`). Only one binding may target each property. Unknown fields and operators reject.

## Types and operations

| Operation | Inputs and result |
| --- | --- |
| `literal` | `value` tagged as `scalar`, `vector2` or `boolean`. Scalar is `{num,den}`; vector is two rationals. |
| `time` | Scalar scene seconds; optional `layer` subtracts that layer's start. |
| `base` | `layer` and `property`; ordinary static or keyframed value before expressions. |
| `property` | `layer` and `property`; linked expression value when bound, otherwise the base value. |
| `link` | `node`; same value and type as that node. |
| `vector` / `component` | Two scalar node IDs `x`,`y` form a vector; `vector` node ID plus `axis` (`x`/`y`) extracts a scalar. |
| `add` / `subtract` | Node IDs `a`,`b`, both scalar or both vector. |
| `multiply` / `divide` | Numeric `a` and scalar `b`; division requires a nonzero divisor. |
| `modulo` | Scalar `a` and positive scalar `b`; result `a - b * floor(a/b)`. |
| `minimum` / `maximum` | Scalar `a`,`b`. |
| `floor` | Scalar node ID `value`; rounds toward negative infinity. |
| `less` / `equal` | Scalar comparison for `less`; matching types for `equal`; boolean result. |
| `and` / `not` | Boolean `a`,`b`, or boolean `value`. |
| `select` | Boolean `condition` and same-typed `yes`,`no` node IDs. |
| `seeded` | Integer `stream` in 0..2^32−1 and scalar node ID `index`; index must be an exact integer in that range. |

All nodes and all branches are evaluated, including unused nodes and unselected `select` arms. A failing branch rejects the request even when it would not supply a bound value. Static types and node/property cycles are checked before evaluation. There is no source-code interpreter, filesystem access, network access, clock access or ambient random state.

## Time, precision and sampling

Timeline clocks remain the existing exact rational `Time` type. An expression's scalar time may be signed: layer-local time is negative before the layer starts and continues after its active interval. Base properties instead clamp local time to `[0, layer duration]` and use the existing curve easing, retiming and nearest-ties-away rounding. A property link exposes the exact expression value before final binding rounding; links to unbound properties expose the ordinary sampled value.

Every scalar intermediate is reduced exactly. The reduced numerator's absolute value must not exceed 9,007,199,254,740,991 and the reduced denominator must be at most 1,000,000,000,000. Excess precision, division by zero and invalid domains reject explicitly. Algebraically equivalent expressions may reach different intermediate bounds; no approximate fallback or expression rewriting occurs.

Bindings must lie within the exact property range before rounding: each position component in −32768..32768, opacity in 0..255. Nearest ties round away from zero. The rounded position is applied before spatial mapping, so linked movement and spatial transforms compose correctly. Ordinary masks, effects and source-frame timing retain their existing clocks. Expressions do not alter source timing or audio.

All scene frame times are evaluated, including times when a bound layer is inactive. This makes domain/range failures explicit rather than concealing them through visibility. Rendering retains the current 25 fps bounded scene profile. Fractional inspection does not itself add motion blur or subframe rendering.

## Reproducible seeded values

`seeded` returns a rational in `[0,1)`. Hash SHA-256 over the bytes `cutbolt-property-seed-v1` followed by one zero byte, then unsigned little-endian `seed` (64 bits), `stream` (32 bits) and evaluated `index` (32 bits). Interpret the first four digest bytes as an unsigned little-endian integer and divide by 2^32. The program seed permits the full unsigned 64-bit range. This is a reproducible property source, not a security token generator.

The value depends only on these explicit inputs, independent of request order, node order, previous frames and process lifetime. Use a time-derived integer index, for example `floor(time * rate)`, for stepped variation. Negative or fractional indices reject.

## Work limits and diagnostics

The graph permits 1..256 nodes, 1..32 bindings and a maximum dependency path of 64 nodes. Read-only inspection permits 1..256 samples and at most 65,536 node evaluations. Ordinary scene compilation retains its own limit of 250 frames, so its maximum is 64,000 node evaluations. Optional [temporal integration](TEMPORAL.md) permits up to 8,000 exposure slots while retaining the aggregate 65,536-node-work limit; out-of-scene slots contribute background and skip evaluation. Graph preparation checks depth independently of traversal order.

`INVALID_EXPRESSION` covers missing references, duplicate bindings and malformed labels. `EXPRESSION_CYCLE`, `EXPRESSION_TYPE`, `EXPRESSION_DOMAIN`, `EXPRESSION_PRECISION` and `EXPRESSION_RANGE` distinguish evaluation failures. Structural schema errors remain `INVALID_JSON`, and work/version bounds use `LIMIT_EXCEEDED`. Rendering validates the complete graph before reading sources or producing media. Existing root, identity, output-collision and publication rules remain in force.

## Acceptance

`tests/expressions.py` authors explicit closed-form Fraction motion and fades, compares every decoded RGB pixel and PCM sample, verifies linked spatial mapping against independent high-precision forward geometry, and checks shuffled node/time order, alternate seeds, fixed hash vectors, typed operators, saved edits/retries/undo and previews. It also exercises actual maximum nodes, bindings, depth and sample counts, plus a 250-frame rendered workload. Fixed latency gates are 15 seconds for maximum inspection and 60 seconds for maximum rendering plus complete decoding/reference comparison. Rejection cases preserve sources, prior output and publication boundaries.

These are bounded position/opacity expressions. General scripting, arbitrary effect parameters, recursion, user-defined functions and unrestricted numerical precision are unsupported.

The retained focused run compares 503 complete frames, 965,760 stereo sample frames and four previews with exact RGB/PCM agreement. All 83 rejection checks pass. The maximum graph inspection takes 0.874 seconds; maximum rendering and decoding takes 0.702 seconds, within the predetermined gates. The source fingerprint remains unchanged during that run. The subsequent integrated run passes all six expression fixture checks plus the exact-arithmetic Rust test, awarding both X03 checkpoints without adding core points.
