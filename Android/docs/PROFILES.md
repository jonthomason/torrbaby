# Shared TV profiles

Confirmed 2026-11-09: Android TV and tvOS support multiple viewers on the same device. Mobile normally remembers one profile, with switching available in Settings. Follow the shared product plan in `../../docs/user-profiles.md` and the eventual server/tvOS API contract.

Status: integration pending the shared profile API and tvOS picker. The current Android build still uses household viewing state. Do not present device registration as a viewer profile or implement local-only profiles that falsely imply server history isolation.

## TV interaction

- Show the active viewer and an obvious remote-accessible switch action in the main navigation; match the tvOS picker when it lands.
- Remember selection per server without treating device identity as person identity. Do not assume that remembering a viewer decides whether TVs show a picker at launch; follow the tvOS product decision.
- Profile creation requires a name only; photo is optional. No email/password/account setup.
- Keep the media library shared, with Continue watching, Recently watched, watched markers, resume positions and subtitle defaults/overrides belonging to the selected viewer.
- Preserve streaming-only behavior: no TV downloads.

## Integration points

- `Api` / `SecureStore`: retain device credentials separately from the selected stable server-owned profile ID. Use the agreed profile transport, not a guessed endpoint/header.
- `MainActivity`: switching invalidates viewer-dependent library state and pending UI responses. Fetch the selected viewer's state before exposing resume/watched actions; ignore old requests completing after a switch. Artwork can remain shared.
- `PlaybackActivity`: snapshot viewer identity when starting playback and retain it for progress, retries, subtitle selection and next episode. Do not relabel an existing session when the active viewer changes.
- `ProgressQueue`: persist the playback-time profile in each durable envelope. Flush using that captured profile, never the currently selected viewer. Retain migration semantics for existing unscoped records, which belong to the default household profile. Handle deleted/rejected profiles without assigning their events to another person.
- `Subtitles`: scope both default and per-title preferences to the viewer; invalidate stale editor/load responses on switching.
- Keep current-client compatibility until the server migrates shared history/preferences into the default household profile. Do not clear existing history or pending progress during migration.

## Acceptance checks when the contract lands

Two viewers watch the same episode independently; one viewer resumes on two devices; switching refreshes shelves and subtitle preferences; offline queued progress uploads to its original viewer after a switch; late responses cannot overwrite the new viewer; existing household history survives migration; missing/deleted profiles have a clear recovery path. Exercise picker creation/selection/back entirely with remote keys and compare its presentation with tvOS.
