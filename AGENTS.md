# Torrbaby product and release rules

- Every new or changed user-facing string must ship in English and Simplified Chinese. This includes buttons, menus, alerts, validation, toasts, accessibility labels and empty/error/loading states on every platform. Add the English key and reviewed Chinese translation to localization/zh-Hans.tsv, regenerate platform resources with scripts/generate_localizations.py, and use the platform localization mechanism. Do not translate protocol values, filenames, user-entered names or media metadata as interface strings.
- Run scripts/check_localizations.py before builds. A release is not complete until new UI has been checked in both EN and 中文. Do not claim visual verification from a successful build.
- Do not put a Connection settings button in the sidebar footer on any platform. Keep connection status at the bottom; connection configuration belongs in Settings.
- The language switch belongs at the top right within existing navigation space; never add a full-width language header that pushes the content down.
- Keep iPad landscape and macOS library/detail layouts consistent. Play/Resume and Download belong on the same row. Wrap secondary actions compactly with consistent spacing. Movies have one main Play/Resume action at the top; episode-style playback rows are for TV episodes and special features only.
- macOS installs only to ~/Applications/Torrbaby.app. The nested historical path is a compatibility symlink. Keep one Mac client process and one main library window group; manually created tabs and separate playback windows are allowed. Never create tabs automatically. The Mac + button belongs immediately after Back/Forward, before the page title; Cmd-T also creates a tab. Verify attempted duplicate launches rather than relying only on an Info.plist flag.

- Subtitle preferences belong only to the device, independent of profiles. Preserve device identity and subtitle choices across updates and restarts; never inherit a server-wide or profile subtitle default. Diamond may store the device-specific record.

- BT之家 browsing must not fetch full details or torrent attachments automatically. Fetch only row-summary information and attachment choices; retrieve a selected torrent attachment only after an explicit Add action. Fetch full details only after the explicit info-button action; keep the server detail route inert for older clients without that flag. Add prepared attachment URLs directly from the result rows.
- Search efficiency must come from cache reuse, duplicate-request coalescing and avoiding unnecessary source requests, not arbitrary user quotas, cooldowns or local Search is busy rejection. Keep search changes consistent across Apple and Android clients. Do not probe Chinese torrent servers for testing.

- Publish iPhone/iPad builds to OTA only. Do not install or force-update the user's physical iPad or iPhone; the user installs from the OTA page. Simulator installs remain allowed for validation.

- Never ask the user to disclose a keychain password. Publishing signed builds is authorized; use the existing signing setup without collecting or logging passwords. Physical iPhone/iPad installation remains separate from OTA publication.

- Active shared-worktree release coordination: read local-state/credits-release-handoff.txt before publishing. The processing-queue session should record readiness in local-state/processing-release-ready.txt; the UI/release session owns the combined build publication.
