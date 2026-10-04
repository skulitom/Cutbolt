# Local asset registry and relinking

Version-1 project assets may carry optional `metadata` and `identity`. Empty metadata and absent identities are omitted when serialized, preserving old snapshots. Metadata belongs to the project; media files are never tagged, moved or rewritten. The existing saved-session commands persist these fields with revision checks, retry receipts, undo/history and `modified_assets` diffs.

Assets can also hold an optional identity-bound `proxy` for lower-resolution previews. Full-quality relinking retains the proxy association when the source content is unchanged. Proxy generation, selection, status and separate proxy relinking are described in [PROXIES.md](PROXIES.md).

## Organize and search

`media.metadata` replaces one asset's complete metadata in a `timeline.apply` or `session.apply` operation:

```json
{
  "op": "media.metadata",
  "asset_id": "source",
  "metadata": {
    "title": "Interview opening",
    "bin": ["Series", "Episode 1"],
    "tags": ["approved", "dialogue"],
    "fields": {"note": "Use the opening answer"}
  }
}
```

Fields default to empty. Limits per asset: 4,096 UTF-8 bytes total, 1,024-byte title, eight bin components, 32 unique tags, 32 custom fields. Bin/tag/field names are nonblank and at most 128 bytes; field values are at most 1,024 bytes. NUL is rejected. Tags are exact and case-sensitive. Bins are ordered component paths; prefix searches include descendants without confusing similarly named siblings.

`registry.search` takes `project` and a `query` object with optional `text`, `bin`, `tags`, `offset` and `limit`. All filters must match. Text is a substring search after Unicode lowercase conversion across ID, path, title, bins, tags and custom field names/values; it is not locale-specific collation. Results sort by asset ID. `limit` defaults to 50 and permits 1-200; `next_offset: null` ends pagination. Search does not read source files. It also reports project-wide groups sharing an exact bound identity; duplicate content remains separate assets, while duplicate asset IDs fail. Asset metadata survives JSON and saved-session round trips.

## Bind, inspect and relink

All filesystem commands require an existing absolute `input_root`, absolute source/candidate paths and resolved containment inside that root. Roots may be changed when a project and its media move. No directory scan or filename guess is performed.

| Command | Additional fields | Behavior |
| --- | --- | --- |
| `registry.bind` | `project`, `expected_revision`, `input_root`, `asset_ids` | Hash 1-1000 selected unique assets and bind `{sha256,bytes}`. Returns `project` and `operations`; does not save them. |
| `registry.status` | `project`, `input_root` | Return per-asset `online`, `changed`, `missing`, `unbound` or `unavailable` plus observed identity or diagnostic. |
| `registry.relink` | `project`, `expected_revision`, `input_root`, `asset_id`, `candidates` | Compare 1-1000 explicit candidate paths against the bound identity. Return a new snapshot and operations only for a unique match. |

Bind while originals are available. Relinking an unbound asset fails with `IDENTITY_REQUIRED`; an established identity cannot be replaced. A deliberate new source should be registered as a new asset. Canonically repeated candidate paths count once, but two separate files with identical content are ambiguous: `AMBIGUOUS_MEDIA` requires selecting one explicitly. No match gives `IDENTITY_MISMATCH`. Wrong revisions and invalid roots/paths fail without modifying saved state or files.

For a saved project, get its current revision, call bind/relink, inspect the returned proposal, then pass its `operations` to `session.preview` or `session.apply` against that original revision. The returned snapshot is convenient for caller-owned projects. Pure `media.bind` and `media.relink` operations can also be constructed by a client: they express metadata intent and do not perform I/O. Rendering and frame previews independently enforce bound identities, so a fabricated or stale path change cannot silently render different content. Existing unbound projects retain their previous source-inspection behavior. Content hashes are observations, not file locks; sources are rechecked by the renderer before publication.

## Bounds and evidence

The project model supports 1,000 assets and 1,000 clips. Validation indexes assets for clip lookup. The current renderer still accepts only its documented 64-clip reference profile; the 1,000-clip acceptance claim concerns inspection and editing operations. Requests remain capped at 4 MiB, so text-heavy projects may reach that limit before the item-count limit.

`python -X utf8 tests/registry.py --output C:\DEV\CutboltData\new-registry-test` generates original media and a serialized project, moves/reloads those fixture files, verifies relinked video/audio and preservation of the transported original project, checks ambiguous/wrong identities and saved undo/restoration, and exercises paged search on 1,000 assets. The Windows process fixture requires validation and a 1,000-operation edit batch each below five seconds, with peak working set below 256 MiB. Exact measurements are recorded in `verification/latest.json`; these are bounded acceptance thresholds, not a claim about every machine or 1,000-clip rendering.
