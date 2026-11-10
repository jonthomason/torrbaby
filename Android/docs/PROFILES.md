# Shared TV profiles

Authoritative product and release plan: [user-profiles.md](../../docs/user-profiles.md).

## Implemented

- Server-owned profiles use `X-Torrbaby-Profile` on viewing requests.
- The selected ID and display name persist with the encrypted device connection.
- Newly linked devices choose or create a named user. Existing migrated devices
  receive Jon from the server binding. Subsequent launches use the remembered person.
- Every browsing page displays the active circular avatar and first name with an accessible Switch user action.
- The profile editor accepts an initial/Unicode/emoji symbol and independent foreground/background colors; appearance syncs through the server.
- Switching replaces the immutable API connection, clears the old browse hierarchy,
  and loads a profile-scoped catalog cache. Late catalog responses are ignored.
- Playback owns its API snapshot. Durable progress envelopes capture the profile;
  the server session remains the authoritative owner, including for legacy reports.
- Subtitle settings remain device-specific and are not reset by switching users.
- TV playback remains streaming only; no downloads.

## Remaining acceptance work

Test selection, creation, Back, relaunch and update persistence on the emulator,
using remote keys. Verify two people have independent history and one person shares
history across devices. Verify queued progress after switching and unavailable
profiles. Compare the rendered picker with tvOS once its profile UI is implemented.
This client requires the profile server API and must not be released ahead of it.

Build 34 is installed on JMGO. Focused server isolation tests pass; physical remote/UI review remains useful.
