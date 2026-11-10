# Apple build 106 — 2026-11-10

- Removed the full-width language header. EN/中文 is in the trailing navigation toolbar; iPad uses inline navigation titles.
- Title actions use a compact row, wrapping secondary actions consistently when necessary. Resume and Download stay together. Movies no longer show the redundant episode-style primary asset row; TV episodes and special features retain their rows.
- Added a process-held flock for macOS plus a single main Window scene. Duplicate direct launch exited successfully and left exactly one native Mac process; canonical install is ~/Applications/Torrbaby.app.
- Added AGENTS.md localization/layout requirements and a build-time check for reviewed EN/ZH catalog entries and newly added literal UI strings.

Subtitle choices belong only to the device, independent of profile. The stable Apple client ID lives in Keychain; a local preference cache survives updates/restarts and Diamond stores the corresponding device record. No household/profile subtitle settings are inherited. Native player choices continue to save back to that device record.

Playback now uses the cached device choice without waiting for a GET request. Reading Settings seeds the cache. A new device's initial English/on choice is consistent even when there is no cache and the request fails. Full subtitle tracks are preferred over forced-only tracks. Initial asynchronous track selection is allowed to settle before observing changes, preventing intermediate startup selection from being saved as Off. Diagnostics report requested and confirmed selection without media URLs or credentials.

Validation:
- Signed Release builds passed for macOS, iPhone/iPad and tvOS Simulator; localization check and git diff --check passed.
- Mac native UI: clean Home header; single-row movie actions; Chinese labels; one main window after reopening. Bone Lake started with English captions visible without selecting On in the native menu; logs confirmed enabled=true/language=en and successful selection. Test playback was paused and closed.
- iPad simulator: landscape detail page has trailing language control, inline title, two compact English action rows (Resume/Download together), a single Chinese action row, and no movie episode row. Selected profile and connection survived replacement/restart of the simulator app.
- OTA build 106 published and downloaded over HTTPS; artifact SHA256 matched current.json. Six provisioned devices, profile expiry 2027-11-10.
- macOS 106 installed at canonical path and launched; final duplicate-launch process check passed. CUA has stale app-path metadata for the canonical install, so the UI checks used the same build from its build output before canonical installation.
- tvOS Simulator 106 installed. Android remains build 35; no Android playback changes or server restart in this release.

Build logs and publication evidence: local-state/layout-106/. Download: ~/Downloads/Torrbaby/Torrbaby-macOS-106.zip. OTA: https://jons-m5-mbp.bengal-locrian.ts.net/ota/torrbaby/index.html
