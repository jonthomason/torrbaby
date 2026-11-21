# App updates from Diamond

Diamond serves the newest builds, so every device finds them through the server it already
reaches, at home or away (server/updates.rs). Apps ask when they come to the foreground; there is
no timer.

## iPhone and iPad (from build 382)

- Publish: `python3 scripts/package_ota.py --app … --publish-dir ~/.cache/miyu/ota/torrbaby
  --base-url https://jons-m5-mbp.bengal-locrian.ts.net/ota/torrbaby --diamond`. Besides the
  laptop's OTA page, the build goes to `<data>/updates/ios/<build>-<random>/` on Diamond and
  `current.json` is replaced last. The five newest builds are kept.
- The app reads `GET /v1/updates/ios` (authorised like any request). For a newer build it shows
  **Install N** above the connection status in the sidebar and phone menu, and beside Version in
  Settings.
- Install opens `itms-services://…` with the manifest at
  `https://torrbaby.diamond.abt.jonthomason.com/updates/ios/<tag>/manifest.plist`. iOS's
  installer fetches the manifest and IPA itself, without the app's credentials, so those two
  files are served openly under the build's random folder name only. They need a trusted
  certificate: Diamond's ABT host answers TLS for `*.diamond.abt.jonthomason.com` (ABT's
  scripts/host_certificate.sh). The device must be on ABT for the name to resolve.

## Android TVs (from build 125)

- Publish: `python3 scripts/publish_android_update.py` (the debug APK; it checks the package and
  reads the version code from the APK itself).
- Coming to the front, the TV app reads `GET /v1/updates/android`; a newer build is offered once
  per run (Install / Not now) and in Settings → About. The APK is downloaded from Diamond,
  checked against the published SHA-256 and handed to Android's installer, which asks on the TV.
  The first time, Android asks to allow "install unknown apps" for Torrbaby.
- Builds must be signed with the same key as the installed one (the building Mac's debug key).
- TCL may refuse installs from apps; until that is known, it keeps install_tcl.py.

## Not covered

- Mac: still installed by hand (`~/Applications/Torrbaby.app`).
- Apple TV: tvOS installs only from TestFlight, the App Store or Xcode.
