# Audio buses, panning and channel routing

Optional `mix.routing` extends the [PCM mixer](AUDIO.md) with explicit buses and named output channels. `audio.inspect` evaluates the graph and reports its layout, route order, PCM identity, meters and clipping. `audio.render` writes a new 48 kHz PCM16 WAV with an explicit speaker mask, verifies its header and every sample, rechecks sources, then publishes without overwriting. Inspection uses the existing read-only MCP tool; rendering remains a blocking local CLI/library call.

Recipes without `routing` retain the original stereo profile and sample behavior. Keep editable recipes and original WAVs. A scene can use a routed mix only when its declared output is stereo; an incompatible soundtrack is rejected before scene rendering. Compile a new scene asset to use the mix in existing saved sessions, undo/history, previews and queued reference rendering. General timeline and delivery surround support remains separate work.

## Layouts and sources

The `output`, track-layout and bus-layout fields accept these exact names:

| Layout | Sample order | Speaker mask |
| --- | --- | ---: |
| `mono` | FC | `0x4` |
| `stereo` | FL, FR | `0x3` |
| `quad` | FL, FR, BL, BR | `0x33` |
| `5.1` | FL, FR, FC, LFE, BL, BR | `0x3f` |
| `5.1(side)` | FL, FR, FC, LFE, SL, SR | `0x60f` |
| `7.1` | FL, FR, FC, LFE, BL, BR, SL, SR | `0x63f` |

FL/FR are front left/right, FC is front center, BL/BR are back left/right, SL/SR are side left/right, and LFE is the low-frequency-effects channel. No automatic bass management, speaker substitution or downmix is applied.

Routed sources are classic mono/stereo PCM16 or WAVEFORMATEXTENSIBLE PCM16 at 24, 44.1 or 48 kHz. Extensible files must declare one supported mask, matching channel count, 16 valid/container bits, the PCM subtype, correct block alignment and byte rate, and complete sample frames. The reader requires a complete RIFF size and one format/data chunk; bounded unknown padded chunks are skipped. Ambiguous classic multichannel files, compressed/float/other-precision WAVs, unsupported masks and malformed chunks fail explicitly.

Channel placement follows the public [WAVEFORMATEXTENSIBLE specification](https://learn.microsoft.com/en-us/windows/win32/api/mmreg/ns-mmreg-waveformatextensible) (20 June 2023), which orders samples by increasing speaker-mask bits. The public [format-tag/GUID mapping](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/converting-between-format-tags-and-subformat-guids) (14 December 2021) identifies PCM. The reader/writer and graph are original project code; no specification or third-party implementation is included.

Each clip retains exact source cuts, output placement, resampling, gain/fades and gain automation from the original mixer. Its `channels` policy determines its track signal:

| Policy | Input and resulting track layout |
| --- | --- |
| `duplicate_mono` | Mono input becomes stereo with identical left/right samples. |
| `preserve_stereo` | Stereo input remains stereo. |
| `preserve_mono` | Mono input remains mono. Requires routing. |
| `preserve_layout` | Retain the validated source layout, which must match the declared track layout. Requires routing. |

All clips on a track must produce that track's declared layout. Use another track and an explicit route when a source requires a different layout. Muted clips and nodes still undergo parameter, source, graph and work validation.

## Graph recipe

The enclosing mix still contains its normal `tracks`, duration and optional master `effects`. Add a `routing` object with:

- `output`: named final layout.
- `tracks`: exactly one `{id, layout}` declaration for every mix track.
- `buses`: up to 16 `{id, layout, gain_milli?, mute?, gain_curve?, effects?}` objects. Gain defaults to 1000, mute to false, and curves/effects are optional.
- `routes`: up to 64 `{id, source, destination, mapping}` objects with unique nonblank IDs.

Node references are `{kind:"track", id}`, `{kind:"bus", id}`, or `{kind:"output"}`. Routes flow from a track/bus to a bus/output. Track and bus IDs have separate namespaces; route references make them unambiguous. Every track/bus must participate in a path to output. Missing nodes, duplicate declarations, cycles and disconnected nodes fail. An empty mix may explicitly produce silence in its chosen layout. Parallel routes are additive, including duplicate signal paths with different route IDs.

This example pans a mono dialogue track into a stereo bus, applies a bus gain and then sends it to the stereo output:

```json
{
  "routing": {
    "output": "stereo",
    "tracks": [{"id":"dialogue","layout":"mono"}],
    "buses": [{"id":"dialogue-bus","layout":"stereo","gain_milli":800}],
    "routes": [
      {
        "id":"pan-dialogue",
        "source":{"kind":"track","id":"dialogue"},
        "destination":{"kind":"bus","id":"dialogue-bus"},
        "mapping":{"type":"pan","law":"equal_power","position_milli":-250}
      },
      {
        "id":"dialogue-output",
        "source":{"kind":"bus","id":"dialogue-bus"},
        "destination":{"kind":"output"},
        "mapping":{"type":"identity"}
      }
    ]
  }
}
```

The enclosing dialogue clips use `channels: "preserve_mono"`. `audio.inspect` returns the submitted routing values plus a deterministic node order; it does not change a saved session.

## Mapping and automation

`mapping` selects one of four types:

| Type | Fields and behavior |
| --- | --- |
| `identity` | Source and destination layouts must match exactly. |
| `matrix` | `coefficients_milli`: destination rows, source columns, signed integer coefficients from -4000 to 4000. A coefficient of 1000 is unity; zero omits that channel and negative values invert phase. |
| `pan` | Mono to stereo only; `law` is `linear` or `equal_power`; `position_milli` is -1000 (left) through 1000 (right). Optional `curve` replaces static position. |
| `balance` | Stereo to stereo only; `position_milli` and optional `curve` have the same bounds. Center leaves both channels at unity; movement attenuates the opposite side. |

A matrix adds all weighted source channels for one destination, then rounds once with signed ties away from zero. Routes add their independently rounded contributions. For example, `[[500,500]]` maps stereo into mono at half amplitude per source channel. A 5.1-to-stereo matrix can explicitly exclude LFE and choose center/surround gains; no standard downmix coefficient is assumed by the engine.

For pan position `p`, linear weights are `(1000-p)/2000` and `(1000+p)/2000`; their sum is one, so center is half amplitude in each channel. Equal-power weights are `cos(theta), sin(theta)` with `theta = pi*(p+1000)/4000`; the squared weights sum to one. Its center is about 0.707 per channel. Endpoints are exact. Equal-power arithmetic uses f64 with final nearest-integer rounding; cross-platform bit identity is not promised. Balance attenuates left by `(1000-max(p,0))/1000` and right by `(1000+min(p,0))/1000`.

Route position curves and bus `gain_curve` use the existing hold/linear/quadratic easing, retiming and checked rational sampler. Their clock is mix-global `sample_index/48000`, including gaps, and their keys must lie within mix duration. Bus gain values are 0–4000; pan/balance values are -1000–1000. Static values are validated even when a curve replaces them. Clip gain curves retain their separate clip-local clocks.

## Processing order and headroom

1. Resample each source sample, then apply existing clip gain, track gain and fades with the original single per-voice rounding. Sum voices into their track channels.
2. A route applies its mapping and rounds each destination contribution. Incoming routes sum without PCM16 saturation.
3. A bus applies its static/animated gain, rounds once, runs its ordered effects and then applies mute. Filters continue through gaps with state reset only at mix start. Output duration does not grow for an effect tail.
4. The output node runs the mix's master effects. Saturate once to PCM16, measure and write the declared channels.

Routing uses a canonical order and checked wide integer headroom. Track/bus sums and gain-scaled signals must remain within an absolute one trillion PCM units; unmuted processed node outputs use the same bound. Excessive graph headroom fails with `AUDIO_ROUTING_OVERFLOW`; it is not silently clipped. Normal overload within that bound clips only at final output, so opposing bus signals can cancel even after exceeding PCM16 range.

Buses and the master accept the existing low-pass, high-pass, peaking and compressor effects, at most eight per node with unchanged parameter limits. EQ state is independent for every channel. Compression links the peak detector across all declared channels, including LFE when present, and applies the same reduction to each. The stereo path retains the previous calculations. Each node's effect chain rounds back to integer once after its final effect; chained buses therefore have declared intermediate rounding. See [audio processing](AUDIO.md) for equations and limits.

Reports include per-channel final peaks, clipping counts and RMS. Stereo output retains its existing integrated-loudness meter. Other layouts explicitly return unmeasured integrated loudness with `loudness_status: "unsupported_layout"`; they do not masquerade as stereo or claim surround loudness/true-peak certification.

## Bounds and publication

Existing limits remain: 1 output sample to 60 seconds, 16 tracks, 128 clips, eight million summed clip sample frames, and 64 MiB of distinct source files. Routing adds 16 buses, 64 routes, at most 16 buses along a path and a 512-million weighted sample-work budget. Work includes channel maps, node channels/effects and active voice channels; dense long graphs can exceed it. Inspection reports the charged work. The graph stores one eight-channel sample frame per node rather than a full-duration buffer per bus. Decoded source and final output buffers retain their bounded sizes.

The original extensible writer declares the precise output layout even for mono/stereo routed exports. Source identities are checked after evaluation and again before publication. Output uses an owned scratch file, decoded format/sample verification and no-overwrite publication; normal failure removes owned scratch files. Existing destination and input files remain protected. A process crash can leave owned scratch files for inspection, as in the other render paths.

## Verification

```powershell
cargo build --locked
python -X utf8 tests/audio_routing.py --output C:\DEV\CutboltData\my-audio-routing-test
$demo = 'C:\DEV\CutboltData\my-audio-routing-test'
$mix = Get-Content -Raw "$demo\surround-buses-downmix-mix.json" | ConvertFrom-Json
@{command='audio.inspect'; mix=$mix; input_root="$demo\sources"} |
    ConvertTo-Json -Depth 40 -Compress | .\target\debug\cutbolt.exe
@{command='audio.render'; mix=$mix; input_root="$demo\sources";
    output_root="$demo\output"; output="$demo\output\extra.wav"} |
    ConvertTo-Json -Depth 40 -Compress | .\target\debug\cutbolt.exe
```

Original signed channel-isolation signals, constant levels and step envelopes feed an independent whole-buffer incoming-graph reference using Fraction arithmetic and 60-digit Decimal panning. External EQ and closed-form compressor step responses verify multichannel state. Tests compare every PCM value, parse headers independently and confirm published channel labels/PCM with the pinned external tools. Cases cover all six layouts and three source rates, matrix rounding, fan-out, bus cancellation/clipping/mute/gain, mixed track layouts, route-order invariance, pan laws/automation, bus/master processing, templates, saved edits and previews. Invalid graphs, curves, WAV metadata/chunks, identities, changed-source publication and existing destinations fail explicitly. Maximum-depth/route tests and a full 60-second 7.1 output check the stated bounds. This earns only the A03 extended gate after full verification; general surround delivery, dialogue repair and recording retain separate criteria.
