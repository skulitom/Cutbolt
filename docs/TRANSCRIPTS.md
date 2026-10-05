# Transcript records and selected-word cuts

The original transcript editing contract and optional local English/Greek speech command pass the complete bounded recognition, editing and failure acceptance suite. Both G04 checkpoints are verified. The stable full run on 3 October 2026 reaches **83/100 core and 83/108 total (76.9%)**, with 358 unique checks; see [progress](PROGRESS.md).

Use `transcript.inspect`, `transcript.correct` and `transcript.plan` through the JSON CLI, Rust library or optional MCP stdio. They return immutable results and write no files or saved state. Apply a returned `transcript.cut` operation through `timeline.apply` or the existing saved-session preview/apply contract. There is no listening service or runtime download.

## Optional local recognition

`transcript.transcribe` is a blocking CLI/library command; it is excluded from the serial MCP catalog. Supply an ID, identity-bound relative `source`, explicit source `format`, exact `start`/`duration`, `channel`, `language`, `input_root`, `scratch_root`, `runtime` and `timeout_seconds`. The runtime is trusted local configuration: explicit WSL distribution, absolute Linux Python path and package directories, absolute Windows model/alignment paths and 1–8 threads. No runtime setup or download is attempted. Current support is Windows with the default `/mnt/<drive>` WSL mounts and a local CUDA device; missing requirements fail locally.

Accepted parents are `stereo_wav` (48 kHz stereo PCM16) or `reference_movie` with explicit width/height (the existing 25 fps FFV1/PCM profile). The command verifies the real source identity and sample count, extracts a 25 ms–120 s interval and creates an owned 16 kHz mono analysis WAV. Choose the left channel, right channel or equal mean explicitly. Resampling uses the pinned external media tool's 32-tap, phase-10 SWR filter without dither. Exact original 48 kHz source origins are retained; a final fractional analysis sample is clamped by at most two source samples. When SWR rounds that partial sample down, one explicit silent analysis sample is appended; `partial_tail_padding_16000` reports it, while `last_sample_clamp_48000` records the exact source-end clamp. Rendering never overwrites the source.

The `local-en-el-context-v1` profile uses external multilingual speech recognition followed by separate English/Greek acoustic models. Original code maps public CTC alignment to its exact convolution clock, refines edges from bounded local energy islands and adds **80 ms leading / 20 ms trailing context** where inter-word gaps permit. Overlapping context shares the gap at its sample midpoint. Returned word `start`/`end` values describe these estimated editing intervals. The separate `alignment` record retains raw CTC and energy-refined times plus an acoustic score. Those raw times are not claimed to meet the contextual interval accuracy gate. Corrections remove per-word model scores/evidence while preserving the document's recognition provenance.

Recognition retains one source window up to 30 seconds. Longer input uses disjoint windows split at the midpoint of the longest RMS-quiet gap of at least 200 ms found 8–14 seconds after the current start; ties select the later gap. With no qualifying gap, as under a music bed or room tone, the worker recognizes the next 14 seconds and ends the window at the middle of the widest gap between recognized words that meets 8–14 seconds; ties select the later gap. It reports `word_gap`, keeps the words before the cut, and recognizes the next window afresh from there. When no word boundary lies in that span, the window ends at 12 seconds and explicitly reports `hard_12s`. A hard boundary can split a spoken word and requires review. The receipt reports every source window, boundary policy and corresponding word indices. For long input each recognition window is at most 15 seconds, including the final remainder. Acoustic alignment constrains each window’s words to its own source interval, so repeated text cannot shift a phrase into a different recognition window. This avoids relying on internal timestamp seeking to advance through a long source.

Recognizers also write down sound that is not speech. Bracketed annotations such as `[Music]` or `(upbeat music)`, up to eight tokens long, and tokens made of music symbols such as `♪` are removed before alignment. They are returned as `non_speech` notes with their `kind` (`annotation` or `music`), text and source times. A token with no letter or digit, such as the `-` of a word cut off mid-way, joins the word it is written against. When there is no such word it becomes a `symbols` note. Word text never keeps the recognizer's surrounding whitespace. Audio in which recognition finds no speech, such as a music bed alone, returns a document with no words plus its notes. Digital silence still returns `NO_WORDS` before inference.

Acoustic alignment longer than 30 seconds uses 24-second acoustic tiles with one second of context on each side and exact retained convolution-frame indices. The profile bounds targets to 16,384 acoustic labels, results to 2,048 words, worker output to one MiB, post-inference CPU peak memory to 6 GiB and allocated CUDA memory to 8 GiB. These are checked result gates, not OS-enforced memory reservations. Digital silence returns `NO_WORDS` before inference. A letter the acoustic vocabulary lacks is aligned as its base letter when that exists, so `café` aligns as `CAFE`. Numeric word spellings and other unsupported characters reject explicitly, naming the word; the profile does not invent timings for them. Every result requires review, and the cut planner rejects estimates unless `allow_reported` is explicit. Recognition can substitute, omit or insert words; caller corrections remain necessary.

The original supervisor is namespace PID 1 with no external network interface. It runs a fixed content-checked worker, enforces the explicit 1–600 second analysis deadline, and treats closure of the native owner's input pipe as cancellation. Exiting that PID namespace terminates detached descendants too. Only the three known files in the invocation's owned scratch directory are removed; cleanup never traverses arbitrary directories. This is a process/network boundary, not a filesystem sandbox. A hard exit before the supervisor starts, or forcefully killing the WSL bridge itself, can leave the three known scratch files. The bridge-kill fixture observes this limitation and verifies that no worker or detached descendant survives. Such files are not a published transcript or changed source. Ordinary error, deadline and owner-pipe cancellation use the cleanup path.

## Whole-file recognition

`media.transcribe`, queued with `job.start`, recognizes a whole source file in one job. The file can be any length and any format FFmpeg decodes with an audio stream, including 30 fps prepared assets that the reference-movie format does not accept. The job works in steps:
1. It extracts the first audio stream losslessly to 48 kHz stereo PCM16, so word times are source times.
2. It recognizes `[start, start + duration)` (default the whole audio) in 120 s windows, each starting 5 s before the previous one ends, with the same runtime settings as `transcript.transcribe`.
3. It stitches each overlap where the documents meet. The earlier document keeps the words whose middle lies before the overlap's middle. The seam moves to the end of its last kept word if that is later, and the next document keeps only words starting at or after the seam.
4. It binds the documents to the source file itself (relative path, content identity and audio length), so `timeline.outline`, `captions.draft`, `export.review` and `transcript.plan` match them to the file's assets.
5. It saves `{"transcripts": [...], "non_speech": [...]}` to a new `.json` `output`. Each non-speech note is kept once, inside the document whose range holds its start. With a workspace, later calls can name the documents as `{"file": ..., "select": "transcripts"}`.

Documents are `<id>-1`, `<id>-2` and so on, and their words remain estimates that need review. A window of digital silence inside a longer range has nothing to recognize: it is skipped, listed in `silent_windows`, and no document covers it. A recognition failure leaves no output file.

**Several files and few launches.** `paths` (1–64 whole files) recognizes several files in one job, with optional `texts` (one script per file) in place of `text`, and saves all their documents in one `output`. Documents are named `<file stem>-1`, and so on. The result lists each file, and a file that fails is listed with its error while the others are saved. Windows of every file, and of one long file, go to the speech runtime together. Each launch takes up to 16 windows (at most ten minutes of audio within the 64 KiB request), so the isolated worker starts and loads its models once per launch instead of once per window. Each window keeps its own outcome, so digital silence in one does not fail the others. The demo's seven narration lines (7.8–11 s each) took about 22 s each one at a time. Together they took 15 s aligned to their scripts and 25 s recognized. A two-window, 136 s file took 37 s, about the same as a single short line.

## Aligning known text

When the words are already known, for example the script given to a speech synthesizer, pass them as `text` to `transcript.transcribe` or `media.transcribe`. No recognition runs: the given words are aligned to the audio with the same acoustic model, clock and contextual intervals. Names keep their spelling (`PixelForge` stays one word), and a disfluency written in the script (`um,`) gets its own interval instead of stretching its neighbour.

- **Words.** The text splits at whitespace. A token without a letter or digit, such as a dash between spaces, joins the word before it, or the next word at the start. Write numbers out in words.
- **Limits.** At most 32 KiB and 2,048 words, and one range of at most 120 s. `media.transcribe` with `text` rejects a longer range; give `start` and `duration` for the part the text covers.
- **Result.** The document's profile is `local-en-el-align-v1`, and its model identity is the acoustic alignment weights. Words are `estimated`, with acoustic evidence and no recognizer confidence (`probability_milli` is null). The recognition window reports `given_text`.

Alignment is forced: it does not check that the text was actually said. If the audio says something else, the word times are wrong. Review the result as for recognition. Speech the text leaves out, such as an "um" a narrator added, is reported as [uncovered speech](#uncovered-speech).

## Vocabulary

Pass `vocabulary` to `transcript.transcribe` or `media.transcribe` for real recordings: terms the speech may contain, spelled as they should be written. Use it for names (`PixelForge`, `Cutbolt`) and, to keep hesitations, `um` and `uh`. It guides recognition only, so it is rejected together with `text`.

- **Prompt.** The terms are joined with commas and end with a full stop (`um, uh, PixelForge, Cutbolt.`). They are given to the recognizer as preceding text for every decode of every window. A vocabulary holds at most 32 terms of 1–64 bytes. The prompt must also fit the recognizer's 223-token prompt context. The worker counts it with the recognizer's own tokenizer and rejects a longer one as `INPUT_LIMIT`, so no term is silently dropped.
- **Respelling.** After recognition, two to four consecutive words whose letters and digits run together into a one-word term become that term, keeping the outer punctuation: `pixel forge,` becomes `PixelForge,`. Words with punctuation between them are not joined. A single word spelled differently takes the term's spelling. The exception is an all-lowercase term such as `um`, which leaves a capitalized `Um` at a sentence start. Terms with spaces only prompt.
- **Provenance.** The document records the terms in `recognition.vocabulary`, and the worker's hash fixes how the prompt is built. Respelled words stay `estimated` and are aligned like any other word. The worker result counts them in `respelled`.
- **Evidence.** In the 5 October demo narration, Whisper small heard "pixel forge" and "cut bolt". With `["PixelForge", "Cutbolt"]` it wrote both names whole. With `["um", "uh"]`, it kept the "um" it had dropped from "Play the frames in order and, um, the character moves". Neither vocabulary added or lost a word on the six other lines. The prompt is a hint, not a guarantee, so review still applies.

## Uncovered speech

Recognizers leave out hesitations, false starts and sometimes whole words, and the word beside them can stretch over the gap. Duration alone does not tell these apart. In the demo, the "and" before the dropped "um" lasted 0.73 s, and the "and" of the next sentence lasted 0.69 s with no filler after it.

The acoustic model's own reading does tell them apart. After alignment, the worker takes the model's most likely label in every 20 ms frame:
1. **Letters outside the words.** Frames whose label is a letter, more than two frames from every word's CTC span, form groups when at most five frames apart.
2. **Voiced extent.** Each group grows over its voiced audio: 5 ms RMS frames within 14 dB of the group's peak, across holes of up to 20 ms. Groups that meet merge.
3. **Separate sounds only.** A group that grows into a word's CTC span, with no such dip between, is left to that word. It cannot be told from the word's own onset or ending, which the aligner may place a little late or early. In the Greek "γάτα", for example, the aligner put the "Γ" at the vowel, and the model read the onset before it as "Χ".
4. **Background.** A group more than 14 dB below the median level inside the words is left out.

The rest are the document's `uncovered` list: `{start, end, letters}` in source time, such as `AM` for that "um" at 2.62–3.15 s. They are candidates for review, never words, and word intervals are computed as before. A word's context can reach into a sound beside it. Readers therefore pick a sound's neighbours by its middle, and a word over the middle covers the sound. A filler run on into a word without a pause, as in "and-um", is not reported. Known text is checked the same way: a script without the "um" leaves that sound uncovered.

- `media.transcribe` lists the sounds with the words either side.
- `transcript.fillers` lists those on the timeline and can cut them; see [removing filler words](#removing-filler-words).
- `export.review` lists those heard in a delivered cut.
- `transcript.correct` drops a sound once a word covers its middle, for example an inserted `um`.
- Rebinding onto a conformed or prepared asset moves the sounds with the words.

On the demo's seven narration lines, the only uncovered sound was that "um". In `export.review` of the demo's 80 s final cut, which has a music bed, it was again the only one, still in the cut at 29.74–30.30 s. With the names as known terms, the review heard all 138 expected words.

## Document contract

A version-1 document contains an ID, revision, optional parent fingerprint, source identity/path/duration, an analysis range, explicit `en` or `el` language, recognition provenance, ordered words and optionally the ordered [uncovered sounds](#uncovered-speech) that no word covers. Every word has a stable ID, one text unit, exact start/end times, an `estimated` or `corrected` origin and optional `probability_milli` from 0 to 1,000. Confidence is model output, not a factual probability of correctness. Corrected words have no model probability.

Times are **absolute source times**, represented as reduced nonnegative rationals on the 48 kHz sample clock. They are not relative to the analysis range or the clip's timeline placement. Intervals are positive, ordered, nonoverlapping and contained in the declared analysis range. The range may start after zero and is limited to 120 seconds; a document holds at most 2,048 words and 128 KiB of word text. Each word is at most 512 UTF-8 bytes. Punctuation and multibyte text are preserved; multiple whitespace-separated words in one record reject. Recognition and `transcript.correct` write word text without surrounding whitespace. Documents recognized by earlier versions keep the recognizer's leading space (`" tiny"`). Every reader ignores it (outline, captions, review, fillers and assembly), so their fingerprints stay as they are, and the next correction removes it.

The source path is relative to an explicit absolute `input_root`. Inspection, correction and planning read the actual source and check its byte count and SHA-256 identity. Recognition provenance records a profile, model identity, worker/analysis hashes, version map and any recognition vocabulary. Documents without uncovered sounds or a vocabulary serialize exactly as before, so their fingerprints are unchanged. The speech command records its actual pinned files and worker/supervisor hashes. Manually supplied documents still carry caller-asserted metadata. Neither a document fingerprint nor an origin flag authenticates a recognizer or proves human review.

## Correction

`transcript.correct` requires `document`, `expected_fingerprint`, `edits` and `input_root`. The batch contains 1–128 explicit operations:

- `replace`: supply `word` with the existing ID, text, start and end.
- `insert`: supply a fresh `word` and `before_id`, or `null` to append.
- `remove`: supply the word `id`.

Replacement and insertion mark the resulting word `corrected`, meaning its text and times were explicitly supplied by the caller. An uncovered sound whose middle a word now covers is dropped. The batch preserves source and recognition provenance, increments the document revision and records the previous fingerprint as its parent. An invalid batch returns no candidate. IDs removed in a batch cannot be reused within that batch. Removing every word is permitted.

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

## Paper edits

`transcript.assemble` is read-only and builds a rough cut from what people say. It takes a sequential `project` and the `transcripts` of its sources. Each of its `selections` names a transcript and a first and last word; the project must already have an asset for that transcript's source (matched as for `timeline.outline`).

Each selection becomes one `clip.append`, in the given order, covering its words' source:
- **Padding.** `padding` (default zero, at most 2 s) extends each side, clipped to the asset.
- **Whole frames.** The start rounds down and the end rounds up to whole frames, so no selected word is clipped. The end never passes the asset's last whole frame.

New clip IDs are `<clip_prefix><n>` (default `s1`, `s2`, …), skipping IDs the project already uses. The result lists each clip with its words. The batch is checked to apply. Placed-track projects are refused: assemble first, then promote to tracks for music and overlays.

## Removing filler words

`transcript.fillers` is read-only. It takes a `project` and `transcripts` of its sources, as `timeline.outline` does, and finds `words` wherever the timeline speaks them, matched on letters and digits ignoring case and punctuation. The default list is um, uh, erm, er, ah, uhm, umm, hmm and mm. It then proposes ripple deletions that remove the fillers, built in four steps:
1. **Runs.** Consecutive fillers form one cut.
2. **Padding.** `padding` (default zero, at most 1/4 s) extends a cut on each side, but never into the neighbouring words.
3. **Snapping.** Cut ends go to the nearest grid point, or away from a neighbouring word when the nearest would enter it. The grid is whole frames that, on placed tracks, are also whole 48 kHz samples. A filler shorter than one grid step between its neighbours is reported and left.
4. **Rippling.** As for `audio.tighten`, cuts ripple every track, split clips and links get new right-hand IDs, and each cut is tried on a working copy. Refused cuts are listed with the reason, so the returned batch applies.

**Fillers the recognizer left out.** The result's `uncovered` lists the transcripts' [uncovered sounds](#uncovered-speech) that lie wholly inside the window and inside audible clips. Each entry gives its letters, timeline times and the words on either side. `filler_like` marks those that read like a hesitation, lasting at least 1/8 s, with at most four letters:
- vowels (A, E or U), then an optional H and M or R, such as AM, UH, ER or ERM;
- or M with an optional leading H, such as MM or HMM.

With `uncovered: true`, each filler-like sound is cut like a filler word between its neighbours, with the same padding, snapping and rippling or lifting. Otherwise each sound is listed with the reason it was not cut, and `next` says how many could be. Review them first: an A or AM can also be a short word the recognizer missed, such as "a" or "am".

## Reading a cut against its transcripts

`timeline.outline` takes the same documents and shows, on each audio clip of a cut, the words inside its source span. A word cut by a clip edge is marked `*`. Use it after a word cut to check that the timeline says what was intended. See [USAGE.md](USAGE.md#reviewing-edits-and-footage). `captions.draft` turns the same words into caption cues for the timeline. After delivery, `export.review` compares the words the cut should say with the words heard in the rendered file, prompting recognition with the names the transcripts spell; see [USAGE.md](USAGE.md#reviewing-a-delivered-cut).

## Saved operations and stale data

The returned operation contains the document and a plan bound to the full project and document fingerprints. Applying it recomputes the native operations. A changed project or supplied document rejects; no partial timeline changes are published. A successful batch increments the project revision once. Saved sessions retain their durable request IDs, retry receipts, diffs, undo and restoration behavior.

There is no global transcript database or automatic “latest document” pointer. Intentionally resubmitting an older document and its matching plan does not establish that it is the user's latest correction. Planning reads source files; pure snapshot/session application checks the bound metadata and fingerprints, with media identities checked again during rendering. It is not a filesystem transaction spanning recognition, planning and application.

## Current evidence and remaining work

The original `tests/transcripts.py` fixture uses authored word anchors over coded pixels and stereo samples. It verifies corrected/estimated selections, nonzero source and placement offsets, a seven-sample linked offset, merged/separated cuts, saved preview/apply/retry/undo/restore, typed MCP, frame/range previews, malformed/stale records and source/output preservation. The retained focused run compares 666 decoded frames and 1,278,720 stereo sample frames exactly, plus eight still previews and 37 rejection cases. The maximum document/batch checks exercise 2,048 words, 128 corrections and 128 distinct sample-clock cuts; the latter produces 129 audio fragments and is an editing-only check beyond the current renderer clip bound. These are editing-contract results, **not speech-recognition accuracy evidence**.

The complete native recognition fixture separately compares 663 decoded frames, 1,272,960 stereo sample frames and six previews from both estimated and corrected selections. Six short English/Greek cases pass the contextual onset gates; recognition errors remain visible (one substitution in each of the two short Greek pilot/isolated cases in the full run). Both two-minute inputs retain every reference word: 240 English and 234 Greek, with zero word-edit distance. Contextual onset maximum/95th-percentile errors are 150/80 ms and 135/125 ms. Full-run inference takes 48.6/69.5 seconds, with CPU peaks 2.41/4.17 GB and allocated GPU peaks 1.47/1.87 GB. Thirty rejected editing cases, 28 native failure cases, cancellation, owner termination and eleven supervisor cases also pass. This is fixture evidence within the declared runtime and media bounds, not universal speech accuracy. Research, downloaded weights and generated speech remain outside the repository.

The quick-tier recognition fixture also checks the [vocabulary](#vocabulary) and [uncovered speech](#uncovered-speech):
- **Names.** A synthesized "PixelForge … Cutbolt" line comes back with both names whole and the terms recorded. Without the vocabulary, the recognizer heard "Pyxel Forge" and "cut bolt".
- **A left-out filler.** A line aligned to a script that leaves out its "um" reports exactly one uncovered sound that reads like a filler. It lies inside the synthesizer's own clock for the "um", between "circle," and "and", and `transcript.fillers` cuts it between those words.
- **No false alarms.** The six clean English and Greek fixtures report no uncovered sounds.

Pure checks cover respelling, prompts, readings, and onsets left to their words. These are quick-tier results; the thorough evidence above predates them.

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
