# Local audio recording

The complete acceptance suite verifies A06 basic and extended, including isolated native capture, exact saved-timeline placement, explicit timing correction, input loss and a sustained 15-minute run. The authoritative score is in [progress](PROGRESS.md).

`audio.inputs` lists active local capture endpoints and their observed mix formats. `audio.record.inspect` validates an explicitly selected endpoint or process and exact duration without starting a stream. `audio.record` captures a new WAV through the local Windows audio engine. `audio.record.place` returns validated native audio-track placement operations for the existing saved-session workflow.

Recording is a blocking CLI/library operation. The other three commands are read-only MCP tools. There is no listening service, runtime download, default-input selection or implicit fallback to another input.

## Select an input explicitly

```json
{"command":"audio.inputs"}
```

Each available device has an `id`, friendly name, current mix format and period. Discovery activates interfaces to read metadata, but never initializes or starts audio capture. An unavailable device can appear with a structured error if it changes during enumeration. Use a returned capture endpoint ID:

```json
{
  "command":"audio.record.inspect",
  "input":{"type":"endpoint","id":"<selected ID from audio.inputs>"},
  "duration":{"num":30,"den":1}
}
```

Alternatively, `{"type":"process","pid":12345}` selects that live process and its descendants through Windows process-specific loopback. It excludes unrelated application audio. Supply the actual selected process ID; the example number is not an executable recipe. A held process handle and recorded creation identity protect an active capture from switching to a reused process ID. The selected process must stay alive until its requested samples arrive. Process capture requires a Windows version supporting this interface; unsupported systems return an error. No systemwide loopback fallback is used.

## Capture a new WAV

```json
{
  "command":"audio.record",
  "input":{"type":"endpoint","id":"<selected ID>"},
  "duration":{"num":30,"den":1},
  "output_root":"C:\\DEV\\CutboltData\\recordings",
  "output":"C:\\DEV\\CutboltData\\recordings\\take-001.wav"
}
```

Create the output directory first and use a new filename. The result contains the file identity, PCM digest, exact sample count, input identity, requested format and packet timing. Save the receipt alongside the take. The WAV is immediately usable independently of a project, and a failed placement does not change it.

The output format is **48,000 Hz stereo PCM16**, from one sample to two hours. Duration must lie exactly on the 48,000 Hz sample grid. Endpoint capture explicitly requests shared-mode conversion to this format, including conversion from a different native rate or layout when Windows supports it. There is no claim that the file contains raw hardware samples before system processing. Input device processing, system mixing, conversion and dithering can affect the captured values. Windows refuses unsupported formats or unavailable devices; the command reports that failure. Process capture requests the same output format through the process loopback interface.

Only requested samples are written. A final native packet can extend beyond that count; the receipt reports the discarded remainder. The first delivered sample defines file time zero. Buffer setup and command startup are not encoded as invented leading silence.

## Timing, latency and interruptions

Receipts distinguish device sample positions from QPC timestamps in 100 ns units. For physical endpoints, packet device positions must remain continuous. The process loopback interface on the tested system returns no usable device position, so the receipt says so; sample counts and QPC are retained instead. A timestamp error, backward clock, discontinuity after the initial packet, or packet timestamp interval differing from its sample count by more than one millisecond rejects the recording. Initial discontinuity flags are retained explicitly. Accumulated QPC/sample-clock deviation is measured, not silently resampled away.

The reported stream-latency query can be unavailable, particularly for process loopback. It is diagnostic data, not a measurement of full acoustic input/output latency. No automatic physical calibration is claimed. Determine the needed correction from a known reference or calibration and supply it to placement as an exact signed shift.

An invalidated endpoint, changed audio resources or exited target process fails explicitly. Reselect a device/process and choose a new output for a new take. Capture never switches to another microphone or application. Five seconds without a packet, or the requested duration plus ten seconds, triggers a bounded timeout. Packet memory is bounded to one second; normal packets are much smaller.

Rust callers can use `recording::run_controlled` with the existing cancellation/control interface. A cancellation fails without publishing a final WAV. The CLI remains blocking; forced process termination can leave an owned scratch directory containing an unfinished file, whose header is not finalized. Automatic recovery of interrupted takes is not implemented. Normal failure removes owned scratch files. Completed output publication never overwrites another owner's file, including a filename created during capture.

## Place the take on a sequence

`audio.record.place` accepts the saved project snapshot, the receipt's source identity, explicit input root, new asset/clip IDs, an existing audio track, optional child sequence ID, start time, compensation and collision policy:

```json
{
  "command":"audio.record.place",
  "project":"<actual project snapshot object>",
  "source":{"path":"C:\\DEV\\CutboltData\\recordings\\take-001.wav","bytes":5760044,"sha256":"<actual file digest>"},
  "input_root":"C:\\DEV\\CutboltData\\recordings",
  "asset_id":"take-001",
  "clip_id":"voice-001",
  "track_id":"dialogue",
  "sequence_id":null,
  "start":{"num":12,"den":1},
  "compensation":{"backward":true,"amount":{"num":1,"den":50}},
  "collision":"reject"
}
```

Replace placeholders with the actual snapshot, identity and track IDs. Here the explicit 20 ms correction advances placement from 12 to 11.98 seconds. `backward:false` delays it. Both start and correction require exact sample alignment. A correction before sequence zero rejects; the engine does not trim the recording implicitly. The whole take must fit the existing sequence duration. Extend that duration deliberately through the normal track operation when needed.

The result includes `operations`, the candidate `project`, uncompensated/corrected start and source PCM identity. It writes no files and changes no session. Preview and apply the returned operations with the existing `session.preview` / `session.apply` revision and request-ID rules. Track locks, transitive child locks, collisions and source limits retain their existing behavior. Retry, undo and restore operate on placement while the captured WAV remains untouched.

Native audio tracks now accept classic or supported extensible 48 kHz stereo PCM16 WAV files directly, with exact source trims and existing nested audio behavior. Streaming inspection rejects malformed headers, ambiguous layouts, duplicate chunks, more than 128 chunks, metadata over one MiB, files over two hours and changed sources. Audio-only WAV cannot provide video. Scaled video previews retain the original WAV and do not require a video proxy for it. Sequential AV clips retain their existing reference profile.

## Verification

```powershell
cargo build --locked
python -X utf8 tests/recording.py --output C:\DEV\CutboltData\recording-check
```

The test compiles the original `recording_fixture` example and selects only its own synthetic playback processes. Its two short quiet-tone captures test 44.1/48 kHz playback conversion, distinct stereo frequencies and exclusion of a second application's signal. No microphone or unrelated application's audio is captured. The 15-minute sustained fixture uses a muted private playback session; it compares the entire PCM stream against the silence/dither limit, checks exact counts/timing and requires peak working set below 128 MiB. A shortened development run (`--native-seconds 3`) deliberately omits the sustained-capture evidence ID.

Original pulse fixtures expose both signs of latency correction; every rendered timeline sample is compared independently. Acceptance also covers real process exit/reselection, saved revisions/retries/undo, nested audio, previews/ranges, malformed sources and protected originals/outputs. Separate Rust fixtures inject device/packet errors, cancellation, a publication race and failed writes. A two-hour streaming-writer fixture checks all 1,382,400,000 PCM bytes against an independently generated digest without allocating that duration. Physical microphone/acoustic latency and manual hardware unplug tests are not claimed by the isolated process fixtures.

Implementation uses public [shared-stream initialization and conversion](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudioclient-initialize), [capture packet contracts](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudiocaptureclient-getbuffer) and [process-specific activation](https://learn.microsoft.com/en-us/windows/win32/api/mmdeviceapi/nf-mmdeviceapi-activateaudiointerfaceasync). The exact external binding versions/licenses are in the [dependency ledger](DEPENDENCIES.md). No third-party implementation, audio asset or device driver is bundled.
