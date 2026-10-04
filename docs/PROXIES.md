# Proxy editing

Proxies are lower-resolution preview variants linked to a full-quality editing asset by SHA-256 and byte count. They preserve its complete frame clock and PCM audio. Generate them with the blocking CLI/library, attach them through a saved editing session, and select a preview scale. Frame/range previews use that selection. **Final `render.plan`, `render.run` and queued `render.start` always use full-quality assets**, even while proxy previews are selected.

## Source and generation contract

Start with identity-bound assets in the 25 fps FFV1/bgr0, 48 kHz stereo PCM reference profile. Mixed-rate or supported compressed sources first use the explicit [media-conform workflow](CONFORM.md). That conversion establishes a full-resolution editing master and exact timestamp selection; generating proxies does not choose a different frame clock. The original native files remain untouched. Final export uses these full-resolution masters, with the conversion's declared 25 fps behavior.

`proxy.generate` accepts `project`, `expected_revision`, `asset_id`, `scale`, `input_root`, `output_root` and `output`. It verifies the complete source, requires its declared asset duration to equal decoded duration, and generates an unused `.mkv` file. Scale is a dimension divisor of 2, 4 or 8. Both project dimensions must divide exactly. Video uses nearest sampling at the top-left pixel of each scale-sized block; all audio sample values remain unchanged. There is exactly one proxy frame per master frame.

Generation inherits media-conform bounds: at most 60 seconds, a 64 MiB encoded source, 36,000 source frames and 4 GiB decoded RGB, plus the documented dimension/audio/tool limits. Output and parent roots must already exist. Inputs and existing outputs are never overwritten. Each output's decoded pixels/samples are checked before publication; normal failure cleans owned scratch files. Generation is synchronous, outside the render queue, and can leave a completed file if the caller later loses a response or encounters a session revision conflict. Retain the receipt and use a new output path for an intentional regeneration.

The receipt includes `proxy`, an updated caller-owned `project`, and proposed `operations`. It does not mutate the saved session. To attach, submit its operations through `session.apply` against the original revision. Multiple generated attachments from the same snapshot can be combined in one batch. Each asset holds one proxy binding at a time; replacing or removing a binding leaves all files intact.

## Saved operations

| Operation | Additional fields | Behavior |
| --- | --- | --- |
| `media.proxy.attach` | `asset_id`, `proxy` | Store the generated binding; require matching full-quality source identity and complete duration |
| `media.proxy.detach` | `asset_id` | Remove the binding without deleting files |
| `media.proxy.relink` | `asset_id`, `path` | Replace only the proxy path; runtime identity checks remain required |
| `preview.proxy` | `scale`: `2`, `4`, `8` or `null` | Select that scale for previews; `null` selects full-quality sources |

Bindings contain `path`, `identity` (`sha256`, `bytes`), `source_identity` (`sha256`, `bytes`), `scale` and `frames`. They are optional `asset.proxy` fields in version-1 snapshots. `project.preview_scale` is optional and omitted for full-quality previews, so old snapshots keep their behavior. An active proxy preview requires every referenced timeline asset to have a matching-scale proxy; unused assets do not need one. Missing, changed, wrong-size or wrong-duration proxies fail explicitly. No automatic fallback is performed.

The preview selection affects no clip ranges, sequence dimensions or final-export settings. All ordinary saved editing, retry, revision conflict and undo/restore behavior remains available. `session.preview` and receipts show `changes.preview` with `scale_before`/`scale_after` when the choice changes; attachment/path changes appear in `modified_assets`. Preview receipts declare `source_quality`, `preview_scale`, dimensions and actual source identities. Final export receipts/plans declare `source_quality: "original"` and identify full-quality sources.

Explicit gap items need no proxy binding. They remain black and silent at the selected preview dimensions, with unchanged frame/sample duration. A gap frame receipt has `gap: true` and null source/source-frame fields.

## Offline media and relinking

`proxy.status` takes `project` and `input_root` and reports `online`, `changed`, `missing`, `unavailable` or `no_proxy` per asset. This checks file identities, not full decoding. Use `registry.status` separately for full-quality sources.

`proxy.relink` takes `project`, `expected_revision`, `input_root`, `asset_id` and 1–1000 explicit `candidates`. A unique matching identity returns `project` and proposed `operations`; no file moves or saved edits occur. Nonmatching and ambiguous candidates fail. Persist proposals through `session.apply`. Full-quality source relinking continues to use `registry.relink`; changing its path retains the association because content identity is unchanged.

Proxy previews validate the proxy content and recorded source association without reading the full-quality file. They therefore work while full-quality media is offline. This does not certify that an independently modified or unavailable master still matches its recorded identity. Full-quality previews/exports enforce that identity when they read it, and fail if it is missing or changed. Export does not fall back to a proxy, and missing proxy files do not block final rendering from valid full-quality media.

CLI/library exposes all commands. MCP exposes `proxy.status` and `proxy.relink`, while existing `session.apply` handles the operations. There is no listening service or runtime network access.

## Runnable fixture

```powershell
cargo build --locked
python tests/proxies.py --output C:\DEV\CutboltData\my-proxy-test
$demo = 'C:\DEV\CutboltData\my-proxy-test'
$project = Get-Content -Raw "$demo\project.json" | ConvertFrom-Json
@{command='proxy.status'; project=$project; input_root=$demo} |
    ConvertTo-Json -Depth 30 -Compress | .\target\debug\cutbolt.exe
```

The retained snapshot deliberately contains an offline proxy after testing that final output still uses full quality. `moved\proxy-a2.mkv` is its relink candidate. The fixture independently checks complete decoded output from original 24 fps, 30000/1001 fps and variable-rate source recipes, all three scales, source-frame boundaries, unchanged PCM, saved switching/replay/undo/detach, moved masters/proxies, ambiguous relinking and synchronous/queued final export. It preserves and verifies all source and proxy bytes through the controlled moves. Codec decoding uses the existing external FFmpeg boundary; selection and spatial references are independent. No speed or general long-form performance claim is made by this fixture.
