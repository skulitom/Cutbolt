# Portable media and checked project history

Native snapshots retain schema version 1. The editing-session store adds an explicit version-1 to version-2 migration, checked consistent backups and recovery into an unused database location. Original media is never copied or changed by these commands. Full verification awards I01 extended; see the [generated tracker](PROGRESS.md).

## Relative media paths

Native asset and proxy paths may be absolute or relative normal components under the command's explicit absolute `input_root`. Rendering, native tracks, nested sequences, previews, proxy generation, registry checks, synchronization and cache dependencies resolve these paths through the same canonical root check. Empty paths, parent traversal, drive-relative paths, embedded NULs and relative colon syntax reject. Canonical paths outside the root reject, including filesystem links that escape it. Existing standalone file commands retain their documented path contracts.

`project.portable` accepts `project`, `expected_revision` and `input_root`. It verifies every asset and attached proxy against its bound content identity, then returns a proposed `project` and `media.paths` operations using relative forward-slash paths. Bind unbound originals first through the existing registry workflow. The command requires one common root containing originals and attached proxies, writes no files and changes no saved state. If all paths already match, the operations array is empty and the revision remains unchanged.

Apply a nonempty operations array through `session.preview` and `session.apply` at the original revision. Each `media.paths` operation supplies `asset_id`, `path` and optional `proxy_path`. It changes paths only, requires a bound original identity, preserves proxy/source identities and uses the existing atomic edit, retry, undo and history behavior. Direct operations validate path syntax; subsequent media use still checks identities and the explicit root. The inspection command supplies content-checked proposals.

After copying the media directory through a separate explicit file workflow, supply the new directory as `input_root`. Relative paths work without changing the saved snapshot. File identities prevent a same-named replacement from being used silently. Proxy previews still require a matching proxy for every used video asset. Full-quality exports always use originals.

Earlier absolute paths remain in earlier history revisions. Making the current revision portable does not rewrite that immutable history. Restoring an older absolute-path revision can require ordinary content-checked relinking if its former directory is unavailable. Creating a new session from a portable snapshot makes its subsequent revisions portable from the start.

## Check and migrate

| Command | Fields | Behavior |
| --- | --- | --- |
| `session.check` | `store_root` | Check a consistent view of every project's history and report heads, counts, schema and migration requirement |
| `session.migrate` | `store_root` | Explicitly migrate a valid version-1 store in one writer transaction; already-current stores return `changed:false` |

The root must be an existing absolute directory. Version 1 remains readable without migration, including successful request replay. New writes into an older store return `MIGRATION_REQUIRED`. Newly created stores use version 2. Queue storage has its own version and migration; it is unaffected by these editing-store commands.

Checks include SQLite consistency and foreign-key checks, expected database objects, contiguous revisions and current heads, snapshot/receipt checksums and identities, reconstructed before/after diffs, request counts, action types and undo/restoration pointers. They validate stored editing data without requiring media to be online. A corrupt or unsupported store rejects; these commands do not guess repairs.

Version 2 adds a checksum binding each revision's metadata, source snapshot checksum and request/receipt fingerprints. Saved reads, history and replay check the relevant records, while `session.check` and backup validation inspect the entire history. Migration preserves the original snapshot JSON, receipts, request fingerprints, revisions, heads and undo behavior. An error rolls back both the new table and version update. An interrupted transaction recovers through the existing SQLite journal. A successful migration is idempotent on retry; older engine builds reject version 2.

Checksums detect accidental corruption and inconsistent records. They are not signatures or protection against someone deliberately rewriting both data and checksums. Keep the store on trusted local storage that honors SQLite locking and flushes. Never remove a journal to clear a lock.

## Backup and recovery

`session.backup` takes `store_root`, `output_root` and `output`. Output must be a new `.sqlite3` file under an existing absolute output root. It creates a consistent SQLite copy, validates its entire history, flushes it and publishes it through a no-overwrite hard link. The receipt includes its content identity and the project heads actually captured. Writers may advance the live store while backup runs; the receipt describes the copied consistent state. Current stored snapshots and receipts remain unchanged.

```json
{
  "command":"session.backup",
  "store_root":"C:/local/session",
  "output_root":"C:/local/backups",
  "output":"C:/local/backups/edit-001.sqlite3"
}
```

`session.recover` takes `source` (path, byte count and SHA-256), `input_root` and `store_root`. Source paths may be absolute or relative normal components under the input root. It checks the source identity, streams a separate copy, validates every history record and publishes `projects.sqlite3` only when complete. The destination root must already exist and must contain no project database, rollback journal, WAL or shared-memory sidecar. Other files are retained. Existing databases are never replaced, including after a repeated recovery request.

Recovery preserves the backup's schema version and does not migrate it. Inspect `session.check` and explicitly migrate a recovered version-1 store before editing. Exact archived requests can replay their original receipts, and undo/restoration retain their prior targets. Media remains separate: restore its directory structure and use its explicit root.

Backup and recovery accept databases up to 256 MiB; they stream file copies without loading a whole database into memory. Full-history checking holds one current snapshot and predecessor per project rather than materializing every snapshot together. Checking very long histories still takes work proportional to their contents. This is not a history-pruning feature.

Normal failures clean only their own temporary file. Abrupt termination can leave a uniquely named `.cutbolt-history-*.sqlite3` sibling for explicit cleanup. A publication crash exposes either no destination or a complete checked destination; an existing destination is never overwritten on retry. These process-interruption checks do not claim power-loss, network-share or hostile-writer guarantees. Filesystems must support hard links. Do not replace or manually copy an actively written store; use the checked backup command.

## Acceptance and provenance

The original `tests/portable_projects.py` fixture first uses an external retained engine build to create a genuine version-1 store. It verifies read-without-migration, rejected pre-migration writes, exact logical table preservation, request replay and old-engine rejection of version 2. Independent decoded RGB/PCM checks cover moved relative originals, proxy previews, edits, undo and recovered history. Concurrent native writers produce separately checked consistent backups. Corrupt snapshots, metadata, heads and identities, occupied destinations, path boundaries and typed MCP schemas reject as specified.

Six substantive Rust checks exercise real child-process exits around migration, backup and recovery commits/publication, metadata corruption, a genuine SQLite full-database migration failure and rejected corrupt legacy history. The migration fault rolls back the schema and all original rows. This does not award the separate E06 render-output failure checkpoint.

Full verification requires `CUTBOLT_LEGACY_STORE_ENGINE` pointing to the retained project-owned version-1 writer, alongside the existing reference environments. The fixture verifies the produced schema and records the executable's actual size/hash. The tested build and provenance are in [the dependency ledger](DEPENDENCIES.md). It is a development reference only; normal engine use has no legacy-binary dependency.

The backup implementation uses the selected SQLite version's documented [VACUUM INTO interface](https://www.sqlite.org/lang_vacuum.html) for a consistent copy. Path policy, validation, checksums, migration, publication and fixture code are original. No third-party implementation, generated database or media is included in the repository.
