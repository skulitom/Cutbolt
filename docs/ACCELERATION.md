# Optional hardware video decoding

The acceptance fixture passes full verification against the merged sources, and both P02 checkpoints are awarded; see the generated [progress tracker](PROGRESS.md).

`media.conform` and `media.conform.inspect` accept an optional `decode` field inside the recipe. Omission retains software decoding. Selection is explicit:

```json
{"backend": "cpu"}
```

```json
{"backend": "cuda", "device": 0, "unavailable": "error"}
```

`unavailable` may be `error` or `cpu`. The CUDA option initializes the specified local device (ordinal 0–31) through the selected external media tool. An unsuccessful initialization exit either returns `DEVICE_UNAVAILABLE` or selects software with the reason in the receipt. Missing tools and the ten-second initialization timeout retain `TOOL_UNAVAILABLE` and `TOOL_TIMEOUT` under both policies. Unsupported media is rejected independently; fallback does not make unsupported editing semantics valid. Failure during a later decode is an error, with no automatic retry on another backend.

The current hardware subset is progressive H.264, 8-bit YUV420, even source dimensions from 128 × 128 through 1,920 × 1,080, inside the existing MP4/MOV conversion profile. All existing color, identity, timestamp, duration, decoded-size, audio, source-root and output-publication rules remain in force. Reference FFV1, audio-only sources and sources outside these hardware bounds use the default software recipe or reject an explicit hardware request. The pinned muxer's tested VFR/B-frame combination lacks a final frame duration and is rejected by the existing source-clock contract; supported VFR fixtures retain that duration.

Only source-video decoding is accelerated. Source inspection, timestamp mapping, RGB/color/LUT processing, audio, FFV1 output and output validation still use the CPU. Hardware frames are explicitly downloaded to NV12 before the existing conversion path. The required hardware-frame filter prevents a successful request from silently using software decoding while reporting CUDA. These are public external interfaces described by the [upstream FFmpeg hardware guide](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.0/ffmpeg-with-nvidia-gpu/index.html); no SDK source or binaries are included in this repository.

Inspection reports requested/selected backend, selected device, initialization time and any fallback reason. It initializes a local GPU context when CUDA is requested, but writes no media or editing state. Execution adds decode-stage and prepublication elapsed measurements. The acceptance fixture measures complete command wall time separately, including publication and serialization. Initialization and GPU-to-host transfers can make short inputs slower than software; no general speedup is promised.

The tested development machine has a GeForce RTX 4090 and driver 591.86. The selected external FFmpeg 7.0 build already contains its hardware decode interface. Driver/runtime components remain external under their own licenses; this feature installs or downloads nothing. Missing tools/drivers/devices are reported through the declared policy. Device availability at inspection does not reserve it for later execution.

`tests/acceleration.py --output <new-external-directory> --device 0` generates original RGB/PCM fixtures and encoded H.264/AAC sources. It compares actual hardware output with an independently decoded software reference, exact rational source-time lookup and exact decoded-audio sample slices. The declared maximum RGB difference is one level; PCM must match exactly. It exercises a real unavailable ordinal, optional fallback, strict failure, source changes, existing output preservation, normalization/resize, typed inspection, saved native assets and repeated throughput measurements. The isolated unavailable-device fixture assumes ordinal 31 is absent. Both P02 checkpoints pass stable full verification completed 4 October 2026.

The expanded focused fixture passes six groups: **1,170 decoded frames and 2,246,400 stereo sample frames**, with eleven rejected cases. Every measured RGB difference is zero within the declared one-level gate. Controlled post-initialization tool failure, truncated decoded frames and changed source contents reject without publishing output or silently switching to software. A timed-out initializer and missing tools retain their specific errors. Ordinary failure cleanup removes only the command's own scratch paths.

Three repetitions per backend measure complete-command medians of **51.9 output fps on CPU and 39.8 fps on CUDA** for 320 × 180, and **3.33/3.35 fps** for a complete 100-frame 1,920 × 1,080 conversion. Isolated decode-stage medians are **898.3/319.0 source fps** and **234.9/174.2 source fps**, respectively. These disk-backed measurements show no useful overall speedup from this bounded GPU-decode route; other CPU stages dominate full-HD conversion. Software remains the default. Normalization/resize and saved preview agreement are checked separately from the frame/sample totals. The complete verifier passes, including actual device selection and the original numerical gates.

Full acceptance uses `python tools/verify.py --decode-device 0` with the already required external speech runtime configuration. Supply the actual selected CUDA ordinal; the bounded fixture reserves ordinal 31 as unavailable. A missing device fails acceptance instead of substituting software and awarding hardware coverage. Engine use does not require a GPU when the recipe omits `decode` or selects `cpu`.
