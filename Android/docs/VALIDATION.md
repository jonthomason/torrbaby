# Emulator validation — 2026-11-09

Android TV API 36, ARM64, 1920×1080, separate Torrbaby-TV AVD. All writes used the local fixture API; real catalog artwork/metadata was read-only. Test video was synthetic H.264/AAC with two audio tracks and an English WebVTT sidecar.

- Debug and unsigned release APKs build; seven API/library unit tests pass. Android lint reports zero errors (19 warnings/informational findings).
- D-pad library/detail/season navigation, remote keyboard search, no-result and empty-library screens, collection See All and return-focus behavior checked.
- Native decoded playback, pause/resume, seek, saved-position range request, audio-track and subtitle selection checked.
- Seven-speed dialog checked; Android media session reports actual 1.75× playback, retained on a subsequent playback after app restart.
- Next-episode transition checked after fixing duplicate MediaSession IDs.
- Progress persists through a simulated outage and clears after server acknowledgement; real end-of-media reports completion.
- Unavailable stream error and Retry recover; expired credentials and empty library have explicit UI.
- Shared subtitle editor operated by D-pad; a title override was saved and independently read back from the fixture API.
- Final Home, Paradise details and episode-row screenshots visually reviewed against the tvOS reference; typography, artwork rounding, ambient background and focus styling refined.
- Shared Apple speed code built for macOS, iOS Simulator and tvOS Simulator; Apple playback-speed runtime behavior was not independently exercised here. No Apple app was installed/deployed.

The emulator cannot establish TCL/JMGO hardware codec, HDR, passthrough or firmware behavior. Physical TV testing remains deferred until emulator review. Embedded subtitle formats beyond the tested WebVTT and subtitle translation/preparation are not claimed as runtime-verified. Release APK needs a private production signing configuration; review APK uses debug signing.

## Android 0.1.1 connection follow-up

Native DNS-SD instrumentation passed on the emulator, resolving an advertised `_torrbaby._tcp.` service and its port. Fresh-app address-only connection completed against the fixture `/v1/lan/register` endpoint and opened the library with encrypted saved credentials. No pairing token was entered. Shared Diamond LAN-server deployment is a separate prerequisite owned by the tvOS/server session.

## Android 0.2.0 — lead TV client

Debug/release builds and 11 unit tests pass; native discovery instrumentation previously passed. Emulator checks confirm torrent stop/start requests and updated status without losing row focus, deletion confirmation focused on Cancel and no delete request after cancellation, and a seeding-target PATCH accepted by the isolated fixture. Requests for prepared English-to-Chinese subtitles contain the shared server contract; the returned WebVTT is fetched and visibly rendered by Media3 alongside decoded video. Fixture caption text is synthetic: this checks integration/rendering, not translation quality.

Library refresh now polls every ten seconds while browsing, torrent status every five seconds. Dialogs and text entry are not interrupted; unavailable server status is marked stale. Profiles remain pending the shared backend, not a currently available tvOS feature. Physical codec/HDR/passthrough acceptance on TCL and JMGO remains required.

## Android 0.2.2 polish and media inventory

- Connection/discovery shows only the server name and mint spinner during normal operation; recovery controls appear after failure or timeout. Saved connections use the same minimal opening screen.
- Mint focus outlines on posters, hero, rows, and primary controls; poster scale has space to render without clipping.
- Registration sends the server-supported `other` platform category, fixing the refresh failure from `Unknown client platform` without changing the shared server. The fixture now enforces its platform enum.
- Shared per-asset `media_info` drives audio/subtitle language lines and purple Generated translation labels, including each episode. Only ready generated jobs receive that label. Missing/failed inventories remain explicit; library polling picks up completed metadata.
- Debug build, 13 unit tests, and lint pass. Emulator screenshots verified the clean discovery screen, focus, and generated-language details using synthetic language metadata. Actual background-translation completion on Diamond was not inferred from that fixture.
