# Combined release — Apple 105 / Android 35

Both sessions' shared source is committed in be231b7: profiles/avatar editor, English/Chinese interface localization, library file actions and cast/browsing improvements, Continue Watching removal/Undo, compact search rows, deduplication and independent Add resolution, single-instance policy, and playback/session/index fixes.

Release builds succeeded for macOS, iPhone/iPad, tvOS simulator and Android. Source fingerprints were checked during client compilation; no source changes occurred. Two server checks exposed and fixed a series-pack year parsing regression and a fixture that predated metadata-readiness gating. Final Rust tests: 46 passed, one saved/live fixture test ignored in the general run. Android unit tests: 21 passed. The saved-source parser test was separately run earlier without contacting the provider.

- Mac 105 signed, installed at ~/Applications/Torrbaby.app and launched; only one native Mac process verified. Historical nested install path is a symlink. ZIP: ~/Downloads/Torrbaby/Torrbaby-macOS-105.zip.
- iPhone/iPad 105 published to https://jons-m5-mbp.bengal-locrian.ts.net/ota/torrbaby/index.html. SHA256 25dc41bc8e5a79d9a4d9966ccebd1f1db1e745516809cd3846a9554a0bf84eb0. Six provisioned devices.
- Android 0.3.9 build 35 signed/published on the same install page and copied to ~/Downloads/Torrbaby/Android/Torrbaby-0.3.9-build35.apk. Device installation is separate; reachable JMGO was still build 34 during verification. No additional TCL install request was sent over its prior pending confirmation.
- tvOS simulator 105 installed on the existing Apple TV simulator. No physical tvOS provisioning claim.
- Diamond server deployed from the same source. Backup: combined-105-20261110-114404. No recent active playback at restart; two pending subtitle jobs preserved/re-admitted. Deployed executable checksum matches local signed bundle.

HTTPS IPA/APK downloads were hashed against published metadata. The install page lists Apple105/Android35. Live Continue Watching hide/Undo for Bone Lake preserved watched flags and positions and restored original visibility. Server health and schema verified; no search-provider probes or torrent additions made during these checks.

Desktop automation could launch the canonical app but misidentified its bundle identifier and could not bind its window, even after refresh. Therefore this release does not claim completed visual acceptance across devices. macOS version/process/signature are verified; menu/focus/layout and physical mobile/TV behavior still need the planned acceptance round.

Local provenance/hashes: local-state/combined-105/release-manifest.json. Only unrelated docs/homeland-s04-subtitle-comparison.pdf remains untracked.
