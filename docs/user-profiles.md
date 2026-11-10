# Profiles, watched state, and library file management

Status: server schema 10 and Android build 25 deployed; Apple build 93 published.
Remaining delivery limitations and implementation decisions are recorded below.

### Current handoff

- Shipped: Diamond server; JMGO Android 25; iPhone/iPad OTA 93; installed Mac 93.
- Waiting for product choice: iPhone/iPad early-next action (final-10-percent
  overlay, paused-only action, or automatic advancement only). The question is
  pending; no fallback has been silently substituted for the requested native UX.
- Waiting for signing access: a tvOS development profile for `org.torrbaby.tv`.
  Xcode reports no configured account/profile. A local App Store Connect private
  key exists, but its issuer configuration was not available to this session.
- Delivery deferred: TCL `192.168.1.201:6553` was unreachable. Android 25 is ready.
- Verification follows the lean policy below. Existing build, migration, playback,
  and destructive-operation evidence is retained; do not restart a broad test tour.


## Persistent goal

Implement and ship persistent user profiles, profile-specific watch history and
subtle watched indicators, and safe library file management across Android TV,
tvOS, iPhone/iPad, and macOS. Migrate existing history to Jon without data loss.
Keep whole downloads out of the library until complete. Support permanent deletion
and, for torrents with seeding obligations remaining, moving the entire download
package into Junk, hiding it from everyone's library, continuing seeding, and
automatically deleting it when the configured seeding requirement is satisfied.
Validate on the server and clients, publish client builds, and install Android on
reachable JMGO/TCL devices. Report any device that still needs installation approval.

## Confirmed product decisions

- Profiles identify people, not devices. One person can use several devices.
- Existing watch history and resume positions belong to **Jon** after migration.
- Profile creation requires a display name. No email, password, PIN, or content
  restrictions in this version. Optional photos can follow; initials suffice.
- Mobile normally uses one person. On first setup, choose an existing profile or
  create one. Profile creation is primarily a mobile/desktop flow.
- TV supports several people, with an obvious Switch user action. Selection is
  sticky across launches, restarts, and updates; never ask again on every launch.
- Mobile and Mac also support switching, less prominently in Settings.
- Library contents and file removal are household-wide. Viewing state is personal.
- Subtitles remain a **device preference**, preserving the established English on
  iPad / Chinese on TCL behavior. Changing profiles must not reset that preference.
- Junk hides content for everyone and automatically deletes it after seeding ends.
- Junk operates on the **entire download package**, including a whole season or
  series torrent. Show the affected contents and scope before confirmation.
- If a completed package has no seeding obligation remaining, permanent deletion
  is the only library-removal option. Do not offer a keep-on-disk hide operation.
- Preserve Apple's native playback controls; do not reintroduce the withdrawn
  custom subtitle overlay as part of this work.

## Profiles and migration

Introduce stable server-owned profile IDs and profile CRUD limited initially to
listing, creation, and display-name changes. Profile deletion, PINs, and parental
restrictions are out of scope. Avoid assuming that a device credential is a person.

Persist an active profile against each registered device, and cache it locally so
launch/offline use is predictable. Existing linked devices migrate to Jon to preserve
continuity; newly linked devices must choose/create a profile. Never silently
switch to another profile when a selected profile cannot be resolved. A reinstall
that loses device identity follows first-run selection again.

Partition resume positions, watched flags, playback history, Continue Watching,
and personalized hero selection by profile. All library responses must reflect the
requesting profile. Capture the profile ID at playback start, in server sessions,
and in durable offline progress events. Switching profiles cannot redirect a late
report into the newly selected user's history. Stop active playback when explicitly
switching people. Pending sync remains owned by its original profile.

Migration must be transactional, versioned, idempotent, and backed up. Preserve
existing media identities, positions, timestamps, explicit watched flags, and
sessions. Assign all pre-profile records and legacy offline events to Jon. Older
clients continue as Jon during rollout; never infer an old client's profile from a
new client's active selection. Avoid shared-cache leakage between profiles.

A locally downloaded file may be shared across profiles; do not duplicate its bytes.
Review existing auto-delete-after-viewing behavior so finishing under one profile
does not unexpectedly delete a download needed by another. Do not expose another
profile's resume position or Continue Watching offline.

## Watched behavior

Current implementation has a shared watched flag and generally uses a 95% threshold.
That is not enough for personal history and may require watching well into credits.

Initial implementation rule: mark watched on actual playback completion, or when
playback reaches **at least 90% and no more than 10 minutes remain**. This is a
predictable heuristic, not automatic credits detection. Use the same rule in online
and offline reporting on every platform. Do not mark watched from scrubbing alone;
base it on a valid playback report with a known duration. Keep an explicit
Mark watched / Mark unwatched action for corrections.

Once watched, keep that flag through rewatches until explicitly marked unwatched.
Retain real resume/history data as appropriate, but remove finished items from
Continue Watching. Update all devices viewing the same profile after refresh.

Use a small, restrained checkmark on watched movie artwork and episode rows; include
an accessible Watched label. Avoid large badges or dimming the artwork. A series is
fully watched only when all available main episodes are watched; extras do not count.
The Matrix's existing completion must be preserved under Jon, with its recorded
position checked during migration rather than assuming that a recent viewing ended.

### Skip episode credits — added 2026-11-10

On Android and all Apple clients, when the user shows the playback controls during
the final **10% of an episode**, offer a **Next episode** button. Keep the native
player and its existing controls. The button belongs with the visible controls;
do not automatically interrupt playback or force the controls onscreen.

Offer the action only for episodes with a known duration and an available next
episode in the correct series order. Do not skip a missing episode silently or
offer it for movies. Preserve the current profile and device playback/subtitle
preferences, flush the outgoing episode's progress, and apply the same watched
completion rule when advancing. Make the action easy to focus with a TV remote
and easy to tap on mobile.

Automatic advancement at actual episode end is reported broken in current mobile
builds. Diagnose and fix that path as part of this work, rather than assuming the
existing implementation works. Use the same ordered-next-episode resolver for the
early button and end-of-playback advancement. Test streaming and downloaded
episodes, native player end notifications, view/presentation lifetime, preparation
failures, missing episodes, and final-episode behavior. A failed preparation must
show useful feedback instead of silently leaving the player at the credits.

When an episode finishes, its next available consecutive episode must occupy the
first Continue Watching slot for that profile, even with zero playback progress.
Order this continuation by the outgoing episode's last-played time, not the new
episode's zero timestamp. Keep the Hero consistent and label the continuation
with the next episode number. Do not populate new profiles from others' history.

## Prepare the library during downloads — added 2026-11-10

Separate metadata/artwork readiness from Library visibility. As soon as a torrent
has its file list, create stable pending asset records, even before the media files
exist on disk. Match the movie or series, resolve episode metadata, and download
posters, backdrops, logos, and episode images into the server artwork cache while
the package downloads. Preserve identities, confirmed matches, and prepared
artwork through completion and organizer relocation.

Pending assets must not appear in Library, search within Library, Hero, or Continue
Watching and must not be playable. The default visibility gate remains completion
and availability of the **entire package**, not an individual finished episode.
After completion and file verification, publish the prepared records rather than
starting metadata matching and artwork downloads then. Torrent/search results can
show the pending download independently of the Library.

Track metadata matching and artwork fetch state separately from download progress.
Retry failed artwork fetches with backoff; a bad image must not abort metadata work
for other titles. Deduplicate shared series artwork and bound background fetch
concurrency. Missing artwork must remain retryable after publication and must not
keep otherwise playable content hidden indefinitely. Uncertain matches must not
be treated as confirmed simply because the download completed. Junk and deleted
packages remain excluded from preparation and publication.

Baseline before this change: torrent metadata and torrent posters were prepared
before completion, and existing partial media files could be matched. However,
the scanner required a file to exist before creating its asset record, and Library
backdrops, logos, and episode images were fetched on demand. The local implementation
now creates pending records from the torrent file list and prepares all these image
variants independently, with persisted retries. Deployment and end-to-end artwork
acceptance remain outstanding. Investigate the reported live failures as well;
these gaps alone do not prove the cause of every missing image.

Acceptance: add a multi-episode package to an isolated test library, verify pending
records and cached artwork before completion, verify no Library visibility after
only one episode finishes, and verify the full package appears with prepared
artwork after completion. Include relocation, daemon restart, transient artwork
failure/retry, ambiguous matches, and Junk exclusion. Verify actual newly added
content on the server and clients before declaring the image issue fixed.

## Library file management

Place a Manage files action in title/episode detail menus on every platform. It is
separate from the profile-specific Mark watched action. The server supplies an
operation preview: package identity, affected titles/episodes, files, total bytes,
seeding state, permitted actions, and any shared-file conflicts.

### Delete permanently

Available for completed content, including active seeders when the user explicitly
chooses to stop seeding and delete. Confirm the actual package scope and explain
that deletion affects everyone. Remove the torrent from Transmission when present,
then remove only the package's owned media, subtitle sidecars, and derived caches.
For completed standalone library files without a torrent, remove the media and its
associated files. Never recursively delete a parent directory containing unrelated
media. Keep profile history as a tombstone so a future re-add can restore watched
state where media identity is confidently matched.

### Move to Junk and finish seeding

Offer while seeding obligations remain, including queued/paused seeders; don't equate
"not currently uploading" with "finished seeding." Confirm the complete torrent
package scope. Record durable removal intent, use Transmission's data-relocation
operation to move the package under a managed Junk root, and verify Transmission
can still find the data. Continue or resume seeding under the existing policy.

Once accepted, suppress the package from all library-derived surfaces (Home, movies,
shows, search within the library, recommendations, Continue Watching, and heroes)
for every profile. Its only active content listing is the Seeding list, with a clear
Junk / delete after seeding label. Exclude Junk from scanner import, metadata work,
and subtitle generation. Polls, restarts, and rescans must not resurrect it.

Use the configured ratio/time/seed-limit completion policy, not a transient stopped
status, to decide when seeding is done. Then remove the torrent and permanently
delete its owned files and caches. Persist and reconcile each phase so interruptions
or timeouts cannot cause repeated moves, lost data, or unrelated deletions. Errors
remain visible on the seeding entry and are retryable. No automatic deletion before
the configured completion condition is positively established.

### Boundaries and protections

- File operations affect server-owned library/package files. Existing offline
  downloads on mobile remain a separate download-management concern; do not add
  remote device wiping silently.
- Verify package ownership, managed roots, symlinks, overlapping torrent files,
  current paths, and filesystem identity immediately before mutation.
- Preview and execution must agree; reject stale previews and return an updated one.
- Reject deletion/relocation during active playback of an affected file, with a clear
  message. Do not interrupt another household viewer without warning.
- If other torrents share files, do not delete or relocate those files until ownership
  is resolved. Never remove unrelated content to make a package operation succeed.
- Keep the existing complete-package library gate enforced independently of UI state.
- Do not alter seed targets, download completion rules, or retained local downloads
  as a side effect of profile migration.

## Implementation sequence

1. **Server profile foundation:** schema migration to Jon, device/profile binding,
   profile endpoints, scoped history/progress APIs, and legacy-client compatibility.
2. **Client identity:** first-run selection/creation, sticky TV switcher, mobile/Mac
   Settings switcher, profile-aware requests, local caches, and offline event capture.
3. **Watched presentation:** common completion semantics, subtle indicators, manual
   corrections, profile-aware Continue Watching and hero selection.
4. **Server file lifecycle:** preview/confirmation endpoints, durable operations,
   safe permanent deletion, Transmission relocation, Junk exclusion, seed-completion
   cleanup, and restart reconciliation.
5. **Client management UI:** clear scope previews and destructive confirmations on
   mobile, Mac, Android TV, and tvOS; useful completion/error feedback.
6. **Release and verification:** back up production, migrate/deploy compatibly,
   publish Apple/Android builds, install on available devices, and verify live profile
   isolation and non-destructive behavior before reporting completion.

## Acceptance checks

Execution policy (user direction, 2026-11-10): keep verification lean. Reuse the
evidence already collected; do not repeat broad suites or platform-by-platform UI
tours. Run builds for changed clients, focused regressions for actual bugs, and
the safeguards needed for permanent deletion. The list below defines behavior to
deliver, not a requirement to build an exhaustive new test matrix before shipping.

- Run the migration twice: all prior history belongs to Jon, with no duplication or
  lost positions. Newly created users have independent empty history.
- Same profile on iPad and TV shares progress/watched state; another profile does not.
- First run offers create/select; selected person survives relaunch and app update.
- A TV profile switch is obvious and remains selected on the next launch.
- Offline playback reported after a profile switch updates the original profile only.
- English/Chinese subtitle defaults stay attached to their devices through switching.
- Finish near movie credits: watched checkmark appears on every platform for that
  profile, and the movie leaves Continue Watching. Manual unwatched works.
- Show episode controls before and after 90%: Next episode appears only in the last
  10%, only with an available consecutive episode. Activate it by remote or touch;
  confirm outgoing watched/progress ownership and incoming episode/profile.
  Hiding controls hides the action; movies and missing-next-episode cases omit it.
- Let an episode reach its actual end on mobile: the next available consecutive
  episode starts automatically. Verify at least two consecutive transitions for
  streaming and downloads, plus TV end advancement and last/missing-episode cases.
- Finish an episode and return to Home: its unstarted next episode is first in
  Continue Watching and agrees with the Hero. Movies, missing episodes, completed
  series, and a fresh profile must not gain a spurious next-episode continuation.
- Incomplete packages remain absent even when some episodes are fully downloaded.
- Preview a series package from one episode: confirmation lists the entire scope.
- Junk move retains valid Transmission data and seeding progress, hides the whole
  package everywhere except Seeding, and stays hidden after scan/restart.
- Completing seeding removes the torrent and owned files; a paused or stalled seeder
  is not mistaken for finished. Restart at each operation phase is safe.
- Permanent deletion handles standalone and torrent-backed content and associated
  subtitles/caches without touching neighbors or shared files.
- Use disposable fixture media/torrents for destructive tests. Do not use The Matrix,
  Friends, or other real household titles as deletion test fixtures.
- Verify the rendered UI and actual flows; compilation alone is not visual acceptance.

## Completion report

Record migration/backup locations, test results, published build numbers and links,
which physical devices were verified, and any remaining installation approval.
Do not claim implementation complete while a required platform or lifecycle path is
unimplemented. Keep unrelated concurrent worktree edits intact.

## Implementation progress — 2026-11-10

- Added server-owned profiles, persistent device selection, profile playback tables,
  and session/profile mapping schema. Added list/create/rename/select API routes.
- Database version 10 migration copies existing viewing state and sessions to Jon,
  binds existing devices to Jon, and leaves new devices unassigned. SQLite snapshots
  are created before upgrading an existing database. Reopening does not remigrate
  or reset device selection.
- Playback, catalog, manual watched changes, and offline progress now use profile
  ownership. Requests without a profile remain Jon for legacy compatibility.
  Playback sessions retain the starting profile, and offline reports use their
  captured profile even after the device switches people.
- Six profile tests cover migration/reopen/backup preservation, completion rules,
  session ownership across switching, catalog isolation, offline event ownership
  and idempotency, and manual unwatched protection against stale reports.
  Full current Rust suite: 21 tests passed on 2026-11-10.
- Android now has local implementation of first-run selection/creation, a named
  navigation switcher, encrypted sticky selection, profile request headers,
  isolated catalog caches, and captured profile ownership in queued progress.
  Switching replaces the immutable API connection and discards old browse state.
  Subtitle preferences remain device-specific. APK assembly, 19 unit tests, and
  lint passed; emulator/physical-device acceptance is still outstanding.
- Apple now has local shared profile selection/creation, sticky per-server/device
  selection, a tvOS user tab, and a Settings switch action on mobile/Mac. Profile
  changes clear the library/history and stop active native playback. Requests carry
  the selected profile, and stale library/history responses are rejected.
- Download manifests preserve legacy progress under Jon and store independent
  viewing records per profile without copying video bytes. Offline synchronization
  sends each event's captured profile. Local completion is sticky and uses the
  agreed 90%/10-minute rule. Automatic removal after playback is suppressed when
  multiple profiles may need the file; explicit local deletion is unchanged.
- macOS, tvOS Simulator, and iOS Simulator builds pass. The executable storage
  regression test in Apple/Tests/ProfileDownloadTests.swift covers old-manifest
  migration, profile isolation, restart persistence, stale acknowledgments, and
  pending-event ownership. Rendered UI and end-to-end client checks remain pending.
- Not yet deployed. Full watched presentation, file lifecycle implementation,
  production migration verification, and all release/physical-device acceptance
  remain outstanding. Do not ship new clients before the profile server API.
- Added subtle artwork checkmarks to Apple and Android posters. A series is marked
  only when every available main episode is watched; extras do not count. Existing
  episode markers remain. Android checks and macOS build pass; visual acceptance
  and the updated tvOS/iOS builds still need verification.
- Added a read-only file-management preview endpoint at
  `GET /v1/library/assets/{id}/manage-files`. It resolves whole torrent scope or
  a standalone file, lists affected catalog entries and associated subtitle files,
  and includes a filesystem-based confirmation fingerprint. Active playback,
  shared torrent paths, hard links, symlinks, and unmanaged paths prevent actions.
  Tests cover package scope, standalone scope, incomplete packages, stopped seeders,
  sharing/playback conflicts, fingerprint changes, and unsafe filesystem paths.
  Rust suite: 22 tests passed. No deletion or relocation endpoint is implemented
  yet; derived-cache cleanup and durable execution/reconciliation remain to do.
- Added transactional removal-intent records with idempotent acceptance and guarded
  phase transitions. Affected assets and torrent identities remain excluded across
  restarts and rescans; catalog, metadata, subtitle candidate selection, organization,
  and new playback consult that exclusion. Personal history remains intact.
- Added filesystem deletion primitives that verify the accepted device/inode,
  size, modification time, managed root, and lack of links before removing individual
  owned files. Retry tolerates files already removed; replacement files and neighbors
  are preserved. These primitives are not yet exposed as an executable API.
  Durable-operation and disposable-filesystem regression tests bring the Rust suite
  to 24 passing tests. Worker integration, actual Junk relocation, cache cleanup,
  seeding-list presentation, and lifecycle acceptance remain unfinished.
- Permanent deletion now has a confirmation API and recovery worker. It detaches
  Transmission with `delete_local_data=false`, verifies owned paths, removes only
  confirmed files, and purges associated HLS, virtual-media, and subtitle caches.
  Busy cache preparation delays deletion; errors stay on the durable operation.
  Playback session creation shares the mutation lock with confirmation.
- `python3 tests/test_library_operations.py` passes against a disposable daemon and
  loopback Transmission stub. It verifies stale confirmation rejection, idempotent
  acceptance, restart recovery, lost RPC response after detach, individual file and
  generated-cache cleanup, neighbor preservation, and retained watched history.
  Rust unit suite remains 24 passing tests. No household files were used.
- Junk submission still rejects explicitly until relocation/reconciliation is
  implemented. Client file-management UI, real seeding lifecycle tests, profile UI
  acceptance, and production releases are still pending.
- Junk relocation is now implemented locally. It uses a package-specific folder
  under the containing managed root's `Junk` directory, preserving same-volume
  inode identity. Transmission moves torrent files; associated sidecars follow
  by verified rename. Relocated file identities are persisted before seeding.
  Paused/stalled states retain data; positive `is_finished` plus complete-package
  validation permits cleanup. Scanner traversal excludes managed Junk roots.
- Both end-to-end operation tests pass (67 seconds total): delete and Junk,
  including lost move/removal responses, restart during seeding, paused-seeder
  retention, preserved neighbor files/caches, and automatic cleanup after the
  stub reports seeding finished. Real Transmission relocation remains unverified.
- Apple and Android Continue Watching now select an unstarted next episode after
  the latest watched episode and sort by recent viewing activity. Posters identify
  the upcoming episode. Android ordering tests and macOS build pass. Mobile auto-
  advancement diagnosis found swallowed preparation errors and no mobile progress/
  end-message presentation; fixing and testing the full transition remains pending.
- Mobile advancement now retains the current AVKit item during next-item preparation,
  surfaces preparation failures instead of swallowing them, and presents a native
  progress indicator and completion message. Apple early/end advancement shares
  one transition method and the same next-episode resolver. Actual consecutive-
  episode playback acceptance is still required; compilation is not proof of a fix.
- Android now advances automatically on episode end and has a remote-focusable
  Next episode button in the final 10%, visible with the player controls. Queues
  use consecutive episode order and stop at gaps. Android build/lint and 21 unit
  tests pass. tvOS uses a native transport-bar Next episode action; macOS and tvOS
  simulator builds pass, with rendered and playback tests pending.
- iPhone/iPad native AVKit does not expose the tvOS transport-bar customization or
  visibility callback in the installed SDK. Asked whether the separate mobile
  button should remain visible throughout the final 10% or only while paused;
  no private view-hierarchy inspection or replacement of Apple's controls planned.
  Mobile and macOS early-next button presentation remains unfinished.
- Concurrent virtual-MKV/media work is present in main.rs, remux.rs, vendor code,
  and experiments. Preserve it and do not treat it as this goal's changes.
- Android detail menus now expose Manage files for complete assets. The server
  preview lists the entire affected package, total size, household scope, and
  blocking reasons. A second confirmation precedes permanent deletion or Junk.
  The dialog shows submission progress and polls the durable operation, preserving
  remote focus while its status is unchanged. Lost-response retries use the same
  confirmation token. Android build, lint, and unit tests pass; rendered UI checks,
  Apple controls, and persistent Seeding-list operation status remain outstanding.
- Apple title/episode More menus now include profile-specific Mark watched/unwatched
  and household Manage files. The management page shows package members and size,
  blocking reasons, destructive confirmation, submission progress, and durable
  operation status. macOS, iOS simulator, and tvOS simulator builds passed; actual
  rendered and interaction checks remain pending.
- Torrent API rows now carry active file-operation status. Android, tvOS, and
  mobile/macOS torrent lists keep Junk in Seeding even when paused or queued and
  display its automatic-delete status and errors. Ordinary torrent mutations are
  rejected while a removal owns the package, preventing competing deletion or
  rename actions. Server unit coverage includes paused Junk and unchanged RPC
  state. Operations whose torrent has already disappeared still need a persistent
  status entry in Seeding; the management screen itself can poll them by ID.
- Pending download assets now exist before media files do. Scanner identity and
  confirmed matches survive file creation, package completion, and relocation.
  Completion requires every package file to exist at its reported size. A later
  filesystem-only scan cannot promote a previously incomplete known package.
  Metadata matching includes pending torrent assets, while Library continues to
  require complete and available assets.
- Artwork preparation now runs independently of metadata matching, with four
  concurrent fetches, shared-URL deduplication, and persisted failure/backoff state.
  It warms title posters, backdrops, logos, and the downloaded package's episode
  images; removal intent excludes Junk. Unit tests cover pending preparation,
  deduplication, episode selection, and retry bounds. Network retry/rendered cache
  acceptance and deployment are still pending.
- Read-only inspection of Diamond confirmed uncached backdrops/logos on Outlander
  and multiple completed movies. Production remains schema 9. An isolated copy
  migrated through the actual preview server to schema 10: all 6 history rows and
  all 21 device bindings were preserved under Jon, and a backup was produced.
  The Matrix retained watched=true at 7804.609 / 8177.709 seconds. Evidence is in
  `local-state/profiles-check/migration-live-copy/verification.json`; the copied
  databases are private local artifacts, not repository deliverables.
- Android TV emulator profile acceptance passed against the isolated Rust preview
  server and copied database. A newly registered device showed Who's watching;
  choosing Jon restored Continue Watching. Creating Alex through the UI produced
  zero watched/started assets, while Jon retained 2 watched and 5 started assets.
  Alex survived force-stop/relaunch and APK replacement without another chooser.
  Switching back restored Jon's Hero and Continue Watching. The device's enabled
  Chinese subtitle preference remained unchanged. After stopping the preview
  server, offline relaunch retained Jon and Jon's cached library.
  Navigation used D-pad keys; profile text was entered via emulator input.
  Screenshots: `Android/.local/screenshots/profiles-*.png`; API verification:
  `local-state/profiles-check/migration-live-copy/android-profile-verification.json`.
  The debug bootstrap now uses a valid stable UUID, allowing these tests against
  the real server instead of only the permissive Python fixture. No household
  profiles/history or physical TV installations changed. Apple profile UI,
  consecutive playback, and file-management interaction checks remain pending.
- Explicit re-add of a package whose removal finished now archives the old removal
  and clears its Library exclusion. Asset IDs and all profile history remain;
  availability/completion reset until a fresh scan verifies the new files. Old
  confirmation retries return the archived receipt rather than deleting the new
  download. Active removal/seeding operations are not released by re-add. Unit
  tests cover a second removal, restart, and history retention; both disposable
  daemon operation tests pass with re-add and delayed-confirmation checks.
- Removal previews and execution now reject subtitle sidecars that also match a
  neighboring media file outside the selected package. This ownership check is
  repeated during reconciliation, protecting neighbors created after confirmation.
  The server unit suite passes (28 tests, including concurrent-session tests).
- iPad profile UI acceptance passed on the dedicated `Torrbaby Profiles acceptance`
  simulator (`2D0B2913-32C3-47E1-8F18-ACBBBAD576F6`) through Device Hub. First-run
  choice, switching Jon/Alex, creating `iPad Test`, and sticky selection after
  relaunch were exercised. Jon's rendered first Continue Watching card was
  Paradise / Up next S01E02, consistent with Hero; new profiles had no inherited
  history. API evidence: `local-state/profiles-check/migration-live-copy/ipad-profile-verification.json`.
- That test exposed an Apple refresh dependency: Transmission being unavailable
  marked the whole client offline and prevented Library loading. Torrent refresh
  now follows Library refresh and reports its own error on torrent screens.
  The iPad UI remained connected and browsable against the isolated daemon with
  Transmission deliberately unavailable. iOS debug, macOS, and tvOS simulator
  builds pass. Simulator persistence checks require signing enabled so Xcode
  supplies the simulated Keychain entitlement; unsigned builds do not prove
  credential persistence.
- iPad streaming acceptance used three generated 20-second H.264/AAC episodes
  against the isolated preview daemon. An uninterrupted run advanced E1 → E2 → E3
  and displayed “No more episodes available”; all three sessions recorded ended.
  An earlier E2 pause did not recur, and resuming that run also completed correctly.
  Evidence: `local-state/profiles-check/migration-live-copy/stream-advance-uninterrupted.json`.
  This proves the current simulator streaming path, not physical-device or offline
  acceptance. Episode Options now includes the existing download control on mobile
  and macOS, allowing a specific episode to be downloaded regardless of the hero's
  selected episode. TVs still do not offer downloads.
- Successful file-operation reconciliation now clears an earlier error even when
  it stays in the same phase, such as continued seeding after an RPC outage.
  The update checks the observed revision so stale recovery cannot clear a newer
  failure. The restart/history regression covers both recovery and a stale result.
- Fully offline iPad advancement also passed E1 → E2 → E3 with the isolated daemon
  stopped. The native player showed the final-episode message. Each downloaded
  episode recorded playbackFinished=true under `iPad Test`; Jon's local positions
  stayed at zero. Evidence:
  `local-state/profiles-check/migration-live-copy/offline-advance-observations.json`.
  iOS Debug, macOS Release, and tvOS simulator Release builds pass after adding
  the per-episode download action. Physical playback and early-next controls remain
  outstanding; no production deployment was performed for this acceptance run.
- Apple and Android Seeding screens now fetch durable file operations separately
  from Transmission and display unfinished operations after the torrent disappears.
  Matching live torrent rows suppress duplicate operation cards; all operation-owned
  torrents stay in Seeding. Errors and automatic-retry status remain visible.
  iPad UI acceptance used `operation-ui-acceptance` in the isolated preview database:
  “Cleanup acceptance fixture”, “Finishing cleanup…”, the injected disk error, and
  the retry message appeared while Transmission was unreachable. No file mutation
  occurred; preview mode disables reconciliation. Android unit tests, Debug build,
  and lint pass; iOS Debug builds and the rendered iPad check pass.
  The card disappeared on refresh after the fixture was marked done. macOS and
  tvOS simulator Release builds also pass. Android rendered acceptance remains.
- Profile-switch concurrency audit found an Android race: repeated remote presses
  could queue multiple device-binding writes, while the immutable-API response
  guard accepted only the first local change. Selection now locks immediately on
  the UI thread, shows switching progress, blocks Back during the write, and restores
  the chooser after failure. Apple rejects overlapping store-level switches and
  duplicate picker actions; profile creation also checks that the server/device
  scope is unchanged before selecting the newly created user. iOS Debug builds
  pass. Rapid-input and delayed-response rendered acceptance remain outstanding.
- macOS now adds Next episode through AVPlayerView's public actionPopUpButtonMenu
  while the shared final-10-percent eligibility predicate is true. It uses the same
  transition path as automatic advancement and removes the menu when ineligible.
  This is a native menu action, not a permanently visible video overlay; rendered
  acceptance remains. iPhone/iPad has no public transport customization or controls
  visibility hook in the installed AVKit SDK. A user choice is pending between a
  final-10-percent overlay, a paused-only action, or automatic advancement only.
  Do not claim the iPhone/iPad early-next requirement is complete until that choice
  is resolved and the resulting UI is implemented and verified.
- Paused playback reports now preserve resume position without qualifying the title
  as watched. Apple streaming/offline and Android send explicit playback eligibility;
  actual-end events still mark completion. Legacy reports retain compatible behavior.
  Offline uploads carry the locally established watched eligibility instead of
  turning a paused seek into completion on the server. A focused server regression
  covers paused seek followed by real playback near the credits.
- Real Transmission 4.1.4 acceptance passed using an isolated loopback instance
  on Diamond with generated fixture files: move to Junk preserved payload and
  sidecars, paused seeding retained them, seed-policy completion detached the
  torrent and removed only owned files, and the neighbor remained intact. The test
  shut down both temporary processes and removed its temporary data. Household
  services/media were untouched. Evidence:
  `local-state/profiles-check/real-transmission-junk.log`; reusable targeted check:
  `tests/test_real_transmission_junk.py`. The first fixture attempt incorrectly
  started after setting an already-met ratio (Transmission disables that limit on
  manual restart); starting before applying the test policy corrected the fixture.
- Production server deployed on Diamond with the signed bundle. Schema 10 is live;
  seven existing history rows and 21 device bindings migrated to Jon. Profiles and
  operations endpoints responded successfully. Rollback bundle/database/config:
  `/Users/jont/Library/Application Support/Torrbaby/backups/profiles-release-20261110-073002`.
  No playback session was active immediately before deployment.
- Android 0.3.0 build 25 was built and signed with the existing update certificate,
  published to the Android download location, and installed successfully on JMGO
  at `192.168.1.206:5555`; package manager confirms versionCode=25/versionName=0.3.0.
  APK: `local-state/profiles-release/Torrbaby-0.3.0-build25.apk`.
  TCL delivery and Apple publication remain in progress.
- iPhone/iPad build 93 is published at the existing HTTPS install page, preserving
  build 92's concurrent-session changes. The HTTPS current.json reports build 93;
  signing covers six registered devices. Artifact: `builds/93-b0f7775d/Torrbaby.ipa`.
  macOS build 93 includes a Settings Switch user action that opens the main profile
  chooser window. Device tvOS signing is currently blocked: no installed provisioning
  profile for `org.torrbaby.tv` and no Xcode account configured. The simulator build
  is available; do not describe a tvOS device release as shipped.
- TCL build 25 delivery failed to connect to `192.168.1.201:6553`; no installation
  prompt was sent and no TCL update is confirmed. JMGO installation is confirmed.
- Live artwork preparation after deployment now has cached posters, backdrops,
  and logos for Outlander, Starship Troopers, and No Time to Die, which previously
  lacked cached background/logo assets. No pending artwork failure records were
  present at this check.
- Junk previews now capture per-torrent ratio and idle-seeding policy. After moving
  and resuming, reconciliation restores that policy because Transmission may disable
  an already-met limit on manual start. The snapshot is durable and included in the
  confirmation fingerprint. Deployed to Diamond; rollback bundle/database:
  `/Users/jont/Library/Application Support/Torrbaby/backups/profiles-policy-20261110-073517`.

## Profile appearance — 2026-11-10

Profiles now include `avatar_symbol`, `text_color`, and `background_color`.
An empty symbol displays the first initial. Initials, Unicode symbols and emoji
(including composed emoji) are supported. Colors use `#RRGGBB`; Apple offers a
native color picker plus swatches, and TV editors offer swatches and hex entry.
Appearance is stored on Diamond and shared by clients; device selection stays sticky.

Apple and Android have profile cards with an active checkmark and a separate edit
action. TV browsing headers show the current circular avatar and first name; selecting
it opens the switcher. Playback remains unobstructed. Editing appearance keeps the
same profile ID, so it does not change watch history or playback positions. Foregrounding
a client refreshes the active profile appearance from the server.

Validation: eight focused profile tests passed, covering independent history, watched
state, session ownership after switching, migration, and atomic appearance validation.
Mac switcher/editor and live EN/Chinese switching were inspected visually. All Apple
platform builds passed (tvOS simulator only); Android builds and the emulator's native
localization/routing check passed. Physical TV appearance still needs user review.

Diamond deployment backup: `backups/profile-appearance-20261110-112351` under the
server data directory. No recent playback was active at deployment; the pending
subtitle job was re-admitted with its existing checkpoint. Apple build 104 is published;
Android 0.3.8 build 34 is installed on JMGO. TCL installation is pending its on-screen confirmation.
