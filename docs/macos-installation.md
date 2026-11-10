# macOS installation and instance policy

Use **~/Applications/Torrbaby.app** as the sole installed macOS app. Never install another app at ~/Applications/Torrbaby/Torrbaby.app: that historical path is now a symlink to the canonical app. Build products remain build artifacts, not installed apps. Quit the installed client before replacing it for a release, then reopen the canonical path.

On 2026-11-10 two actual processes were observed: build 103 at the canonical path and build 102 at the historical nested path. Both had org.torrbaby.mac as their bundle ID. This was conflicting installation conventions, not separate menu-bar helper processes. The nested build 102 was stopped and moved to local-state/retired-apps/Torrbaby-build102.app; the historical path now resolves to the canonical installation.

Info.plist now enables Apple's LSMultipleInstancesProhibited policy. The project generator preserves it. Launch Services rejects attempts to start a separate instance, including explicit new-instance launches. Normal launches of the installed app reuse the running process. Do not bypass Launch Services by running the executable directly.

User requested that new builds wait for localization and profile integration. No release build was made for this change. In the next combined test round check: normal reopening, historical-path reopening, open -n rejection, magnet-link delivery while already running, menu-bar Open Torrbaby, and separate playback windows. Separate windows/tabs are allowed within the one process.
