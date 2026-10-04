# Textured planes, cameras and lighting

The existing scene recipe supports optional `geometry`. The integrated run passes all eight independent geometry checks, awarding both X01 checkpoints. Keep the editable recipe and source textures; rendering produces a flattened ordinary timeline asset.

## Coordinates and projection

The `textured-planes-v1` profile uses a right-handed world: x right, y up, z toward the default camera. Each node has `id`, optional `parent`, `transform`, and optional `plane`. A transform contains `position_milli`, `rotation_mdeg` and `scale_milli` vectors. Each vector has `value: [x,y,z]` and optional `animation` with existing x/y/z curves. Local transforms apply scale, x/y/z rotations, then translation; parents multiply that result. Positions and scales use thousandths; rotations use millidegrees. Geometry curves use scene-global exact time with the existing interpolation and rounding rules.

A plane contains `layer`, `size_milli: [width,height]`, `material` (`unlit` or `lambert`) and `double_sided`. Its local center is the origin and front normal is +Z. Every scene layer must be bound exactly once. Texture coordinates are `u=x/width+0.5`, `v=0.5-y/height`; both use half-open [0,1) bounds and nearest held texture sampling. Layers retain source crop/offset, rectangular masks, effects, opacity, blend and source timing. Their 2D position/scale/rotation must be neutral, anchors zero, and 2D spatial transforms and position animation/expression bindings absent. Opacity curves and expression bindings remain supported.

The camera has optional `parent`, `position_milli`, `target_milli`, `up_milli`, `projection`, `near_milli` and `far_milli`. Camera vectors use the same value/animation form. Projection is either `{"kind":"perspective","vertical_fov_mdeg":{"value":60000}}` or `{"kind":"orthographic","vertical_size_milli":{"value":100000}}`. These scalar values optionally carry an existing curve under `animation`. Position, target, up, field of view and extent can animate. Parents transform position/target as points and up as a direction; projection scalars are not parent-scaled.

Pixel centers generate camera rays. Clipping uses camera-forward depth, inclusive near and exclusive far. Visible plane samples composite far to near at each pixel. Exactly equal depths sort by ascending node ID, so the later ID composites last. Hidden back faces contribute nothing; double-sided planes flip their lighting normal toward the viewer. Node/layer list order does not determine geometry depth. Quarter-turn trigonometry is exact; other geometry uses bounded f64 arithmetic while all sample times remain rational.

## Declared light behavior

`lights` contains up to eight lights. Each has `kind`, RGB byte `color` and `intensity_milli` scalar (value and optional curve):

- `ambient` adds an unmodulated color gain.
- `directional` adds optional `parent` and a `toward_light_milli` vector, normalized before the diffuse dot product.
- `point` adds optional `parent`, `position_milli`, `falloff` (`constant` or `inverse_square`) and positive `reference_distance_milli`. Inverse-square gain is reference distance squared divided by distance squared. A point exactly coincident with the sample contributes zero.

Lambert shading sums ambient and nonnegative diffuse RGB gains, multiplies the straight encoded-RGB texture by that sum, then rounds/clamps channels to bytes before alpha composition. Gains may exceed one and brighten the texture. This is an explicit encoded-color approximation. Masks/effects precede lighting; lit premultiplied textures are converted to straight color for this operation. Unlit premultiplied textures retain their existing composition behavior. Camera/light parents and light direction/position/intensity can animate. `shadows` must be `"none"`; other shadow modes, arbitrary meshes and materials reject explicitly.

## Bounds and inspection

The existing scene, temporal and expression limits still apply. Geometry additionally permits 1–32 nodes, parent depth at most 16, at most 16 planes, 16,777,216 pixel-plane-sample visits and 32,768 node-sample records. Position components are within ±1,000,000 thousandths, rotation within ±360,000 millidegrees, and positive scale components 1–100,000 thousandths. Plane dimensions are 1–1,000,000 thousandths. Up/light direction components are within ±1,000 thousandths and must produce nondegenerate directions. FOV is 1,000–170,000 millidegrees; orthographic extent 1–2,000,000 thousandths. Clipping requires `0 < near < far <= 2,000,000` thousandths. Intensity is 0–4,000 thousandths, and point reference distance 1–1,000,000 thousandths.

World matrices and inverses must remain finite with coefficient magnitude at most 1e9 and determinant magnitude at least 1e-18. Cycles, unknown parents/layers, duplicate bindings, invalid cameras and excess work reject before publication. Unknown shapes/fields reject at the typed boundary. No silent fallback or automatic reduction of samples is performed.

`scene.inspect` reports the specification, sampled camera/light state, node world matrices, coordinate/depth/lighting rules and work counts. With shutter sampling, records use the same frame-major flattened sample layout as [temporal integration](TEMPORAL.md). `scene.render`, saved timeline edits, retries, undo and frame/range previews use the existing workflow. There is no new command or runtime dependency.

## Independent acceptance

`tests/geometry.py` uses 60-digit forward vertex transforms, camera-space polygon clipping and projected triangle barycentric interpolation to independently predict depth and texture coordinates. Production uses camera-ray intersections. Independent point-light/alpha equations cover lighting. Original external charts exercise perspective/orthographic cameras, intersecting transparency, equal-depth ordering, clipping, back faces, nonuniform parent transforms, animated cameras/lights, masks/effects, premultiplied equivalence, opacity links and shutter samples.

The retained focused run compares 332 complete frames and 637,440 stereo sample frames, four previews and 34 rejected requests. Every tested RGB value matches the reference exactly; lit/graded cases allow a predetermined one-byte tolerance. The actual maximum 16,777,216-visit fixture renders and decodes in 15.851 seconds against a fixed 180-second gate. A second fixture reaches all 32 nodes, 16 hierarchy levels, eight lights and 32,768 records. Typed MCP schema validation, saved retry/undo and unchanged source/output checks also pass. These are bounded plane-scene results, not a general mesh renderer or physical-lighting claim.
