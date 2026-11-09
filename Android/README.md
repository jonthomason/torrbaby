# Torrbaby for Android TV

A native, remote-operated viewing client for Torrbaby's existing library API. All Android source, tooling, and local development state live here; no Apple or server changes are required.

## Scope

- tvOS-aligned Home: full-width hero, Continue watching, Recently added, Recently watched, movie/show and genre shelves, and See All collection grids.
- Alphabetical Movies and TV Shows grids, search, genre filters, and sorting.
- Live server torrent status, sorting, start/pause, verify, reannounce, rename, file details, organization retry/ignore, magnet submission, seeding targets, and separately confirmed removal. No TV torrent engine.
- Title details, seasons, episode selection, resume/start over, watched/unwatched, and next episode.
- Native AndroidX Media3/ExoPlayer video and remote controls: play/pause, seek, audio tracks, embedded subtitles, and server WebVTT sidecars.
- Seven remembered playback speeds from 0.5× to 2×.
- Shared subtitle defaults and per-title overrides, including language, font, size and opacity.
- Shared server playback history, with pending progress retained locally through transient network failures.
- Streaming only. Torrent controls operate on the server; no local download jobs, storage permission, persistent video cache, or offline media.
- Encrypted connection credentials in Android Keystore; app backup disabled. Cleartext HTTP is supported for the existing home-network server. Media/authentication requests reject redirects and off-origin URLs.

Android 8.0/API 26 or newer. The manifest includes both ordinary and TV launcher entries because Chinese TV/projector firmware may omit Google's Leanback launcher. No Google Play Services dependency.

## Build

Java 17+ (the bundled Android Studio JBR works), Android SDK platform 36 and build tools, Gradle wrapper included:

```sh
Android/tools/build.sh assembleDebug testDebugUnitTest lintDebug
```

Debug APK: `Android/app/build/outputs/apk/debug/app-debug.apk`.
Release APK is unsigned until configured with a private signing key; no signing secrets belong in this repository.

Dependencies: AndroidX Media3 1.9.3 (Apache 2.0); Android Gradle Plugin 9.2.1; Gradle 9.6.1. Test-only JUnit 4.13.2 (EPL 1.0) and JSON-java (public domain). No FFmpeg libraries are bundled. Host FFmpeg is used only to generate synthetic emulator test footage.

## TV emulator

Install `system-images;android-36;android-tv;arm64-v8a` with SDK Manager, then:

```sh
Android/tools/emulator.sh
```

This creates a separate `Torrbaby-TV` 1080p AVD in `Android/.local/avd`, leaving other AVDs alone. The TV density produces a 960×540 logical layout; fonts are deliberately restrained for a large screen.

Generate test footage and run the isolated fixture API in a second terminal:

```sh
Android/tools/make_test_media.sh
python3 Android/tools/fixture_server.py
```

To use an ignored local catalog snapshot instead of the synthetic catalog:

```sh
python3 Android/tools/fixture_server.py --catalog Android/.local/library.json
```

`tools/snapshot_art.py` reads artwork from Diamond into `.local/art` using SSH; credentials stay on Diamond. It requires a private `.local/library.json` snapshot. Catalogs, artwork, emulator state, and test media are ignored by Git. Fixture mutations never reach a real server.

Install and launch on the emulator (substitute its serial if different):

```sh
adb -s emulator-5554 install -r Android/app/build/outputs/apk/debug/app-debug.apk
adb -s emulator-5554 shell am start -n com.torrbaby.tv.debug/com.torrbaby.tv.MainActivity \
  --es test_server http://10.0.2.2:18765 --es test_token fixture
```

Only the debug variant accepts those bootstrap extras. The release variant ignores them. Normal TV setup matches tvOS: discover `_torrbaby._tcp.` with native DNS-SD, then POST the device name/platform to `/v1/lan/register` and retain the returned credential encrypted. One discovered server connects automatically; multiple servers show a chooser. Manual fallback takes only the server address, not a code/token. Diamond must advertise the service and enable the trusted-LAN registration endpoint before this works. The server authorizes the socket peer; the client does not decide which networks are trusted.

Use arrow keys, Enter/OK and Back/Escape. In video, OK reveals the native controls; Back first dismisses visible controls, then returns to the library. Media play/pause and seek keys also work. The gear in the native playback bar opens D-pad-friendly Android dialogs for audio, subtitles and playback speed; the caption button opens subtitle selection.

Fixture controls (localhost only): `POST /test/mode` with `{"mode":"normal"}`, `empty`, `offline`, `unauthorized`, `bad_video`, `slow_subtitles`, or `translated`. `GET /test/events` records fixture play requests, byte ranges, and progress for assertions.

## tvOS parity

See [the parity checklist](docs/TVOS_PARITY.md). `tools/build.sh` checks hashes of the reviewed tvOS reference files and prints a warning when the other session changes them. `python3 Android/tools/check_tvos_parity.py --strict` makes that check fail for automation. A matching hash means the reference has not changed; it is not an automatic proof of visual parity.

## Upcoming shared-TV profiles

Multiple viewers per TV are planned alongside tvOS. See [the integration checklist](docs/PROFILES.md). The current build still uses household history until the shared profile API is available.

## Device validation boundary

Emulator validation covers navigation, API integration, native controls, H.264/AAC playback, subtitles, and progress/error behavior. It cannot establish the TCL or JMGO hardware's HEVC, HDR/Dolby Vision, lossless audio, or passthrough support. ExoPlayer uses the device's decoders; Torrbaby does not transcode unsupported formats. Test representative real files on both devices after the emulator UX is accepted.

Android uses native embedded/sidecar subtitles and the shared extraction/translation job API. When the preferred language is missing, it prepares a server WebVTT track and attaches it directly to original-media playback; no HLS remux is required on Android. Unsupported preparation falls back to playback with a notice.
