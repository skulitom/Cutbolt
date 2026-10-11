# Installed fonts

`fonts.find` reads installed font files in place and returns content identities for text scenes,
captions and template font parameters. It does not install or copy fonts. Rendered output contains
rasterized glyphs; the font's licensing remains the user's responsibility.

```json
{"command":"fonts.find","limit":32}
```

An optional `family` is a case-insensitive substring of a family or full name. Results are sorted
by canonical file path. Each `fonts` entry contains `identity` (`path`, `sha256`, `bytes`),
`families`, `full_names`, `styles`, `usable` and `reason`. Follow `next_offset` by passing it as
`offset`; null means the last page. `limit` defaults to 32 and accepts 1–128.

Pass a usable entry's `identity` directly in a text object's `fonts` array, a caption layout's
`fonts`, or a template's `fonts` parameter/default. These absolute font identities may be outside
the media input root, but must remain inside an OS font folder on every read. Relative font
identities still resolve inside `input_root`. Both forms also accept `{ "path": ... }` alone;
the engine fills the content identity and reports it in `resolved_identities`.

A font entry may instead be a string naming an **exact** family or full name, ignoring case.
It must match exactly one file, and that file must be supported. The engine never silently selects a Regular face from
a family that also has Bold or Italic files. `UNSUPPORTED_FONT` reports matching files and points
to `fonts.find` when the name is absent, ambiguous or unsupported. Names resolve before queueing;
the saved job holds the file's exact content identity. A result's `resolved_identities` includes
the original `font_name` and resolved file. Reuse the explicit identity for reproducible edits
and use the original ticket to follow a queued job.

The OS folders are:

- Windows: `%WINDIR%\Fonts` and `%LOCALAPPDATA%\Microsoft\Windows\Fonts`.
- Linux: `/usr/share/fonts`, `/usr/local/share/fonts` and `~/.local/share/fonts`.
- macOS: `/System/Library/Fonts`, `/Library/Fonts` and `~/Library/Fonts`.

Discovery reads `.ttf`, `.otf`, `.ttc` and `.otc` files, recursively within those folders. Directory
symlinks are not followed, and file links escaping the font folders are excluded. Rendering
currently supports single, static, monochrome TrueType outline fonts. Collections (names of the
first face only), CFF outlines, variable/color fonts and AAT layout are reported as unsupported;
discovery does not add rendering support for them. Malformed or oversized files appear in
`skipped` (first 32 reasons) and `skipped_count`. Missing folders are normal; unreadable folders
fail the scan. Windows and Linux have synthetic acceptance coverage; macOS folder selection has
not been exercised on a macOS host.

Each scan is bounded to 8,192 font files, 2,048 directories, 131,072 directory entries, 8 MiB per
font and 512 MiB read in total. Existing rendering bounds still apply: at most eight files and
32 MiB of font data in one scene. Files are content-checked again before publication. This
exception applies only to font fields; scene images and audio retain their input-root rules.

`fonts.find` is a read-only direct tool in the full MCP catalog, and is available through
`cutbolt_run` in the default core catalog. It needs no workspace or input root. See
[text rendering](GRAPHICS.md) for layout and fallback rules and [Unicode text](UNICODE_TEXT.md)
for the optional shaping profile. `tests/fonts.py` uses original generated outline fonts in
isolated per-process font folders; it never installs a font or copies a system font.
