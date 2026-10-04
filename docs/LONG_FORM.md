# Long-form reference rendering

The original sequential FFV1/PCM path uses a bounded large-raster profile for
frames containing at least 8,000,000 pixels, with both dimensions at least four.
It selects 16 FFV1 slices, 16 encoder threads and four input decoder threads.
Full video inspection uses 16 decoder threads with a 900-second tool deadline;
the sequential encoder has an 1,800-second deadline. These are individual tool
deadlines, not a promise that every project completes in that time. Other frame
sizes retain the existing encoding profile and deadlines. Placed-track rendering
does not gain a new large-project or long-form guarantee.

All native rate, whole-frame/sample, source identity, frame count, timestamp and
output publication checks still apply. The current sequential limit remains 64
clips and 180,000 frames. This does not imply real-time 4K playback, arbitrary
codec support or arbitrary memory use. Sources are never overwritten.

Normal failure and cancellation remove only their owned temporary output. On
Windows, a terminated child can briefly retain a file handle after its process
scope closes. Cleanup retries sharing/lock errors on that exact path for up to
five seconds, at 20-millisecond intervals. Other errors are not retried; a crash
or persistent filesystem failure may still leave scratch files. There is no
directory sweep or deletion of another job's output.

## Acceptance evidence

`tests/long_form_4k.py --output <external-new-directory> --long-form` creates an
original 30-minute, 3840 × 2160, 25 fps edit. Two nonzero source ranges surround
a one-second black/silent gap. Its moving gradients, edges and binary frame
identifiers repeat over a one-second source cycle; the stereo PCM sample-index
pattern spans the entire source without that cycle. Every one of the 45,000
decoded RGB frames is compared with the independently generated pixel hashes,
and every one of the 86,400,000 stereo sample frames is compared directly. All
video timestamps and preserved source hashes are checked separately.

The complete native pipeline includes source inspection, encoding, output
inspection and integrity checking. Its acceptance limits are 2,700 seconds and
4 GiB sampled process-tree peak working set on the recorded test machine. A
200-millisecond Windows process sampler sums the reported per-process peak
working sets of currently visible descendants; this is a bounded sampled metric,
not a guarantee of observing every allocation. Independent post-render pixel
and audio comparisons are additional verification work outside that timing gate.
Short mode verifies 12 seconds and cannot satisfy the long-form criterion.

`tests/long_form_stress.py` repeats three complete 12-second moving edits with
a 60-second/4-GiB gate per render, verifies every frame/sample, exercises two
actual encoder writes failing after 8,192 bytes, cancels a running 30-minute
4K job, holds real process handles to check tool-tree exit, and verifies the
following queued edit. The write fault injects OS error 112; it does not fill a
physical volume. Cancellation has a ten-second gate, with no published output
or leftover owned partial. Existing output content and sources stay unchanged.

The full verifier requires both fixtures and the existing 1,000-clip edit gate.
Run its performance gates without unrelated CPU-intensive jobs. The full run passes these gates; see [current progress](PROGRESS.md).
