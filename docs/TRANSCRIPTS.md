# Transcript records and selected-word cuts

The original transcript editing contract and optional local English/Greek speech command pass the complete bounded recognition, editing and failure acceptance suite. Both G04 checkpoints are verified. The stable full run on 3 October 2026 reaches **83/100 core and 83/108 total (76.9%)**, with 358 unique checks; see [progress](PROGRESS.md).

Use `transcript.inspect`, `transcript.correct` and `transcript.plan` through the JSON CLI, Rust library or optional MCP stdio. They return immutable results and write no files or saved state. Apply a returned `transcript.cut` operation through `timeline.apply` or the existing saved-session preview/apply contract. There is no listening service or runtime download.

## Optional local recognition

`transcript.transcribe` is a blocking CLI/library command; it is excluded from the serial MCP catalog. Supply an ID, identity-bound relative `source`, explicit source `format`, exact `start`/`duration`, `channel`, `language`, `input_root`, `scratch_root`, `runtime` and `timeout_seconds`. The runtime is trusted local configuration: explicit WSL distribution, absolute Linux Python path and package directories, absolute Windows model/alignment paths and 1–8 threads. No runtime setup or download is attempted. Current support is Windows with the default `/mnt/<drive>` WSL mounts and a local CUDA device; missing requirements fail locally.

Accepted parents are `stereo_wav` (48 kHz stereo PCM16) or `reference_movie` with explicit width/height (the existing 25 fps FFV1/PCM profile). The command verifies the real source identity and sample count, extracts a 25 ms–120 s interval and creates an owned 16 kHz mono analysis WAV. Choose the left channel, right channel or equal mean explicitly. Resampling uses the pinned external media tool's 32-tap, phase-10 SWR filter without dither. Exact original 48 kHz source origins are retained; a final fractional analysis sample is clamped by at most two source samples. When SWR rounds that partial sample down, one explicit silent analysis sample is appended; `partial_tail_padding_16000` reports it, while `last_sample_clamp_48000` records the exact source-end clamp. Rendering never overwrites the source.

The `local-en-el-context-v1` profile uses external multilingual speech recognition followed by separate English/Greek acoustic models. Original code maps public CTC alignment to its exact convolution clock, refines edges from bounded local energy islands and adds **80 ms leading / 20 ms trailing context** where inter-word gaps permit. Overlapping context shares the gap at its sample midpoint. Returned word `start`/`end` values describe these estimated editing intervals. The separate `alignment` record retains raw CTC and energy-refined times plus an acoustic score. Those raw times are not claimed to meet the contextual interval accuracy gate. Corrections remove per-word model scores/evidence while preserving the document's recognition provenance.

Recognition retains one source window up to 30 seconds. Longer input uses disjoint windows split at the midpoint of the longest RMS-quiet gap of at least 200 ms found 8–14 seconds after the current start; ties select the later gap. With no qualifying gap, the window ends at 12 seconds and explicitly reports `hard_12s`. A hard boundary can split a spoken word and requires review. The receipt reports every source window, boundary policy and corresponding word indices. For long input each recognition window is at most 15 seconds, including the final remainder. Acoustic alignment constrains each window’s words to its own source interval, so repeated text cannot shift a phrase into a different recognition window. This avoids relying on internal timestamp seeking to advance through a long source.

Acoustic alignment longer than 30 seconds uses 24-second acoustic tiles with one second of context on each side and exact retained convolution-frame indices. The profile bounds targets to 16,384 acoustic labels, results to 2,048 words, worker output to one MiB, post-inference CPU peak memory to 6 GiB and allocated CUDA memory to 8 GiB. These are checked result gates, not OS-enforced memory reservations. Digital silence returns `NO_WORDS` before inference. Numeric word spellings and unsupported acoustic characters reject explicitly; the profile does not invent timings for them. Every result requires review, and the cut planner rejects estimates unless `allow_reported` is explicit. Recognition can substitute, omit or insert words; caller corrections remain necessary.

The original supervisor is namespace PID 1 with no external network interface. It runs a fixed content-checked worker, enforces the explicit 1–600 second analysis deadline, and treats closure of the native owner's input pipe as cancellation. Exiting that PID namespace terminates detached descendants too. Only the three known files in the invocation's owned scratch directory are removed; cleanup never traverses arbitrary directories. This is a process/network boundary, not a filesystem sandbox. A hard exit before the supervisor starts, or forcefully killing the WSL bridge itself, can leave the three known scratch files. The bridge-kill fixture observes this limitation and verifies that no worker or detached descendant survives. Such files are not a published transcript or changed source. Ordinary error, deadline and owner-pipe cancellation use the cleanup path.

## Document contract

A version-1 document contains an ID, revision, optional parent fingerprint, source identity/path/duration, an analysis range, explicit `en` or `el` language, recognition provenance and ordered words. Every word has a stable ID, one text unit, exact start/end times, an `estimated` or `corrected` origin and optional `probability_milli` from 0 to 1,000. Confidence is model output, not a factual probability of correctness. Corrected words have no model probability.

Times are **absolute source times**, represented as reduced nonnegative rationals on the 48 kHz sample clock. They are not relative to the analysis range or the clip's timeline placement. Intervals are positive, ordered, nonoverlapping and contained in the declared analysis range. The range may start after zero and is limited to 120 seconds; a document holds at most 2,048 words and 128 KiB of word text. Each word is at most 512 UTF-8 bytes. Punctuation and multibyte text are preserved; multiple whitespace-separated words in one record reject.

The source path is relative to an explicit absolute `input_root`. Inspection, correction and planning read the actual source and check its byte count and SHA-256 identity. Recognition provenance records a profile, model identity, worker/analysis hashes and version map. The speech command records its actual pinned files and worker/supervisor hashes. Manually supplied documents still carry caller-asserted metadata. Neither a document fingerprint nor an origin flag authenticates a recognizer or proves human review.

## Correction

`transcript.correct` requires `document`, `expected_fingerprint`, `edits` and `input_root`. The batch contains 1–128 explicit operations:

- `replace`: supply `word` with the existing ID, text, start and end.
- `insert`: supply a fresh `word` and `before_id`, or `null` to append.
- `remove`: supply the word `id`.

Replacement and insertion mark the resulting word `corrected`, meaning its text and times were explicitly supplied by the caller. The batch preserves source and recognition provenance, increments the document revision and records the previous fingerprint as its parent. An invalid batch returns no candidate. IDs removed in a batch cannot be reused within that batch. Removing every word is permitted.

## Cut planning

`transcript.plan` requires `project`, `document`, `expected_revision`, `expected_document_fingerprint`, `input_root` and an explicit `spec`:

| Field | Meaning |
| --- | --- |
| `clip_id` | A concrete media clip in the root native arrangement; nested instances are unsupported |
| `track_ids` | Explicit target tracks, including the bound clip's track |
| `ranges` | 1–128 inclusive `{first_id,last_id}` selections in document order |
| `clock` | `video` for the project frame clock or `audio` for 48 kHz samples |
| `rounding` | `strict` rejects nonaligned boundaries; `outward` expands to surrounding clock ticks |
| `estimates` | `reject` or explicit `allow_reported` for selected estimated words |
| `collateral` | `reject` or explicit `allow_reported` for unselected words touched by rounding |
| `links` | Existing `include` or `reject_partial` linked-track policy |
| `end_policy` | Existing `resize` or `keep` timeline-end policy |
| `transitions` | Existing `reject_affected` or `remove_affected` policy |

The selected words must be fully inside the bound source clip. The asset's identity and duration must match the document. Planning checks the actual asset file as well as the document source; a separate identical copy is allowed. Changed bytes reject even if the other copy remains intact. Source-to-timeline mapping uses the native clip's exact source-in and placement. Clock expansion cannot cross the clip's edges. Audio-clock selection still rejects a non-frame-aligned deletion when linked video is affected.

Overlapping or touching selections form one union. Separated intervals are deleted from latest to earliest using the native ripple editor, deterministic fresh fragment IDs, linked-track expansion, lock checks and transition rules. These are timeline deletions across affected tracks: other clips on those tracks can move or be trimmed. Inspect the returned native operations and saved-session diff before applying.

The result reports requested and snapped boundaries, expansions, merged cuts, selected estimates, collateral word IDs and retained source/time fragments. A word crossing a clip edge has `binding_complete: false`; its intersecting portion is still checked for collateral cuts. Words fully outside the binding are reported separately. The projection describes this bound clip's words, not every occurrence of the source elsewhere in the project.

## Reading a cut against its transcripts

`timeline.outline` takes the same documents and shows, on each audio clip of a cut, the words inside its source span. A word cut by a clip edge is marked `*`. Use it after a word cut to check that the timeline says what was intended. See [USAGE.md](USAGE.md#reviewing-edits-and-footage).

## Saved operations and stale data

The returned operation contains the document and a plan bound to the full project and document fingerprints. Applying it recomputes the native operations. A changed project or supplied document rejects; no partial timeline changes are published. A successful batch increments the project revision once. Saved sessions retain their durable request IDs, retry receipts, diffs, undo and restoration behavior.

There is no global transcript database or automatic “latest document” pointer. Intentionally resubmitting an older document and its matching plan does not establish that it is the user's latest correction. Planning reads source files; pure snapshot/session application checks the bound metadata and fingerprints, with media identities checked again during rendering. It is not a filesystem transaction spanning recognition, planning and application.

## Current evidence and remaining work

The original `tests/transcripts.py` fixture uses authored word anchors over coded pixels and stereo samples. It verifies corrected/estimated selections, nonzero source and placement offsets, a seven-sample linked offset, merged/separated cuts, saved preview/apply/retry/undo/restore, typed MCP, frame/range previews, malformed/stale records and source/output preservation. The retained focused run compares 666 decoded frames and 1,278,720 stereo sample frames exactly, plus eight still previews and 37 rejection cases. The maximum document/batch checks exercise 2,048 words, 128 corrections and 128 distinct sample-clock cuts; the latter produces 129 audio fragments and is an editing-only check beyond the current renderer clip bound. These are editing-contract results, **not speech-recognition accuracy evidence**.

The complete native recognition fixture separately compares 663 decoded frames, 1,272,960 stereo sample frames and six previews from both estimated and corrected selections. Six short English/Greek cases pass the contextual onset gates; recognition errors remain visible (one substitution in each of the two short Greek pilot/isolated cases in the full run). Both two-minute inputs retain every reference word: 240 English and 234 Greek, with zero word-edit distance. Contextual onset maximum/95th-percentile errors are 150/80 ms and 135/125 ms. Full-run inference takes 48.6/69.5 seconds, with CPU peaks 2.41/4.17 GB and allocated GPU peaks 1.47/1.87 GB. Thirty rejected editing cases, 28 native failure cases, cancellation, owner termination and eleven supervisor cases also pass. This is fixture evidence within the declared runtime and media bounds, not universal speech accuracy. Research, downloaded weights and generated speech remain outside the repository.

## External verification setup

Full verification requires the installed optional runtime and voices from the [dependency ledger](DEPENDENCIES.md). Set `CUTBOLT_TRANSCRIPTION_RUNTIME` to an absolute path outside the repository containing the following configuration shape (replace the example paths with the installed, pinned assets):

```json
{
  "distribution": "Ubuntu",
  "python": "/usr/bin/python3",
  "python_paths": ["/home/user/speech-packages"],
  "model": "C:/SpeechModels/small.pt",
  "alignment_roots": {
    "en": "C:/SpeechModels/english",
    "el": "C:/SpeechModels/greek"
  },
  "threads": 4
}
```

The native command takes the same runtime settings with one language-specific `alignment_root` instead of the test configuration's `alignment_roots`. Recognition consumes an identity-bound source already inspected or registered by the caller; it returns JSON without saving a transcript or mutating a project. Save the returned document outside the repository, review/correct it, then inspect and apply a content-bound cut through the usual session contract.

Run `python tests/transcription.py --output <fresh-external-directory> --runtime <runtime-json>` for the focused fixture. `python tools/verify.py --thorough --decode-device 0` reads the same environment variable and includes it in the full acceptance run. Missing setup fails explicitly; it never skips recognition tests while awarding coverage. Fixture outputs include actual transcripts, independent word-error/timing comparisons, native long-input resource receipts and process-lifetime checks.
