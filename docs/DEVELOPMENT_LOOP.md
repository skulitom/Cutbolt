# Development loop audit and plan

Audit of the edit → build → test → verify loop, 4 October 2026. Every number below was measured on the development machine: 32 logical processors, 64 GB RAM, a Samsung 990 PRO NVMe with NTFS, Defender real-time protection on, and an RTX 4090. Debug build of commit `d714096` unless stated.

**Goal:** turn a typical change from "edit, then wait one to two hours" into "edit, check in under a minute, commit after a five-minute check". The thorough evidence run stays intact for milestones.

## Status

- **Phase 0 is done.**
  - `tools/verify.py` defaults to the quick check (61/61 fixtures in 16.4 minutes); `--thorough` is the evidence run.
  - `--only`, `--last-failed`, `--fail-fast` and `--strict` are available, with a preflight of tools and runtimes.
  - Fixtures run a copied engine, and budgets go through `tests/budgets.py`.
- **The push gate and background verification (section 4, phase 0 item 2, extended) are done.**
  - `tools/impact.py` builds a function-level coverage map of all fixtures.
  - `tools/verify.py --gate` runs only the fixtures the diff can affect, within about two minutes, beside lint and Rust tests, and reuses passes whose inputs are unchanged.
  - `tools/ship.py` gates, pushes, and verifies the deferred remainder in an isolated background worktree that blocks no one.
  - Map, run records and timings are shared by all worktrees.
- **Shipping takes about 100–130 s, measured over eight real ships** (pushed by `tools/ship.py`, including under heavy load). Lessons built into it:
  - The gate budget is a hard deadline. Predictions fail under contention: one early gate took six minutes.
  - Background runs use four lanes, start nothing while a gate runs, and skip fixtures that a newer run of a descendant commit will verify.
  - Ships take a shared lock, then fetch, rebase, gate and push inside it. Concurrent sessions otherwise kept racing at the push.
  - `docs/PROGRESS_HISTORY.md` union-merges.
  - The real-time capture fixture may retry once outside thorough runs.
  - Background verification uses a hidden shared console and a shared build directory.
- **Phase 1a is done.** One packet listing replaces three `ffprobe` runs per source inspection (−24% engine launches on `tracks`).
- **Next:**
  - Rebuild the impact map regularly. It is diffed from its own commit, so selection widens as main moves.
  - Phase 1b/1c: packet timing where pictures are not read (item 4). The content-addressed inspection cache (item 3) exists: a passed inspection is keyed by the source's SHA-256 and size, its parameters, the ffprobe executable's SHA-256 and the engine build. Entries live in the process and, with a workspace, in `.cutbolt/cache/inspections` (or the directory `CUTBOLT_INSPECTION_CACHE` names), so the strict decode runs once per file rather than once per command. Every command still hashes its sources. A pass also records the inspections it implies (alpha acceptance of an opaque file, packet timing that agrees with the decode); sources that only feed audio tracks are packet-timed; and a command inspects its sources up to four at a time.
  - Phases 2–3.

## 1. Summary

The loop is slow for five measured reasons. In order of impact:

1. **Process launches, not computation, dominate fixture time.** A fixture launches 450–1,900 processes. About three quarters of the engine's launches repeat one three-call `ffprobe` inspection of sources that have not changed, and one of those calls decodes every frame. The machine creates at most about 100 processes per second however many run in parallel, so a full verification (about 59,000 launches) has a floor of roughly 10 minutes before any real work. It is also why parallel fixtures slow each other down: `tracks` takes 29 s alone, 88 s with eight neighbours and 241 s with sixteen.
2. **Every check runs the full acceptance suite.** Until now there was one mode: every fixture, long-form durations, wall-clock budgets on a quiet machine, and evidence generation. It took 103 minutes serially and 45–62 minutes after today's parallel scheduler. Nothing smaller existed between `cargo test` and that.
3. **Verification blocks development.** Fixtures run `target/debug/cutbolt.exe`, which Windows locks, so nobody can rebuild during a run. Any edit to `src/`, `tests/` or `tools/` during a run makes it stale. Several sessions share one working tree. Today one session waited an hour, and one run had to be discarded.
4. **Some fixture oracles are slow single-threaded Python.** Two fixtures spend 45 s and 137 s in per-pixel `Fraction`/`Decimal` arithmetic that could be cached or vectorized exactly.
5. **The engine itself is slower than it needs to be:**
   - Debug code generation halves scene rendering speed.
   - FFV1 is encoded with 4 slices at every size, which caps a 4K encode at about 26 fps.
   - Previews and renders decode every source frame just to read timestamps.

Fixing these in the order of section 4 should reduce:

| Loop | Today | After phase 0 | After phases 1–3 |
| --- | --- | --- | --- |
| Edit → build | 7 s | 7 s | 7 s (13 s with an optimized profile, or keep it for the verifier only) |
| Targeted check of the touched area | ad hoc, often skipped | `verify.py --only …`, 0.5–3 min | 10–60 s |
| Pre-commit check | none between `cargo test` and the full run | `verify.py` (quick), about 10–15 min | about 3–5 min |
| Thorough evidence run | 103 min serial, 45–62 min parallel | unchanged, run at milestones or nightly | about 25–35 min |

The acceptance criteria, exactness and evidence rules stay as they are. Only the quick tier relaxes anything, and only wall-clock budgets, which it records but does not enforce.

## 2. Measurements

### 2.1 Thorough verifier

| Run | Mode | Wall time | Result |
| --- | --- | --- | --- |
| Previous evidence run | serial | 6,164 s (103 min) | passed |
| Run 1 (today) | serial | stopped at 67 min | hardware-decode fixture regression, fixed |
| Run 2 | parallel, 16 jobs | 42 min | 4 failures (missing `pwsh`, capture under load, two harness issues), fixed |
| Run 3 | parallel | 62 min | 1 failure: registry's 5 s edit budget hit 5.03 s beside the 4K render |
| Run 4 | parallel, quiet final phase | 45.5 min | 1 failure: 15-minute real-time capture hit a 10 ms gap |

Stage wall times, run 3 (parallel; times in the pool are inflated by contention):

| Stage | Seconds | Stage | Seconds |
| --- | ---: | --- | ---: |
| long_form_4k | 2,040 | color | 353 |
| recording (sustained capture) | 930 | transitions | 327 |
| export_formats | 518 | native_timing (long form) | 308 |
| native_scenes | 405 | delivery_profiles | 295 |
| transcription | 382 | track_edits | 290 |
| hdr | 363 | sequences | 271 |
| delivery | 355 | acceleration | 258 |

The pool phase (47 correctness fixtures) finished in about 10 minutes. The critical path is the 4K lane (about 33 minutes) plus the real-time capture (15.5 minutes). Stage durations sum to 12,316 s.

### 2.2 Where fixture time goes

Thirty correctness fixtures were profiled, eight at a time, with a Windows job object, which gives exact CPU time across all child processes.

- **Totals:** 2,886 s of wall time, 1,630 s of CPU, and **28,965 processes**, about 965 per fixture.
- **Short-lived tool processes:** CPU sampled in long-running processes covers well under half of the total; the rest is spent in processes too short to sample.
- **Python oracles dominate CPU in about ten fixtures:**

  | Fixture | Python CPU (s) |
  | --- | ---: |
  | color | 53 |
  | compositing | 43 |
  | stabilization | 38 |
  | remapping | 37 |
  | hdr | 27 |
  | tracking | 24 |
  | keying | 21 |

- **FFmpeg encoding dominates only two fixtures:** `export_formats` (100 s) and `delivery_profiles` (43 s).

A logging shim on `CUTBOLT_FFPROBE`/`CUTBOLT_FFMPEG` recorded every tool launch the engine made:

| Fixture | Engine tool launches | Inspection triple: `ffprobe` streams + all video frames + all audio frames | `-version` calls | Renders/encodes |
| --- | ---: | ---: | ---: | ---: |
| tracks | 454 | 334 (74%) | 55 | 27 |
| delivery | 732 | 470 (64%) | 105 | 53 |
| transitions | 709 | 535 (75%) | 77 | 38 |

Each launch costs about 40 ms on an idle machine, almost all of it process startup for these small files. The same few unchanged sources are re-inspected 100–180 times per fixture.

### 2.3 Process creation is a machine-wide bottleneck

`ffprobe -version` launched from parallel threads:

| Parallel launchers | Processes per second | Latency per launch |
| ---: | ---: | ---: |
| 1 | 32 | 32 ms |
| 4 | 107 | 37 ms |
| 8 | 98 | 82 ms |
| 16 | 87 | 184 ms |
| 32 | 80 | 401 ms |

Throughput peaks at about four launchers and then falls. The likely causes are kernel process-creation locks plus Defender's on-access scan of each launched image. This is why adding lanes beyond about four hardly speeds up the pool. In the 16-job run, `tracks` took 8× its standalone time while total CPU stayed near 30%.

### 2.4 Build loop

Measured in an isolated copy:

| Step | Time |
| --- | ---: |
| Incremental `cargo build` after an edit (core or leaf module) | 7.1 s |
| Same with `rust-lld` | 7.1 s (linking is not the bottleneck) |
| `cargo clippy --all-targets` after an edit | 13 s |
| `cargo test` build + run after an edit | 20 s (78–84 tests run in about 1.5 s) |
| No-op build | 0.2 s |
| Pre-commit material check | 2.8 s |
| Optimized dev profile (crate opt 1, dependencies opt 3): full build | 102 s |
| Optimized dev profile: incremental build | 13.0 s |

### 2.5 Engine hot spots

- **Optimized code generation.** The native 1080p 225-frame scene renders in 26–28 s with the debug build and 13.1–13.4 s with the optimized dev profile. The decoded outputs are bit-identical (same MD5 over all decoded streams). Debug assertions and overflow checks stay on in both.
- **FFV1 slice count.** At 3840×2160, encoding runs at 25.9 fps with the engine's fixed 4 slices, 60.0 fps with 16 and 64.9 fps with 24. Decoded output is identical. The 30-minute 4K render takes about 20 minutes at roughly 21% CPU.
- **Source inspection decodes every frame.** `ffprobe -show_frames` on a 30-second 1080p FFV1 source takes 6.0 s; packet timestamps take 0.09 s. Every render, preview, export, scopes call and track input does this per source, so a single-frame preview of a 1080p, 30-second source waits about 6 s before any work. Audio-only exports now use packet timing (1.5 s instead of 26 s for a 51-second 1080p timeline).
- **Engine process start** costs 24 ms per trivial command, so a persistent engine process would help little.

### 2.6 Fixture oracles (cProfile)

- `color`: 98.6 s total. Of that, 45.4 s is 240,074 calls each to two pure per-value transfer functions (`encode`/`decode`, about 95 µs per call). Their inputs are 8- or 16-bit values, so caching the results gives identical output. Another 46 s is spent waiting on child-process pipes.
- `compositing`: 153.6 s total. 137 s is `expected_frame`, which makes 550 million Python calls, mostly `fractions.Fraction` construction and arithmetic per pixel.
- `native_scenes`: the independent spatial oracle takes 144 s for one 1080p frame on one core.

### 2.7 Flaky and blocking factors

- **Real-time capture flakes.** The sustained capture failed in 2 of 3 runs: once under load, and once on a quiet machine 96 s in, with a 10 ms packet gap. The engine correctly rejects such gaps, but a 15-minute real-time gate on a busy desktop is flaky by nature.
- **Tight budgets fail under neighbouring load.** The registry's 1,000-edit budget is 5 s, and it takes about 3.0–3.5 s on an idle machine. The same benchmark gives 2.99 s for the previous commit and 2.94 s for this one, so this is load, not regression.
- **Verification blocks development:**
  - The Windows executable lock blocks rebuilds during runs.
  - Fingerprinted edits during a run make it stale.
  - Up to four sessions edited one working tree today.
  - Runs 1–3 each surfaced failures only at the end, because the old verifier printed nothing until it finished.
- **Missing external tools fail late.** `pwsh` 7.6.5 had disappeared from the machine and was only discovered 6 minutes into a run.

## 3. Principles

- **Evidence integrity is non-negotiable.** Progress points come only from a thorough run on one source fingerprint, with every budget enforced and every long-form case complete.
- **Correctness checks are never relaxed.** Exact pixels, samples, clocks, rejections and memory limits are asserted in every tier. Only wall-clock budgets, which measure the machine as much as the code, become "recorded" in the quick tier.
- **Commits do not require a thorough run.** A commit may carry stale evidence if its history entry says so; the next thorough run refreshes it. This matches existing practice.
- **Faster engine code must not change results.** Any speed-up in the engine must keep decoded output, receipts' exact values and error behaviour identical. Prove that with the existing fixtures, not by assertion.

## 4. Plan

### Phase 0: verification tiers and unblocking (tooling only; in progress)

1. **Quick mode becomes the default, and `--thorough` reproduces today's run.**
   - Quick runs every correctness fixture in one memory-aware pool, using the existing short modes of the long-form fixtures and a 30-second capture at the end.
   - Wall-clock budgets go through one shared helper (`tests/budgets.py`). It enforces them by default, and quick mode sets it to record only.
   - Quick writes no evidence. Fixtures whose external runtime is not configured are reported as skipped, not failed, unless `--strict` is given.
   - `--thorough` keeps all current behaviour and is the only mode that writes `verification/latest.json` and regenerates progress.
2. **Targeted runs.**
   - `--only a,b` runs the named fixtures.
   - `--last-failed` reruns failures recorded in a git-ignored `verification/last-run.json`.
   - `--changed` maps files changed since a base commit to fixtures, through a small manifest. It falls back to everything when core modules (`media`, `render`, `scene`, `model`, `time`, `commands`) change.
3. **Never lock the developer's binary.** The verifier copies the built engine to a run-specific directory and passes `CUTBOLT_EXE` to fixtures. About 61 fixtures hardcode `target/debug/...`; one shared `tests/engine.py` helper replaces those references.
4. **Report early and record timing.**
   - Failures already print as soon as they happen, and every run records stage times.
   - The longest-first order should use the last run's times even when that run failed.
   - Add `--fail-fast` for interactive use.
5. **Preflight external tools in seconds.** Check `pwsh`, the OTIO/segmentation runtimes, the CUDA device and FFmpeg/ffprobe versions before starting, so a missing tool fails in seconds rather than an hour into a run.

Expected result: a 10–15 minute pre-commit check, 0.5–3 minute targeted checks, and no blocked rebuilds.

### Phase 1: cut process launches (engine; the largest single gain)

1. **One `ffprobe` call per source.** Merge stream/format metadata, video frame timestamps and audio frame sample counts into a single invocation. This cuts the inspection triple from 3 launches to 1.
2. **Cache tool identity.** Read `ffmpeg -version`/`ffprobe -version` once per process, and persist the result keyed by the tool's path, size, modification time and a hash computed once. This removes 55–105 launches per fixture and about 80 ms per command.
3. **Content-addressed validated-probe cache.** Key validated inspection results by source SHA-256, byte count and tool identity. The engine still hashes the source on every command, so changed bytes can never reuse an entry. The `cache_store` module already provides integrity-checked storage. Make the cache opt-in by an explicit cache root, or a workspace default, so the source-protection contract stays visible. Repeated inspections become free.
4. **Timestamp validation without full decodes** wherever pictures are not read: previews of other frames, plans, scopes and track inputs outside the rendered range. Full decode validation remains wherever frames are actually decoded, and audio-only exports already work this way. A 1080p 30-second preview drops from about 6 s of probing to under 0.5 s.
5. **Fixture-side launches.** Fixtures decode outputs with FFmpeg once per comparison. Keep one decode per output (most already do), and replace per-case `ffprobe` metadata calls with the engine's receipts where the receipt already carries the value.

Expected result: 70–85% fewer engine launches per fixture and 2–3× faster standalone fixtures. Parallel scaling also improves sharply, because the 100 processes/s ceiling stops binding. Agents see the same gain on every preview and render.

### Phase 2: faster oracles (fixtures; exactness preserved)

1. **Cache pure per-value functions.** Transfer functions in `color`, `hdr`, `grading`, `delivery` and `keying` get `functools.lru_cache`, or 256/65,536-entry tables. Results are identical; `color` saves about 45 s.
2. **Exact vectorization.** Replace per-pixel `Fraction` loops with integer NumPy using scaled integers or common denominators, keeping the documented rounding. Targets: `compositing`, `stabilization`, `remapping`, `tracking`, `keying`, `spatial` and the `native_scenes` spatial check. Expect 10–100× on those oracles.
3. **Parallel oracle frames.** Use `concurrent.futures.ProcessPoolExecutor` for independent frames, for example the 144 s spatial frame split by row bands.
4. **Content-addressed fixture caches.** Store generated synthetic sources and oracle outputs under a user-level cache directory, keyed by a hash of the generating code and its parameters. Never cache anything produced by the engine under test.
5. **Split the longest fixtures** (`export_formats`, `hdr`, `color`, `delivery`, `transitions`, `native_timing`) into independently schedulable parts, so the quick pool is not bound by one 5–9 minute fixture.

Expected result: the slowest correctness fixtures fall to 1–2 minutes and the quick tier to about 3–5 minutes.

### Phase 3: engine speed (helps users as much as verification)

1. **Optimized development code generation**, through one of two options:
   - A `[profile.verify]` that inherits dev with `opt-level = 1` and `[profile.verify.package."*"] opt-level = 3`, used by the verifier and agents' demos, while plain `cargo build` stays at 7 s.
   - Or the same settings on dev itself, at 13 s per incremental build.

   Debug assertions and overflow checks stay on. Engine-heavy work runs about 2× faster, with identical decoded output.
2. **Scale FFV1 slices with frame size,** for example 16 at 1080p and above, while keeping the documented narrow-frame exception. 4K encode becomes 2.3× faster, the 30-minute 4K fixture render drops by roughly 10 minutes, and user renders gain the same. Receipts that report slice counts must be updated, and fixtures that assert them. **Done 5 October 2026:** frames of 640 x 360 and larger get 16 slices, and strict output verification decodes with 16 threads at every size.
3. **Real-time capture robustness.** Run the capture and fixture source threads under MMCSS "Pro Audio" priority and measure the gap rate. This hardens real user recordings against busy desktops, not just the test.

### Phase 4: workflow and environment

1. **One worktree and branch per agent session,** each with its own target directory (about 9 GB each; 280 GB is free). Integrate by commits, not by editing a shared tree. This removes interleaved changes, stale runs and executable locks between sessions.
2. **Thorough runs on commit snapshots,** in a dedicated `verify` worktree, nightly (Task Scheduler) and at milestones. Results are committed separately. Developers keep working while it runs.
3. **Stage-level resume for thorough runs.** If a thorough run fails in one stage, rerun only failed stages against the same starting source fingerprint and assemble one report. All stages still pass on identical sources, so evidence integrity holds. Today a single flaky 15-minute stage costs a full rerun.
4. **Real-time gates nightly.** Run the 15-minute capture in the nightly thorough run on an idle machine, with at most one retry that the report discloses.
5. **Machine (user decisions).**
   - **Dev Drive:** try a Windows 11 Dev Drive (ReFS, Defender performance mode) for the repository, target directories and the fixtures' temporary root, then re-measure section 2.3's launch throughput and the profile in 2.2. This touches security settings, so only you can make that change.
   - **Quiet machine:** use the High performance power plan, and keep heavy background applications closed during thorough runs.
6. **Agent guidance in `AGENTS.md`:**
   - While working: `cargo test` and `verify.py --only <touched fixtures>`.
   - Before committing: `verify.py`.
   - At milestones or when claiming points: `verify.py --thorough`.
   - Never run a thorough check for an intermediate step.

## 5. Order of work and checkpoints

| Step | Content | Proof it worked | Expected gain |
| --- | --- | --- | --- |
| 0 | Quick default, `--thorough`, `--only`, `--last-failed`, budgets helper, preflight, `CUTBOLT_EXE` copy | A quick run passes on an idle machine in 10–15 min; `--thorough` behaviour unchanged | Pre-commit check exists; nobody blocked |
| 1a | One `ffprobe` per source, cached tool versions | Shim launch counts per fixture before and after; all fixtures pass | About 60% fewer launches |
| 1b | Validated-probe cache and packet timing where frames are not read | Launch counts; 1080p preview latency; fixtures pass unchanged | Most remaining repeated launches; agent latency |
| 2a | Memoized transfer functions | `color`/`hdr` CPU before and after; identical results | About 1 min per run |
| 2b | Vectorized and parallel oracles; split long fixtures | cProfile before and after; identical pass sets | Quick tier at 3–5 min |
| 3a | `profile.verify` | Render benchmark and identical decoded MD5 | About 2× engine compute |
| 3b | FFV1 slice scaling | 4K encode fps; decoded output identical | About 10 min off the thorough run; faster user renders |
| 4 | Per-session worktrees, nightly thorough, stage resume | One week of nightly results | Thorough runs leave the critical path |

## 6. What not to do

- Do not shorten the 30-minute 4K or 15-minute capture acceptance cases, or remove any check, to save time. Move them to the thorough tier instead.
- Do not add lanes beyond about 4–8 for launch-heavy fixtures before phase 1: section 2.3 shows it slows every fixture.
- Do not cache engine outputs, and do not let a cache bypass the identity check of the bytes actually used.
- Do not mark points from quick runs. Quick runs write no evidence.
