# Remove from Continue Watching

Integrated with the profile/localization work for Apple 105, Android 35 and the matching server. The coordinated source includes the profile prerequisites.

- Per-profile SQLite profile_hidden_titles stores title ID and removal timestamp, separately from watched flags and positions. POST /v1/library/continue-watching accepts title_id and hidden. Catalog includes continue_watching_hidden and revision invalidation includes removals/restores.
- Starting a new playback session clears suppression for that title/profile only. Accepted offline progress clears it only when observed after the removal, so an older queued report cannot resurrect it.
- Apple: context menus on library posters (right-click / long press, including tvOS), plus title detail action. Continue Watching filtering honors suppression without altering Resume behavior. Shared bottom banner provides Undo for ten seconds, scoped to the original profile.
- Android: poster long-press, grid long-press, and title detail action; a bottom banner with a focusable Undo button. Updates local visibility after server acknowledgement and refreshes the catalog.
- Removal needs connectivity; failures use the existing error UI. No confirmation, media deletion, or watched-history reset.

Validation: focused Rust test passed for progress preservation, restoration on a new session, and isolation between profiles; Rust compilation passed as part of that test. Swift frontend syntax parsing passed. Apple and Android release builds pass. Further device acceptance should exercise menus/Undo and focus on macOS, iPhone/iPad, tvOS and Android; two devices on one profile; another profile; resume after removal; old vs new offline reports.

The task-only patch is saved locally at local-state/continue-watching.patch with before snapshots at local-state/continue-watching-before. Source edits are already applied; do not apply that patch a second time. The coordinated release commits the source together with its profile prerequisites.
