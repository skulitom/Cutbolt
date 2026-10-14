# Placed-footage performance

14 October 2026. The original twenty-minute 1080p30 H.264/AAC source was placed
on separate video and audio tracks. Five alternating warm observations per build
measure the final-frame preview and the final ten-second lossless export. A
neutral-grade case also exercises the compositor path. The predeclared ten-second
**plain-preview ceiling passes** in every current-build observation.

Before: `f13453d`, prior to native layering. After: `04255ee`, with the completed
rendering changes and repeatable H.264 profile. The subsequent snapshot-schema
wording correction changes no render operation.

| Case | Before seconds, median [range] | After seconds, median [range] | Median change |
| --- | ---: | ---: | ---: |
| plain / preview | 3.461 [3.034–4.776] | 4.039 [3.772–4.469] | +16.7% |
| plain / export | 57.927 [57.633–58.336] | 57.941 [57.316–58.737] | +0.0% |
| neutral grade / preview | 7.030 [5.770–7.459] | 7.582 [5.749–8.347] | +7.9% |
| neutral grade / export | 56.506 [56.222–57.468] | 57.079 [56.253–58.378] | +1.0% |

Positive change means slower. These are five observations per cell on one host;
the ranges and recorded load limit what can be inferred from the medians.

| Case | CPU busy before request, before / after | CPU busy during request, before / after |
| --- | ---: | ---: |
| plain / preview | 7.0–14.4% / 8.2–17.2% | 13.8–29.1% / 14.8–36.3% |
| plain / export | 6.3–14.9% / 6.5–18.6% | 25.0–26.9% / 24.1–28.6% |
| neutral grade / preview | 8.0–13.6% / 8.7–17.4% | 13.9–19.7% / 14.8–19.2% |
| neutral grade / export | 7.8–14.4% / 6.5–17.3% | 21.0–23.7% / 21.6–26.1% |

Each request follows a three-second load sample at or below 20% busy. No
compiler activity was observed during measurements. The during-request values include the render
itself. Setup, source indexing, warmup and output verification are untimed.

Every measured request opens one original native-YUV video decoder with an
input seek near the end; none scans original frames from the start. All twenty
previews and twenty timed FFV1/PCM exports match the independent explicit-conform
reference exactly. The final build also passes two separate H.264 direct/conform
pairs, including decoded AAC padding. The original source hash is unchanged.

Lossless timing keeps the same encoding policy in both releases. The H.264
repeatability fix is checked outside those timings. These results cover the
default 8-bit encoded path and a short late range; precise-mode throughput and
general mixed-source workloads require their own measurements.

The [machine-readable evidence](../verification/placed-footage-performance-2026-10-14.json)
retains all forty samples, source/binary identities, decoded hashes and raw-report
identities. The reproducible driver is `tests/placed_long.py`; raw media, logs and
frozen binaries remain external. The thorough milestone is a separate gate.
