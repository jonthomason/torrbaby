# Hardware keyboard audit

Magic Keyboard is a primary iPad input method. Keep native AVKit controls and
system text-entry/focus behavior. Shortcuts must not capture typing in app forms.

| Context | Keys | Current status |
| --- | --- | --- |
| Full-screen player | Space | Added explicit play/pause in build 57; uses timeControlStatus so buffering can also be paused. Physical keyboard verification pending. |
| Full-screen player | Left / Right | Existing 15-second back/forward; user verified on iPad. |
| Full-screen player | Escape | Existing close action returns to originating page; physical-keyboard regression check pending. |
| Native controls | Tab / Shift-Tab, Return | Preserve inherited AVKit commands; manually audit focus and selection. |
| Library/torrent browsing | Command-F | Search shortcut currently wired only on macOS; add iOS support. |
| App navigation | Command-[ / Command-] | macOS commands exist; mobile back/forward buttons lack key bindings. |
| App | Command-R | macOS refresh exists; add mobile binding. |
| App | Command-comma | macOS Settings scene exists; add iPad Settings action. |
| Torrents | Command-Shift-N | macOS Add Torrent exists; add mobile binding. |
| Sheets/forms | Escape, Tab, Return | Audit native cancellation, focus order, submit and text entry. |
| PiP | Keyboard/media keys | Audit AVKit ownership while app or another app is foreground. |

PlayerController now subclasses the shared KeyboardPlayerController instead of
maintaining a second divergent command list. Custom commands expose discovery
titles and preserve inherited AVKit commands except exact key/modifier matches.
No global keyboard monitor is installed on iOS. tvOS simulator keyboard capture
is a Device Hub setting, separate from app shortcut handling.
