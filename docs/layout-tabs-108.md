# Apple build 108 — compact navigation and manual tabs

- Restored the macOS + button immediately after Back/Forward and before the page title. Cmd-T and the native tab-bar + create tabs in the same window. Automatic window tabbing remains disabled; duplicate processes remain blocked.
- iPad library/torrent search and EN/中文 now share an explicitly ordered trailing toolbar item, with language at the far right. No extra language header.
- iPhone/iPad branding sits in the sidebar navigation bar; on iPad it is to the left of the native sidebar toggle. Removed mobile Connection settings footer button. Connected/Offline status occupies its own fixed bottom area above the safe-area boundary.
- Reduced mobile sidebar row and section spacing. All entries, including Settings, fit on the tested iPhone portrait and iPad landscape simulators.
- Compact iPhone library shelves show three complete posters across, with smaller typography, outer margins and gaps. Grids also use three columns; regular-width layouts retain their existing poster sizes.

## Validation and delivery

- Release macOS/iOS builds 108 and Debug iOS simulator build passed; localization checker passed all 560 reviewed EN/ZH entries, and git diff --check passed.
- Visually checked EN/中文 on mobile, iPad far-right selector and inline branding, full iPhone menu, bottom connection status, and three complete phone Home posters.
- Visually exercised Mac toolbar +, Cmd-T, and native tab-bar + in build 108; explicit actions produced native tabs rather than separate windows.
- Installed signed Mac build 108 to ~/Applications/Torrbaby.app. Attempting a duplicate binary launch exited successfully with only the canonical Mac process remaining.
- Published iPhone/iPad build 108 to the existing Miyu Install Torrbaby OTA page; verified remote current.json and the downloaded IPA SHA-256. Android artifact remains unchanged.
- Mac archive: ~/Downloads/Torrbaby/Torrbaby-macOS-108.zip.

No Diamond service or media files were changed for this release. Layout checks cover the simulator sizes used, not every Dynamic Type setting or device size.
