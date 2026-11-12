# Disk aggregation: one library across several drives

Status: phase 1 done (2026-11-12); phase 2 next. Work happens in the
`drive-aggregation` worktree, separate from release work. Written 2026-11-12, when Diamond had one 2 TB drive
(`/Volumes/Alpha`, about 620 GB free) and a second 2 TB drive was expected.

## Decision

Torrbaby spans drives at the application layer. **macOS never joins the drives**:
no AppleRAID concatenation, no Fusion or APFS container across disks. Each drive
stays an ordinary independent volume with its own `Media` folder. Then:

- If one drive fails or is unplugged, only that drive's titles are affected, and
  only until it is back.
- A drive can be added without erasing or copying anything. Alpha stays exactly
  as it is.
- Any drive can be read on its own by any Mac.

The goal is the fault tolerance described in the processing rules: degrade, don't
fail. A missing drive never deletes records, loses watch history, or blocks the
other drives.

## Today: one set of roots

`Config` (server/config.rs) has exactly one `source` (Downloads), one `movies` and
one `tv`. About 80 references across 13 server files assume them. The main ones
are listed below.

| Area | File | What assumes one root |
| --- | --- | --- |
| Library scan | scanner.rs | walks `movies`, `tv`, `source`; `library_ready` = under movies/tv; Junk exclusion |
| Organizer | organizer.rs | new torrents download to `source`; completed ones move to `tv`/`movies` with `torrent_set_location move:true` |
| File actions | library_files.rs | managed-root checks; Junk is `<root>/Junk/<hash>` and must be on the same volume |
| API | api.rs | path safety (`checked_path`, "outside managed roots"), status/settings, `/v1/storage` |
| Cloud downloads | cloud.rs | destination is `<movies or tv>/<title>` |
| Trials | trials.rs | `<source>/.torrbaby-trials` and the winner's destination |
| Storage report | volumes.rs | already reports a **list** of volumes (groundwork) |

Asset IDs are random UUIDs, not path hashes, so a file can move between drives
and keep its ID, its watch history and its subtitle jobs.

## Target model

### Configuration

Add a `drives` list. When it is absent, the old `source`/`movies`/`tv` fields
become drive 1, so existing configs keep working unchanged.

```json
"drives": [
  {"id": "alpha", "media": "/Volumes/Alpha/Media", "accept_new": true},
  {"id": "beta",  "media": "/Volumes/Beta/Media",  "accept_new": true}
]
```

- Each drive's roots are `<media>/Downloads`, `<media>/Movies` and `<media>/TV`.
  An optional per-drive override keeps Alpha's existing paths exact.
- `id` is stable and never derived from the mount path, which can change
  (e.g. `/Volumes/Beta 1`). It is recorded in a marker file on the drive,
  `<media>/.torrbaby-drive.json`, so Torrbaby recognises the drive even if it
  remounts somewhere else.
- `accept_new: false` keeps a drive read-only for placement. Use it for one that
  is full or being retired.
- Reserve: never place on a drive below a free-space floor (default 50 GB, the
  level the clients already show in orange).

### A single abstraction: `volumes.rs` grows into `drives.rs`

Every caller asks this module, and none of them touches the config roots
directly:

- `roots(role)`: every root of a role across all drives (for scanning, path
  checks and lookups).
- `drive_of(path)`: which drive a path is on, if any.
- `is_managed(path)`: replaces the three-root checks in api.rs, library_files.rs
  and scanner.rs.
- `place(kind, title_hint, bytes)`: which drive a new download goes to.
- `online(drive)`: mounted, marker matches, writable.
- `report()`: what `/v1/storage` returns today, one row per drive.

Phase 1 is mostly mechanical: replace each `config.source/movies/tv` use with
these calls.

### Placement rules

1. **A torrent never crosses drives.** It downloads into the chosen drive's
   `Downloads` and is organized into that same drive's `Movies`/`TV`. Moving
   within a volume is a rename: instant, safe, and Transmission keeps seeding.
   A move across volumes would copy gigabytes while seeding, so it is never
   automatic.
2. **Episodes follow the show.** A new episode or season of a show already in
   the library goes to the drive with the show's newest season, when there is
   room. Otherwise it goes to another drive. The catalog already merges assets
   by entity, so a show split across drives still appears once.
3. **Otherwise use the most free space**, among drives accepting new downloads
   and above their reserve.
4. **Size is known up front for torrents** (metainfo) and cloud jobs
   (`total_bytes`), so placement can refuse a drive that would overflow. For a
   magnet whose size is not yet known, the choice is made when metadata arrives,
   by moving the torrent's data directory before any data is written.
5. The trials directory and Junk are per drive. Junk already requires the same
   volume.

### When a drive is missing

- **Never mark a missing drive's files deleted.** The scanner must tell
  "file gone" apart from "drive gone": if a drive's marker is unreachable, skip
  that drive entirely and keep its assets' last known state.
- Its assets report `available: false` with a reason. Clients already grey out
  unavailable files.
- Its torrents are paused, or left to Transmission's own error state. They are
  not removed, re-added or re-downloaded.
- Needs attention gets one item, "Drive Beta is not connected", instead of one
  error per file. The processing queue skips that drive's files without
  counting failures, because the cause is transient.
- Placement skips the drive. When it comes back, a rescan restores everything,
  with no user action.
- A drive that is full (below its reserve) stops accepting new downloads and
  shows a warning; existing files keep working.

### Moving titles between drives (explicit only)

"Move to drive…" is for a show or a movie, chosen by the user. Use it to balance
drives or to empty one before retiring it.

- If the files belong to a torrent, use `torrent_set_location move:true` and let
  Transmission copy and verify. Otherwise copy, verify size and hash, switch the
  asset's path, then delete the original. Never delete before verifying the copy.
- A file being played or processed is skipped or waited for, never moved
  underneath an open handle.
- Progress uses the existing library operations machinery, the same as Junk and
  delete.
- Asset IDs are unchanged, so history, subtitles and credits follow the file.

## Phases

1. **Abstraction, no behaviour change.**
   - Add `drives` config with the backwards-compatible default, and `drives.rs`.
   - Move every root reference behind it; keep exactly one drive in production.
   - Acceptance: every test passes; Diamond's library, scan results and
     organizer actions are identical before and after, compared on a copy of
     the database.
   - Done: `server/drives.rs` (grown from `volumes.rs`) and the `drives` config.
     `torrbaby scan-report` prints what a scan would record and what the
     organizer would move, changing nothing. Run before and after on Diamond
     against a backup of the database and one saved Transmission snapshot, the
     output was identical (1,539 assets). With every torrent pointed at
     Downloads, the organizer's plans and refusals were identical too.
2. **Read across drives.**
   - Scan every drive; path safety accepts any managed root.
   - Missing-drive handling (no false deletions), the drive markers, and a
     Needs attention item for a missing drive.
   - Acceptance: a second drive added in preview with a few test files shows
     them in the library. Unmounting it marks them unavailable, deletes nothing,
     and remounting restores them.
3. **Placement.**
   - New torrents and cloud jobs choose a drive by the rules above.
   - The organizer stays within a drive. Episodes follow the show.
   - Per-drive Downloads, Junk and trials directories.
4. **Move to drive and polish.**
   - The explicit move action; the per-drive report already in clients.
   - A drive list in Settings → Server: name, free/total, accept new downloads,
     reserve.
   - Strings in EN and 中文 per AGENTS.md.

## How to test without a second drive

- A disk image stands in for the second drive:
  `hdiutil create -size 20g -fs APFS -volname Beta beta.sparseimage`, then
  attach and detach it to simulate plugging in and unplugging.
- Use the Rust preview (`serve` with an alternate port and a copy of the
  database). Never point a preview at the live database, and never run two
  organizers against the live library.
- Unit tests use temporary directories, as `volumes.rs` does. A "missing drive"
  test removes the marker or the directory and asserts no asset is marked gone.

## Decisions (answered 2026-11-12)

- A new show or film goes to the drive with the most free space, which balances
  the drives over time. Episodes still follow their show.
- A new drive is not used until it is added in Settings. Torrbaby never adopts a
  mounted volume on its own.
- Drives stay invisible to everyone except when administration needs them: names
  appear in Settings → Server and, only while a drive is missing, in Needs
  attention.
