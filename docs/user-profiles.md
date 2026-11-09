# User profiles — queued after currently planned work

User requested separate users with individual watched history and resume positions.
Implement after the current download/offline handoff and planned subtitle/media work.
Do not conflate a person with a device: several people can use one iPad, and a person
can watch on multiple devices.

- Introduce stable server-owned profile IDs, display names and a simple profile picker
  in all apps. No external accounts or remote-access system required.
- Migrate today's shared history and positions into a default household profile;
  preserve every existing record. Existing clients use this default during migration.
- Key watch history, resume state, playback sessions and offline events by profile.
  Switching profile must not transfer another person's playback state.
- Capture profile ID when playback starts and in every durable offline event, including
  events retained after deleting downloaded files. Reconnection uploads to the captured
  profile, never whichever profile happens to be selected at sync time.
- A downloaded media file can be shared locally across profiles to avoid duplicate
  storage, but viewing state remains separate. Revisit delete-after-playback policy
  so one profile cannot unexpectedly remove media another person still needs.
- Scope subtitle defaults and per-title overrides per profile, migrating current values
  into the default profile. Devices are still independently named and linked.
- Validate migration, two profiles watching one episode, one profile on two devices,
  profile switches while offline, and deleted-download progress reconciliation.

Open product decisions for implementation time: profile icons, optional child profiles,
optional profile lock, and ownership of download retention. These are not prerequisites
for the initial simple profile picker and separate watch state.
