# Dependency and provenance ledger

Recorded 3 October 2026. Original Cutbolt code and documentation use the repository's MIT license. Dependency licenses remain separate; MIT on this project does not relicense FFmpeg or its other dependencies.

## Prepared editorial interchange reference

A separate external development environment pins **OpenTimelineIO 0.18.1** (Apache-2.0) with CPython **3.12.14** for independent interchange acceptance. It uses the published Windows x64 wheel and the public [versioned format specification](https://opentimelineio.readthedocs.io/en/v0.18.1/tutorials/otio-file-format-specification.html), [timeline semantics](https://opentimelineio.readthedocs.io/en/v0.18.1/tutorials/otio-timeline-structure.html) and [release license](https://github.com/AcademySoftwareFoundation/OpenTimelineIO/blob/v0.18.1/LICENSE.txt). The original [editorial adapter](INTERCHANGE.md) and fixture generators use this library for independent authoring, export readback and normalization, followed by decoded video/audio comparisons. The same package crashed during import in the separate Anaconda CPython 3.11.5 environment; the working selection is explicitly 3.12.14. Focused adapter acceptance and the complete integrated verifier pass, awarding I02 basic/extended. Package files, generated timelines and installation/hash records stay external; the running engine has no new dependency or network requirement.

## Verified optional hardware decode

The separate hardware-decode development slice uses the already selected external FFmpeg 7.0 CUDA/NVDEC interface, with a GeForce RTX 4090 and installed driver **591.86** in its initial local tests. The installed driver remains an external proprietary runtime; no driver, SDK source, headers, plugins or new Rust dependency are vendored or installed by the engine. Public interfaces are described by the [upstream hardware guide](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.0/ffmpeg-with-nvidia-gpu/index.html). See [current implementation and limits](ACCELERATION.md). Full acceptance passes on the selected device/build; this does not establish portability across devices or driver versions.

## Optional local speech profile

The original [transcription adapter](TRANSCRIPTS.md) selects external **Whisper small multilingual**, separate English/Greek CTC models and their public Python interfaces. No recognizer implementation, upstream documentation, package or model is vendored. Recognition, forced alignment and model execution use the external libraries below; source clock conversion, bounded tiling, energy refinement, contextual editing intervals, supervision, document corrections and timeline planning are original project code. Optional runtime setup is explicit and external, with no download or network access during inference.

The tested profile uses Windows with WSL Ubuntu, Linux Python **3.10.12**, CUDA **12.1** package builds and an RTX 4090. Linux user/PID/network namespace isolation uses the separately installed `unshare` utility from util-linux **2.37.2** (installed package copyright reports GPL-2-or-later for this utility). The worker enforces the thirteen core package versions listed in its `VERSIONS` map; the remaining table entries record the resolved external graph, rather than promising that every transitive import is runtime version-checked. Package license labels below come from installed distribution metadata/classifiers. They record dependency selection, not a redistribution bundle or replacement for each license.

| External speech package | Exact selected version | Reported license |
| --- | --- | --- |
| openai-whisper | 20250625 | MIT |
| torch | 2.3.1+cu121 | BSD-3-Clause |
| torchaudio | 2.3.1+cu121 | BSD |
| transformers | 4.44.2 | Apache-2.0 |
| tokenizers | 0.19.1 | Apache |
| huggingface-hub | 0.24.7 | Apache |
| safetensors | 0.4.5 | Apache |
| numpy | 1.26.4 | BSD |
| numba | 0.60.0 | BSD |
| llvmlite | 0.43.0 | BSD |
| tiktoken | 0.7.0 | MIT |
| more-itertools | 8.10.0 | MIT |
| tqdm | 4.66.4 | MPL-2.0 AND MIT |
| certifi | 2024.6.2 | MPL-2.0 |
| charset-normalizer | 3.3.2 | MIT |
| filelock | 3.13.1 | Unlicense |
| fsspec | 2024.2.0 | BSD |
| idna | 3.7 | BSD |
| Jinja2 | 3.1.3 | BSD-3-Clause |
| MarkupSafe | 2.1.5 | BSD-3-Clause |
| mpmath | 1.3.0 | BSD |
| networkx | 3.2.1 | BSD |
| packaging | 24.1 | Apache OR BSD |
| PyYAML | 5.4.1 | MIT |
| regex | 2024.5.15 | Apache |
| requests | 2.32.3 | Apache-2.0 |
| sympy | 1.12 | BSD |
| triton | 2.3.1 | MIT |
| typing_extensions | 4.9.0 | PSF |
| urllib3 | 2.2.1 | MIT |
| nvidia-cublas-cu12 | 12.1.3.1 | NVIDIA Proprietary Software |
| nvidia-cuda-cupti-cu12 | 12.1.105 | NVIDIA Proprietary Software |
| nvidia-cuda-nvrtc-cu12 | 12.1.105 | NVIDIA Proprietary Software |
| nvidia-cuda-runtime-cu12 | 12.1.105 | NVIDIA Proprietary Software |
| nvidia-cudnn-cu12 | 8.9.2.26 | NVIDIA Proprietary Software |
| nvidia-cufft-cu12 | 11.0.2.54 | NVIDIA Proprietary Software |
| nvidia-curand-cu12 | 10.3.2.106 | NVIDIA Proprietary Software |
| nvidia-cusolver-cu12 | 11.4.5.107 | NVIDIA Proprietary Software |
| nvidia-cusparse-cu12 | 12.1.0.106 | NVIDIA Proprietary Software |
| nvidia-nccl-cu12 | 2.20.5 | NVIDIA Proprietary Software |
| nvidia-nvjitlink-cu12 | 12.3.101 | NVIDIA Proprietary Software |
| nvidia-nvtx-cu12 | 12.1.105 | NVIDIA Proprietary Software |

Pinned models are content-checked before and after inference. Their supporting config, vocabulary and preprocessor file identities are also checked by the original adapter and included in the returned provenance.

| Model | Selected revision / weight file | Bytes | SHA-256 | Upstream reported license |
| --- | --- | ---: | --- | --- |
| Whisper small multilingual | `small.pt` | 483617219 | `9ecf779972d90ba49c06d968637d720dd632c55bbf19d441fb42bf17a411e794` | MIT |
| facebook/wav2vec2-base-960h | `22aad52d435eb6dbaf354bdad9b0da84ce7d6156`, `model.safetensors` | 377607901 | `8aa76ab2243c81747a1f832954586bc566090c83a0ac167df6f31f0fa917d74a` | Apache-2.0 |
| jonatasgrosman/wav2vec2-large-xlsr-53-greek | `489b34fb35fc5876af6193d419772cb9d6d1d531`, `pytorch_model.bin` | 1262101912 | `65f2b38c59498e261b55da3d81f1156c22531b798055174e62a4343bbc22951f` | Apache-2.0 |

English alignment uses the public safetensors loader. Greek uses `torch.load(weights_only=True)` only after matching the fixed weight-file identity; arbitrary model files are unsupported. Public interfaces and model terms: [Whisper](https://github.com/openai/whisper), [English model card](https://huggingface.co/facebook/wav2vec2-base-960h), [Greek model card](https://huggingface.co/jonatasgrosman/wav2vec2-large-xlsr-53-greek), [Torchaudio 2.3 forced alignment](https://docs.pytorch.org/audio/2.3.0/generated/torchaudio.functional.forced_align.html) and [Transformers 4.44.2 Wav2Vec2 interface](https://huggingface.co/docs/transformers/v4.44.2/en/model_doc/wav2vec2). Generated media, model cards, acquisition receipts and package inventories remain external.

The additional speech acceptance fixtures use original English/Greek passages, independent synthesis events and isolated-word PCM activity bounds. The installed **Microsoft Hazel Desktop** and **Microsoft Stefanos** voices, PowerShell **7.6.5**, and `System.Speech` assembly **10.0.0.5** generate these fixtures through public file-output APIs. Existing Windows components keep their own system licensing; PowerShell is MIT licensed. No voice or generated speech is redistributed. These fixture requirements are separate from the engine runtime and from the older dialogue-repair fixture environment recorded below.

## Original materials

| Material | Origin | Repository treatment |
| --- | --- | --- |
| Rust engine and CLI | Original implementation in this project | MIT source |
| Synthetic video/audio fixture recipe | Original color, motion, frame-counter and tone generation | MIT generator; generated media outside Git |
| Acceptance tests and progress criteria | Original project work | MIT source and factual evidence |
| Lower-third and title-card scene templates | Original layer layouts and parameter definitions | MIT JSON recipes; supplied fonts stay external |
| MIT license text | Standard MIT grant selected by the user | LICENSE; copyright notice names Cutbolt contributors |

The original master audio processor implements mathematical EQ equations from the [W3C Audio EQ Cookbook (8 June 2021)](https://www.w3.org/TR/2021/NOTE-audio-eq-cookbook-20210608/) and stereo integrated-loudness equations/coefficient values from [ITU-R BS.1770-5 Annex 1 (November 2023)](https://www.itu.int/rec/R-REC-BS.1770-5-202311-I). These are public algorithm references, not additional software dependencies; no third-party DSP implementation or document copy is included. Compressor behavior is project-defined. External FFmpeg filters/meters provide an independent test reference only; audio processing itself uses the Rust library.

The original [native track model](TRACKS.md) compiles explicit frame/sample placements through the existing external FFmpeg 7.0 build's public trim, concat, sample-delay and audio-mix interfaces. An independent Python Fraction/integer reference checks every decoded frame and PCM sample, including final saturation. Track editing, links, locks and collision policies are original Rust code. No additional library, implementation source or tool version is introduced.

Original [editable transitions](TRANSITIONS.md) extend that compiler with project-defined integer blend/wipe equations, sample-center clocks and signed rounding through the same build's public blend, audio-expression and channel-join interfaces. Public interface documentation is linked in the transition contract. Original motion, geometric still-card and stereo-signal generators provide independent acceptance inputs; no preset, implementation source, dependency version or license changed.

Original [audio routing](AUDIO_ROUTING.md) uses the existing Rust standard library and processing implementation, with public WAVEFORMATEXTENSIBLE speaker-mask and PCM-subtype facts linked in its contract. Its original reader/writer does not incorporate third-party implementation source. Independent Fraction/Decimal graph references, Windows process-memory counters and the already pinned external media filters verify output. No dependency or license version changed.

Original [dialogue repair](DIALOGUE_REPAIR.md) adds no Rust/runtime dependency. Its transform, overlap accumulator and suppression rules are original project code. The independent development reference selects the already installed **NumPy 1.26.4**, under its [BSD three-clause license](https://numpy.org/doc/1.26/license.html), for separate whole-buffer Fourier transforms and numeric comparisons. It is not copied or bundled. Install this exact test dependency into the external Python environment when needed: `python -m pip install numpy==1.26.4`.

Spoken quality fixtures use original project passages through the installed Windows `System.Speech` file-output API (assembly version **4.0.0.0**, recorded OS **10.0.26200.0**) with the local **Microsoft Hazel Desktop** and **Microsoft David Desktop** voices. These are existing Windows components under their own system licensing, not project redistributions or engine runtime requirements. Verification records selected voice names and generated-file hashes; all generated speech stays outside Git. The test environment must provide these voices; the suite does not install components or substitute a network service. The public [speech file-output interface](https://learn.microsoft.com/en-us/dotnet/api/system.speech.synthesis.speechsynthesizer.setoutputtowavefile) is used without copying implementation source. Other selected development tools and their licenses remain unchanged.

## Build dependencies

Original primary grading uses public sRGB transfer-function facts from [W3C CSS Color 4 section 10.2, draft of 30 September 2026](https://www.w3.org/TR/2026/CRD-css-color-4-20260930/#predefined-sRGB). Exposure/contrast/diagonal white balance, curve ordering and clipping are project-defined in [GRADING.md](GRADING.md). The numerical reference uses Python's standard-library Decimal and Fraction arithmetic plus the existing external original-font/image tools. No new dependency, copied implementation or specification file is introduced.

Original selective correction uses the same draft's [public HSL definitions](https://www.w3.org/TR/2026/CRD-css-color-4-20260930/#rgb-to-hsl), with project-defined integer quantization, band weights, correction masks and mixing. [SELECTIVE_COLOR.md](SELECTIVE_COLOR.md) records the precise contract. Its independent reference uses the same existing Python standard-library arithmetic and original external fixtures; no dependency versions or licenses changed.

Original [chroma keying and presets](KEYING.md) use project-defined opponent-channel distance, matte weighting, screen subtraction, spill caps and recipes. The reference combines the existing standard-library exact arithmetic with original synthetic foreground/screen mixtures. No external preset, implementation, plugin or dependency was added.

Original [SDR normalization](COLOR.md) applies public sRGB and BT.709 transfer/matrix facts in Rust using standard f64 arithmetic. Existing external FFmpeg only decodes/encodes the selected native samples; no external color processor or added package is selected. Its independent fixture uses 48-digit standard-library Decimal arithmetic and original native RGB/YUV charts. Exact public document versions and project-defined range/clipping policy are recorded in the color contract.

Rust/Cargo tested: 1.98.1, x86_64 Windows MSVC. `Cargo.lock` records exact dependency versions and checksums. Dependency source is stored by Cargo outside this repository, and compiled output is ignored under `target/`. There is no vendored source or binary distribution in this first start.

Original [LUT conversion and numerical scopes](LUTS_SCOPES.md) use project-owned Rust parsing, interpolation and population counting. Public LUT format/filter interfaces and BT.709 coefficient facts inform the documented contract. The existing external `lut1d`/`lut3d` filters provide a second acceptance reference alongside exact Python Fraction calculations; they are not used to apply LUTs in the engine. All tables and charts are original generated fixtures outside Git. No dependency version/license or bundled material changed. All engine FFV1 writers select version 1/one slice when either output dimension is below four pixels, otherwise version 3/four slices. This preserves tiny proxies while retaining large-frame support in the selected external encoder; [the HDR contract](HDR.md) records the dimension tests.

| Direct dependency | Locked version | Reported license |
| --- | --- | --- |
| serde | 1.0.229 | MIT OR Apache-2.0 |
| serde_json | 1.0.151 | MIT OR Apache-2.0 |
| sha2 | 0.10.9 | MIT OR Apache-2.0 |
| rusqlite (bundled and blob features) | 0.40.2 | MIT |
| schemars | 1.2.2 | MIT |
| png | 0.18.1 | MIT OR Apache-2.0 |
| hound | 3.5.1 | Apache-2.0 |
| fontdue (std only, default features disabled) | 0.9.4 | MIT OR Apache-2.0 OR Zlib |
| harfrust | 0.13.3 | MIT |
| unicode-bidi | 0.3.18 | MIT OR Apache-2.0 |
| unicode-segmentation | 1.13.3 | MIT OR Apache-2.0 |
| unicode-script | 0.5.8 | MIT OR Apache-2.0 |
| unicode-linebreak | 0.1.5 | Apache-2.0 |
| windows (Windows target only) | 0.62.2 | MIT OR Apache-2.0 |
| windows-core (Windows target only) | 0.62.2 | MIT OR Apache-2.0 |
| windows-sys (Windows target only) | 0.61.2 | MIT OR Apache-2.0 |

The 91-package lockfile includes the project and build/transitive/other-target dependencies. The tested Windows metadata includes 77 external packages. SQLite 3.53.2 is compiled through `libsqlite3-sys` 0.38.2 from Cargo's external package cache; neither the SQLite source nor compiled binary is vendored in this repository. The bundled build removes the need for a runtime SQLite installation and requires a C compiler when building. SQLite's embedded source declares public-domain status; the Rust wrapper has its own MIT license.

Text layers use [fontdue](https://github.com/mooman219/fontdue) 0.9.4 with scalar rasterization. Default scalar text omits shaping; the optional [Unicode layout](UNICODE_TEXT.md) enables indexed substitution glyph loading and uses external HarfRust 0.13.3 for shaping, unicode-bidi 0.3.18 for paragraph ordering, unicode-segmentation 1.13.3 for graphemes, unicode-script 0.5.8 for script properties and unicode-linebreak 0.1.5 for break opportunities. Original Cutbolt code selects fonts, combines visual/script/font runs, wraps and positions lines, enforces work bounds and composites coverage. The independent acceptance generator authors its own rectangle fonts and GSUB/GPOS facts; no selected shaper is used by its pixel oracle. Fonts remain content-checked external inputs, with no system discovery, bundled typeface or runtime download. Exact versions/licenses and their parser/math dependencies are recorded here; no dependency implementation is vendored.

The original local cache also selects rusqlite's `blob` feature for incremental payload I/O. It adds no new crate or version. Cache ownership checks use the public [SQLite database header format](https://www.sqlite.org/fileformat.html), including application ID and user version, before opening an existing file for recovery. Cache keys and content checks use the existing SHA-256 dependency. The development build optimizes sha2 0.10.9 to retain full content verification without dominating preview latency; no algorithm or integrity check is removed. The original build script embeds a producer fingerprint from project source, dependency selection and declared compiler configuration. See [cache limits and tests](CACHE_PREVIEWS.md).

Session transactions use rollback journaling (DELETE), `synchronous=EXTRA`, and a five-second lock timeout. [SQLite atomic commits](https://www.sqlite.org/atomiccommit.html) and [synchronous settings](https://www.sqlite.org/pragma.html#pragma_synchronous) describe the storage assumptions. Our verification covers injected process exits, not hardware/power-loss guarantees.

Before distributing a compiled binary, assemble required notices for the exact resolved graph and target. The following Cargo-reported inventory records selection; it does not replace dependency license texts. Versions for other targets remain in `Cargo.lock`.

| Tested Windows dependency | Exact version | Reported license |
| --- | --- | --- |
| adler2 | 2.0.1 | 0BSD OR MIT OR Apache-2.0 |
| bitflags | 2.13.2 | MIT OR Apache-2.0 |
| block-buffer | 0.10.4 | MIT OR Apache-2.0 |
| bytemuck | 1.25.2 | Zlib OR Apache-2.0 OR MIT |
| bytemuck_derive | 1.12.1 | Zlib OR Apache-2.0 OR MIT |
| cc | 1.5.1 | MIT OR Apache-2.0 |
| cfg-if | 1.0.5 | MIT OR Apache-2.0 |
| core_maths | 0.1.1 | MIT |
| cpufeatures | 0.2.17 | MIT OR Apache-2.0 |
| crc32fast | 1.5.2 | MIT OR Apache-2.0 |
| crypto-common | 0.1.7 | MIT OR Apache-2.0 |
| digest | 0.10.7 | MIT OR Apache-2.0 |
| dyn-clone | 1.0.20 | MIT OR Apache-2.0 |
| fallible-iterator | 0.3.0 | MIT/Apache-2.0 |
| fallible-streaming-iterator | 0.1.9 | MIT/Apache-2.0 |
| fdeflate | 0.3.7 | MIT OR Apache-2.0 |
| find-msvc-tools | 0.1.14 | MIT OR Apache-2.0 |
| flate2 | 1.1.10 | MIT OR Apache-2.0 |
| foldhash | 0.2.0 | Zlib |
| font-types | 0.12.6 | MIT OR Apache-2.0 |
| fontdue | 0.9.4 | MIT OR Apache-2.0 OR Zlib |
| generic-array | 0.14.7 | MIT |
| harfrust | 0.13.3 | MIT |
| hashbrown | 0.17.1 | MIT OR Apache-2.0 |
| hashlink | 0.12.2 | MIT OR Apache-2.0 |
| hound | 3.5.1 | Apache-2.0 |
| itoa | 1.0.18 | MIT OR Apache-2.0 |
| libm | 0.2.16 | MIT |
| libsqlite3-sys | 0.38.2 | MIT |
| memchr | 2.8.3 | Unlicense OR MIT |
| miniz_oxide | 0.8.9 | MIT OR Zlib OR Apache-2.0 |
| miniz_oxide | 0.9.1 | MIT OR Zlib OR Apache-2.0 |
| once_cell | 1.21.4 | MIT OR Apache-2.0 |
| pkg-config | 0.3.34 | MIT OR Apache-2.0 |
| png | 0.18.1 | MIT OR Apache-2.0 |
| proc-macro2 | 1.0.107 | MIT OR Apache-2.0 |
| quote | 1.0.47 | MIT OR Apache-2.0 |
| read-fonts | 0.43.3 | MIT OR Apache-2.0 |
| ref-cast | 1.0.27 | MIT OR Apache-2.0 |
| ref-cast-impl | 1.0.27 | MIT OR Apache-2.0 |
| rusqlite | 0.40.2 | MIT |
| schemars | 1.2.2 | MIT |
| schemars_derive | 1.2.2 | MIT |
| serde | 1.0.229 | MIT OR Apache-2.0 |
| serde_core | 1.0.229 | MIT OR Apache-2.0 |
| serde_derive | 1.0.229 | MIT OR Apache-2.0 |
| serde_derive_internals | 0.30.0 | MIT OR Apache-2.0 |
| serde_json | 1.0.151 | MIT OR Apache-2.0 |
| sha2 | 0.10.9 | MIT OR Apache-2.0 |
| shlex | 2.0.1 | MIT OR Apache-2.0 |
| simd-adler32 | 0.3.10 | MIT |
| smallvec | 1.16.2 | MIT OR Apache-2.0 |
| syn | 2.0.119 | MIT OR Apache-2.0 |
| syn | 3.0.6 | MIT OR Apache-2.0 |
| ttf-parser | 0.25.1 | MIT OR Apache-2.0 |
| typenum | 1.20.1 | MIT OR Apache-2.0 |
| unicode-bidi | 0.3.18 | MIT OR Apache-2.0 |
| unicode-ident | 1.0.26 | (MIT OR Apache-2.0) AND Unicode-3.0 |
| unicode-linebreak | 0.1.5 | Apache-2.0 |
| unicode-script | 0.5.8 | MIT OR Apache-2.0 |
| unicode-segmentation | 1.13.3 | MIT OR Apache-2.0 |
| vcpkg | 0.2.15 | MIT/Apache-2.0 |
| version_check | 0.9.5 | MIT/Apache-2.0 |
| windows | 0.62.2 | MIT OR Apache-2.0 |
| windows-collections | 0.3.2 | MIT OR Apache-2.0 |
| windows-core | 0.62.2 | MIT OR Apache-2.0 |
| windows-future | 0.3.2 | MIT OR Apache-2.0 |
| windows-implement | 0.60.2 | MIT OR Apache-2.0 |
| windows-interface | 0.59.3 | MIT OR Apache-2.0 |
| windows-link | 0.2.1 | MIT OR Apache-2.0 |
| windows-numerics | 0.3.1 | MIT OR Apache-2.0 |
| windows-result | 0.4.1 | MIT OR Apache-2.0 |
| windows-strings | 0.5.1 | MIT OR Apache-2.0 |
| windows-sys | 0.61.2 | MIT OR Apache-2.0 |
| windows-threading | 0.2.1 | MIT OR Apache-2.0 |
| zlib-rs | 0.6.8 | Zlib |
| zmij | 1.0.23 | MIT |


Original fixture generation uses Python's standard library and external tools. The v0.4 scene tests and optional PixelForge adapter additionally use Pillow 10.3.0 (HPND) for independent PNG decoding and forward image transforms. v0.3 verification also uses the existing external `jsonschema` 4.25.0 (MIT) for schema checking and official `mcp` 1.12.3 (MIT) for independent client interoperability. These are development-only: the native stdio adapter needs neither Python nor an SDK at runtime. Transitive packages retain their own licenses and are not vendored.

Graphics verification additionally uses external FontTools 4.25.0 (MIT) to generate original abstract rectangle glyphs into external TrueType fixtures. No existing font or font implementation is copied into this repository. The pixel reference uses analytic rectangle coverage and known original advances; it does not call the engine's rasterizer.

Install these exact direct development versions into an external environment when needed: `python -m pip install jsonschema==4.25.0 mcp==1.12.3 Pillow==10.3.0 fonttools==4.25.0`. The original controlled media-tool fixture compiles with rustc into an external test directory; its executable is never committed. The optional, explicit model-setup helper described below additionally uses the separately installed `huggingface-hub` package; it is never called by the engine or verification suite.


The local recording backend selects **windows 0.62.2** and **windows-core 0.62.2** (MIT OR Apache-2.0) for typed COM/audio bindings and callback support. These remain in Cargo’s external package cache. Original project code implements input selection, packet accounting, streaming WAV output and placement; it uses the installed Windows shared audio engine for explicitly requested format conversion. Public [audio initialization](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudioclient-initialize), [packet capture](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudiocaptureclient-getbuffer) and [process-specific activation](https://learn.microsoft.com/en-us/windows/win32/api/mmdeviceapi/nf-mmdeviceapi-activateaudiointerfaceasync) describe the called interfaces. The original playback fixture uses its own nonpersistent audio session. No driver, SDK implementation, binary or generated recording is vendored.

## External media tools

The original [caption parser/editor](CAPTIONS.md) adds no dependencies. Its bounded interchange profile uses the public [WebVTT draft dated 20 May 2026](https://www.w3.org/TR/2026/CRD-webvtt1-20260520/) and [Library of Congress SRT description](https://www.loc.gov/preservation/digital/formats/fdd/fdd000569.shtml). Tests use the existing external FFprobe to inspect subtitle packet timestamps/text and the existing original-font pixel reference. No parser source, document copies or external subtitle fixtures are vendored.

Existing local paths: `C:\ffmpeg\bin\ffmpeg.exe` and `C:\ffmpeg\bin\ffprobe.exe`. Tested build: `7.0-full_build-www.gyan.dev`. Its reported configuration enables GPL and version 3 components; `ffmpeg -L` reports GPL-3.0-or-later for this exact executable. Cutbolt invokes these existing executables as local subprocesses; it does not copy, bundle, modify, or redistribute them in this repository. Keep any future packaged-tool licensing review separate from Cutbolt's MIT grant. [FFmpeg license guidance](https://ffmpeg.org/legal.html)

The reference renderer uses FFV1 and PCM. The bounded [media-conform matrix](CONFORM.md) additionally selects this same build's H.264/AAC decoders for MP4/MOV inputs, with explicit color, timestamp and audio constraints. Original acceptance fixtures use its external libx264 encoder and native AAC encoder; neither is bundled. The fixed [delivery profile](EXPORT.md) also selects libx264 and native AAC for engine exports, using `lutrgb` for original public transfer expressions and `scale` for declared BT.709 matrix/range/chroma conversion. The recorded local build has libavcodec 61.3.100, libavformat 61.1.100, libavfilter 10.1.100 and libswscale 8.1.100; its emitted encoder identity is x264 core 164 r3190 7ed753b. These components remain within the separately installed GPL/version-3-enabled build and are not redistributed. Public interfaces are described in the [FFmpeg codec](https://ffmpeg.org/ffmpeg-codecs.html) and [filter](https://ffmpeg.org/ffmpeg-filters.html) documentation. Public sRGB and BT.709 facts and the independent numerical reference are cited in the export contract; no implementation or specification copy is included.

Other formats/encoders present in the build remain outside the declared engine contract. No dependency version changed for these additions.

The [high-bit-depth/HDR path](HDR.md) selects the same external build's FFV1 native planar RGB/YUV444 10/12/16-bit decoding and RGB16 encoding. Transfer, gamut, exposure and tone calculations are original Rust using public equations; the engine writes standard declared display metadata into reserved space in its own temporary Matroska output. The acceptance fixture also uses this existing build's `zscale` filter as an external PQ-to-linear numerical reference. No additional package, binary, source, specification copy or fixture asset is vendored, and the selected build/version/license is unchanged. Public references and the comparison tolerance are in the HDR contract.

## External compatibility pilot assets and tools

Selected/inspected 2 October 2026. These do not become Rust runtime dependencies. See the [agent handoff](YOUTUBE_PIPELINE.md) for local setup and the distinction between prepared files and untested integration.

| Item | Exact identity | Reported license / status |
| --- | --- | --- |
| PixelForge | `pixelforge-agent` 0.1.0, commit `911d4fa167bdea5949669c20679693257e3cc170` | MIT; external checkout at `C:\DEV\PixelForge`, interface inspected, integration not tested |
| Qwen Base weights/configuration | `Qwen/Qwen3-TTS-12Hz-0.6B-Base` revision `5d83992436eae1d760afd27aff78a71d676296fc` | Model card reports Apache-2.0; snapshot includes speech tokenizer; inference not tested |
| Hugging Face download client | Installed `huggingface-hub` 0.23.4 | Apache-2.0; used only for explicit setup in `tools/download_qwen.py` |
| Qwen inference environment | Not selected/installed by this preparation | Pin Python, qwen-tts, torch/CUDA and transitive versions/licenses before testing; do not assume the system Python inventory is compatible |
| PixelForge (later observation, 4 October 2026) | `pixelforge-agent` 0.7.0, commit `d1dcb2a9d1aaadeb86663e997efc050a71881d44` | MIT; worked unchanged with the handoff helper in an external demonstration; not an acceptance result |
| Qwen CustomVoice weights/configuration | `Qwen/Qwen3-TTS-12Hz-0.6B-CustomVoice` revision `85e237c12c027371202489a0ec509ded67b5e4b5` | Model card reports Apache-2.0; 13 files verified by `tools/download_qwen.py --repository`; preset speakers, no reference voice |
| Observed Qwen inference environment (WSL, external) | `qwen-tts` 0.1.1, `transformers` 4.57.3, `accelerate` 1.12.0, `torchaudio` 2.3.1+cu121, `numpy` 1.26.4, `torch` 2.3.1+cu121 | Reported Apache-2.0, Apache-2.0, Apache-2.0, BSD, BSD and BSD-3; used only for an offline demonstration outside the engine |

The external model directory is `C:\DEV\CutboltData\models\Qwen3-TTS-12Hz-0.6B-Base\5d83992436eae1d760afd27aff78a71d676296fc`. All 13 upstream snapshot files total 2,516,106,051 bytes; each was checked against upstream size and LFS SHA-256 or Git blob SHA-1, and the external `download-receipt.json` records SHA-256 values for every file. The main model SHA-256 is `180b3b10eb1c9f1b4db7806d5475bae3071c0243c299d49926bab1da3b6946f6`; bundled speech-tokenizer weights are `836b7b357f5ea43e889936a3709af68dfe3751881acefe4ecf0dbd30ba571258`.

The separately published `Qwen/Qwen3-TTS-Tokenizer-12Hz` revision `7dd38ad4e9bad454aae9cd937d0cd577604fe229` advertises that same tokenizer weight digest. Its repository was inspected for identity, not downloaded as a second copy. Prefer the tokenizer already bundled in the pinned Base snapshot.

No third-party source, weights, media or package files were added to this repository. New production brief, contracts and download helper are original project materials. Model/license references: [pinned Base model card](https://huggingface.co/Qwen/Qwen3-TTS-12Hz-0.6B-Base/blob/5d83992436eae1d760afd27aff78a71d676296fc/README.md), [Qwen official repository](https://github.com/QwenLM/Qwen3-TTS), [Hugging Face client license at v0.23.4](https://github.com/huggingface/huggingface_hub/blob/v0.23.4/LICENSE).

## Original historical store reference

The [portable-history fixture](PORTABLE_PROJECTS.md) retains a project-owned MIT Cutbolt v0.4.0 Windows binary that writes editing-store schema 1, before the explicit schema-2 migration. Selected 4 October 2026: 25,174,528 bytes, SHA-256 `6a5626b89383cc4d6c6408cbd8412694a81bda26f7ff9de6a81044a8b0965d04`. It remains outside the repository under `C:\DEV\CutboltData\portable-preparation-20261004\legacy-engine`; source location and hash are recorded externally. It generates original history fixtures for migration acceptance and is not a runtime or third-party dependency. Full verification receives its path through `CUTBOLT_LEGACY_STORE_ENGINE`, verifies the produced store version and records its actual content identity. No dependency version, license or Rust feature changed. Consistent backups use the existing bundled SQLite public [VACUUM INTO statement](https://www.sqlite.org/lang_vacuum.html); original validation runs before publication.

## External native-project source application

The optional [native-project transfer](NATIVE_PROJECTS.md) uses an already installed, separately licensed source application through its public scripting interface. Exact version/build, executable hash, access basis and original external adapter hashes are recorded in the private acceptance fixture; application-specific material is not distributed here. The engine acquires no new runtime dependency and executes no adapter. The generic transfer uses the existing public OTIO data contract. Full verification rechecks that selected installation and replays three recorded captures; it does not freshly launch the application or generalize compatibility to other versions. Original private adapter code contains no decompiled implementation or copied SDK source.

## Optional foreground-mask worker

Selected 4 October 2026 for the [annotation workflow](SEGMENTATION.md): external CPython **3.12.14** (Python Software Foundation license), **opencv-python-headless 4.13.0.92** (MIT packaging wrapper, OpenCV 4.13.0 under Apache-2.0, external third-party wheel notices) and **NumPy 2.2.6** (BSD-3-Clause with external bundled-component notices). The package's MIT wrapper text and third-party notice files were checked in the installed external wheel; its metadata also reports the underlying Apache license. These packages remain in a separate external environment, not the ordinary test Python or the Rust dependency graph. No Rust dependency, plugin, binary, model or third-party source is vendored.

The worker calls the public CPU GrabCut interface with explicit user annotations, fresh zero-initialized model arrays, one thread, fixed per-frame seeds and OpenCL disabled. No pretrained weights are selected. Each actual foreground/background GMM and its digest are retained in the external mask document. Receipts fingerprint installed package files, the original helper, Python executable/base runtime and selected execution settings. The selected NumPy wheel carries its own OpenBLAS/LAPACK/compiler-runtime notices; the OpenCV wheel likewise carries component notices. Keep these original notices with any separately reviewed runtime distribution; this project does not redistribute the wheels or extend its MIT grant to them.

Public references: [pinned OpenCV Python wheel](https://pypi.org/project/opencv-python-headless/4.13.0.92/), [OpenCV license](https://opencv.org/license/), [NumPy 2.2.6](https://pypi.org/project/numpy/2.2.6/), [Python license](https://docs.python.org/3/license.html). Full verification uses `CUTBOLT_SEGMENTATION_PYTHON` to select the external interpreter and requires the declared exact package versions; it never installs them automatically.
