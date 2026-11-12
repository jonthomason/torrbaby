# Android screensaver: slow scenic videos

Status: planned, not started. Written 2026-11-12. Start once the release
session's current Android work has shipped. The JMGO comes first; the TCL waits
until the screensaver has been in use on the JMGO for a while, and until we have
working ADB on the TCL.

## Why

The JMGO's own screensaver (`com.jmgo.os.dream`) showed ads and downloaded
content in the background. It has been removed for user 0 (see "JMGO cleanup"
below), and the projector currently uses Android's plain colour screensaver.
The goal is something like the Apple TV's aerial screensaver: slow,
high-quality video, crossfading between clips, with the time shown.

## What the user sees

- After the normal idle time, Torrbaby's screensaver starts on the projector.
- It plays scenic clips in 4K (or the best the device decodes), one after
  another, with a slow crossfade (about 2 s) between them.
- It shows a small, dim clock in a corner, and the clip's place name in the
  clip's last seconds (for example "Earth from the ISS · Pacific at night"),
  with EN and 中文 text per AGENTS.md.
- The order is shuffled, and recently shown clips are not repeated, so it does
  not start on the same clip every time.
- Any key on the remote wakes the projector back to where it was, as Android's
  screensaver framework already does.
- There are no network requests while the screensaver runs. It never stutters
  on Wi-Fi, and nothing leaves the house.

## Footage

Use only footage we are allowed to use. **Not Apple's aerial videos**: they are
Apple's copyrighted content and not licensed for other apps.

- **NASA**: Earth from the ISS and similar footage. It is public domain, and
  the closest match to Apple's space clips.
- **Freely licensed stock**, such as Pexels and Pixabay. Their licences allow
  free use without attribution. There are plenty of slow 4K drone shots of
  coastlines, mountains, forests, cities at night and Chinese landscapes.
- Record the source, licence and original URL of every clip in the manifest
  (below), so the collection's provenance is always known.

Starter set: 20–30 clips, 1–2 minutes each, HEVC 4K at a sensible bitrate
(about 15–25 Mb/s), with no audio track. That is roughly 15–25 GB.

## Where the clips live

- **On the projector itself.** The JMGO has about 50 GB free, so it plays from
  local storage with no streaming at all.
- **Diamond is the source of truth.** A folder on Alpha,
  `Media/Screensaver/`, holds the clips plus a `manifest.json`. The server
  exposes the manifest and serves the files with range support, as it already
  does for media.
- **The Android app syncs.** When the app is idle on Wi-Fi and the screensaver
  is not running, it downloads missing clips into its own app storage
  (`getExternalFilesDir`, so no storage permission is needed), checks each
  file's size and SHA-256 against the manifest, and deletes clips that are no
  longer listed. It keeps within a storage budget (default 25 GB, never below
  10 GB free on the device).
- If no clips have synced yet, the screensaver uses the library's backdrop art
  instead: slow Ken Burns pans with crossfades, using images Torrbaby already
  has.

`manifest.json`, one entry per clip:

```json
{"id": "iss-pacific-night", "file": "iss-pacific-night.mp4",
 "title": {"en": "Earth from the ISS · Pacific at night", "zh-Hans": "国际空间站俯瞰 · 太平洋夜色"},
 "duration": 94.0, "bytes": 212345678, "sha256": "…",
 "width": 3840, "height": 2160, "codec": "hevc",
 "source": "NASA", "licence": "Public domain", "url": "https://…"}
```

## Android implementation

- A `DreamService` in the Torrbaby Android app (`TorrbabyDream`), declared with
  `android.service.dreams.DreamService` and the `BIND_DREAM_SERVICE`
  permission.
- Two ExoPlayer (Media3) instances alternate, so the next clip is prepared
  before the crossfade starts. Use the device's hardware decoder, as playback
  already does.
- The clock and the place name are plain text overlays. Keep the layout safe
  for projector overscan.
- Settings in the Android app: screensaver on or off, the storage budget, and
  "sync now". Show sync progress there, not in the screensaver.
- Making it the default needs ADB once (no root):
  `settings put secure screensaver_components com.torrbaby.tv/.TorrbabyDream`,
  plus the same for `screensaver_default_component`. We have already confirmed
  that the JMGO firmware keeps this setting across restarts. The idle timeout
  is the projector's own setting.

## Server

- Add a configured screensaver folder, defaulting to `<media>/Screensaver` on
  the first drive (see `docs/disk-aggregation.md` for multiple drives).
- `GET /v1/screensaver` returns the manifest.
  `GET /v1/screensaver/files/{id}` serves the file with range support and
  authentication, like media downloads.
- A small script, `scripts/screensaver_add.py`, takes a downloaded clip and its
  metadata: it transcodes if needed (HEVC, no audio), computes the hash, and
  adds the manifest entry.

## Acceptance (JMGO)

- After idle, the screensaver starts within the normal timeout, plays local
  clips, crossfades smoothly, and wakes on any key.
- There is no network traffic from the screensaver (check `netstats` before and
  after an hour of running).
- With the projector's Wi-Fi off, it still plays.
- Sync recovers from interruptions (a restart mid-download) and stays within
  the budget.
- EN and 中文 text has been checked on the projector itself.
- It keeps running for at least a week before it goes on the TCL.

## JMGO cleanup so far (2026-11-12, no root)

- `pm disable-user` does **not** survive a restart: the firmware re-enables
  apps at boot.
- `pm uninstall -k --user 0 <app>` **does** survive. Undo it with
  `cmd package install-existing <app>`.
- Removed this way: `fusion.android.tv.demo` (retail demo), `com.jmgo.os.guide`,
  `com.jmgo.os.gamecenter`, `com.jmgo.browser`, `com.jmgo.helpcenter` and
  `com.jmgo.os.dream` (the ad-filled screensaver). Screensaver set to
  `com.android.dreams.basic/.Colors`.
- `com.jmgo.ai.voice` (the voice assistant, a system-uid app kept updated by
  `com.jmgo.update`) is restored by the firmware at boot even after removal.
  Only DNS-level blocking (iFlytek, Umeng) can limit it without root.
- Kept on purpose: iQiyi, Tencent Video, CIBN, screen mirroring
  (`com.jmgo.holacast.x`), and every projector-hardware app (`hippo`, `spider`,
  keystone, focus, brightness, colour, eye protection, remote).
