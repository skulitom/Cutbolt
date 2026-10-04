# Bounded native-project import

`native.import` accepts an original, content-bound transfer created through an
installed source application's public project interface. The application reads
its own native file; Cutbolt validates the supplied native-file identity, exact
adapter/build acceptance and editorial transfer, then returns a proposed native
snapshot. No proprietary file parser, application binary or adapter is bundled.
The engine does not launch the source application or execute the adapter.

The reviewed local profile is **flat editorial structure**: one explicitly
selected 25 fps progressive, square-pixel sequence, ordered video/audio tracks,
cuts, source ranges and gaps, with explicit 48 kHz stereo media bindings.
Muted tracks remain disabled. Disabled clips require acknowledgement that their
editable definition becomes an empty interval. The returned proposal uses the
existing [public editorial mapping](INTERCHANGE.md) and saved-session workflow.

This is not native appearance or sound-mix equivalence. Native effects,
compositing, color management, clip/track/master processing, channel maps,
source interpretation, proxies, links, bins, labels, markers and display origins
are not translated by this adapter. Its sequence-level and individual-component
reports require explicit acknowledgement. Unselected sequences are reported.
Nested, multicamera, merged, offline, adjustment, template and retimed items,
native transitions and unsupported clocks/layouts block the proposal. A
blocking issue cannot be acknowledged away. Keep the original project and
external capture for semantics not represented in the editing snapshot.

## Preparation and import

1. Select an external original adapter and exact installed build reviewed for
   this profile. Pin the application and adapter files by SHA-256. Have that
   application open a task-owned copy of the native project through its public
   interface, with an explicit sequence selection. Wait for media readiness;
   incomplete or unreadable inventories fail or report blocking issues.
2. Produce a `cutbolt-native-transfer-v1` JSON document and a reviewed
   `native-flat-timeline-v1` acceptance manifest. Recheck the original native
   file, copy, source media, application and adapter identities after capture.
   Keep all application-specific code, native projects and captures external.
3. Supply the native file, transfer and acceptance as three **distinct** bound
   identities, each with a relative normal-component `path`, `bytes` and
   lowercase `sha256`, under an existing absolute `input_root`. Supply an
   existing absolute `media_root`, a new project `id`, and explicit local media
   `bindings` using the same contract as `interchange.import`.
4. Inspect `host_issues`, editorial `losses` and `required_acknowledgements`.
   Resubmit with only the reviewed nonblocking IDs in `acknowledged_losses`.
   Only `ready: true` returns a project. Save it explicitly with `session.create`;
   ordinary preview/apply, retry, history and undo then apply.

The read-only command is available as `cutbolt_native_import` over MCP stdio.
It writes no files and never updates a session. Capability discovery exposes
its limits and provenance model. Native file size is limited to 64 MiB, transfer
JSON to 4 MiB, acceptance to 64 KiB, the matrix to 32 unique exact versions and
the host issue inventory to 1,024. The editorial mapper retains its own 32-track,
1,000-clip and other strict limits; renderer limits remain separate.

## Transfer and trust contract

The transfer declares `protocol`, `profile`, `producer`, a content identity for
`source`, `selected_sequence`, `sequence_count`, `complete`, `width`, `height`,
`frame_rate`, `issues`, and the public OTIO `document`. `producer` contains
`adapter_id`, `adapter_sha256`, `version` and `application_sha256`. An issue has
a unique `host:`-prefixed `id`, `path`, `code`, `message` and `blocking` flag.
More than one sequence requires an `unselected_sequences` issue.

The acceptance manifest has `schema_version: 1`, `profile`, `adapter_id`,
`adapter_sha256`, and `builds`, an array of unique exact `version` plus
`application_sha256` pairs. Unknown fields, duplicate versions/issues,
unaccepted builds/adapters, changed files, incomplete inventories and unknown
or blocking acknowledgement IDs reject. Native source, transfer and acceptance
are rehashed before returning. Local media identities and roots are checked
before a project is proposed.

These are caller-reviewed, hash-bound records, **not cryptographic attestation**
that an arbitrary supplied adapter actually inspected every native semantic.
The caller must trust and review the selected adapter and acceptance manifest.
Cutbolt cannot infer unreported semantics from the native bytes. Supplying
invented transfers is not evidence of compatibility.

## Tested matrix and practical limits

The retained external fixture records **one exact application build**, its
binary SHA-256 and both original adapter components. All other build labels or
binary identities reject unless separately reviewed into an acceptance matrix.
Application-specific version strings, filenames and capture evidence stay in
the private provenance record. No claim covers every release or every native
feature. The tested capture route requires an exclusively owned application
instance; a cold launch worked, an already-running instance did not. A missing
audio-device dialog required dismissal in the background test desktop. This
route is not a verified unattended launcher and is not performed by the engine.

`tests/native_projects.py --fixture <external-fixture.json> --output
<external-new-directory>` replays the three actual content-bound captures:
flat cuts, disabled/muted content with multiple sequences, and blocked nesting.
Original raw RGB/PCM fixtures provide independent full-output expectations;
the converted bound asset is checked against the original source. The test
also exercises typed MCP, saved edits/retry/undo, exact-build and identity
rejection, roots, limits and explicit losses. The full verifier requires
`CUTBOLT_NATIVE_PROJECT_FIXTURE` and rechecks the installed application and
adapter hashes. It **does not freshly launch the application**; that capture
step is separately recorded native integration evidence. Both native-project checkpoints pass the complete verifier against these retained captures and the exact selected build.
