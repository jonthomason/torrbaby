# Projector screensaver: a small standalone app

Status: planned, not started. Written 2026-11-12. **This is not part of
Torrbaby.** It is a separate, very simple Android app with its own project
folder (proposed: `~/src/scenic-screensaver`). This note lives here only
because the JMGO cleanup that led to it happened in a Torrbaby session. The JMGO
comes first; the TCL waits until the screensaver has been in use on the JMGO for
a while, and until we have working ADB on the TCL.

## Why

The JMGO's own screensaver (`com.jmgo.os.dream`) showed ads and downloaded
content in the background. It has been removed for user 0 (see "JMGO cleanup"
below), and the projector currently uses Android's plain colour screensaver.
The goal is something like the Apple TV's aerial screensaver: slow,
high-quality video, crossfading between clips, with the time shown.

## What the user sees

- After the projector's normal idle time, the screensaver starts.
- It plays scenic clips one after another, with a slow crossfade (about 2 s)
  between them, in random order without recent repeats.
- It shows a small, dim clock in a corner. Optionally, in a clip's last
  seconds, it shows the place name from the clip's file name.
- Any key on the remote wakes the projector, as Android's screensaver
  framework already does.
- It has no network access at all: the app does not request the `INTERNET`
  permission. It only plays files already on the device.

## The app

- One `DreamService` plus a tiny settings screen (or none at first).
  Package: e.g. `com.jonthomason.scenic`. Minimum Android 11 (the JMGO's
  version).
- It plays every video file in its clip folder,
  `/sdcard/Android/data/com.jonthomason.scenic/files/clips/`. That is the app's
  own storage, so it needs no storage permission.
- Two ExoPlayer (Media3) instances alternate, so the next clip is prepared
  before the crossfade starts. Use hardware decoding, with HEVC 4K preferred.
- The place name comes from the file name:
  `Earth from the ISS - Pacific at night.mp4` is shown as
  "Earth from the ISS · Pacific at night". An optional
  `Earth from the ISS - Pacific at night.zh.txt` beside it gives the Chinese
  name, shown when the system language is Chinese.
- If the folder is empty, it shows a plain black screen with the clock, never
  an error.
- Interface strings come in EN and 中文, although there are almost none.

## Getting clips onto the projector

There is no server and no syncing. Clips are copied straight to the projector
from the Mac with ADB:

```sh
adb -s 192.168.1.206:5555 push clips/ /sdcard/Android/data/com.jonthomason.scenic/files/clips/
```

A small script in the app's repo (`push_clips.sh`) does this, skips clips
already there, and lists what is on the device. Adding or removing clips is
just copying or deleting files. The JMGO has about 50 GB free; keep the
collection under about 25 GB.

## Footage

Use only footage we are allowed to use. **Not Apple's aerial videos**: they are
Apple's copyrighted content and not licensed for other apps.

- **NASA**: Earth from the ISS and similar footage. It is public domain, and
  the closest match to Apple's space clips.
- **Freely licensed stock**, such as Pexels and Pixabay. Their licences allow
  free use. There are plenty of slow 4K drone shots of coastlines, mountains,
  forests, cities at night and Chinese landscapes.
- Keep a `SOURCES.md` in the clip folder on the Mac: each clip's source,
  licence and original URL.

Starter set: 20–30 clips, 1–2 minutes each. Prepare each on the Mac as HEVC 4K
at about 15–25 Mb/s with the audio removed (`ffmpeg -an`). That is roughly
15–25 GB.

## Making it the screensaver (once, over ADB, no root)

```sh
adb shell settings put secure screensaver_components com.jonthomason.scenic/.ScenicDream
adb shell settings put secure screensaver_default_component com.jonthomason.scenic/.ScenicDream
```

We have already confirmed that the JMGO firmware keeps this setting across
restarts. The idle timeout is the projector's own setting.

## Acceptance (JMGO)

- After idle, the screensaver starts within the normal timeout, plays the local
  clips, crossfades smoothly, and wakes on any key.
- The app has no network permission. Confirm with `dumpsys package`, and check
  that `netstats` shows no traffic for its uid.
- The setting and the clips survive a restart.
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
