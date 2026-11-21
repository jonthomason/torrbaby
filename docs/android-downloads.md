# Downloads on Android phones and tablets (from build 132)

Like the iPhone's Downloads: a title or episode is kept on the phone to watch without the server.

- **Download** sits beside Play/Resume on a movie's page; each episode row has its own mark
  (progress, then a check). The menu's Library group has **Downloads**.
- Preparing (Saved.kt): every ready subtitle job is packed with the video into one MP4 by
  Diamond (`POST …/virtual-mp4`, then `/v1/virtual-jobs/<id>` until ready), as iOS does, so the
  copy plays with its subtitles offline. Without subtitle jobs the file itself is fetched.
  `download-info` gives the size; the credits analysis is kept for Next episode timing.
- Transferring (SavedService.kt): a foreground `dataSync` service with a progress notification,
  one item at a time, continuing with `Range` from the bytes already kept. Pause/Resume and
  Remove are on the row's ⋯. Downloads wait for Wi-Fi; *Allow cellular for 3 hours* lifts that,
  as on the iPhone. A lost network leaves the item waiting until a real network (not a VPN such
  as ABT) appears or the app is next opened. Android may end long transfers after some hours a
  day; what is left continues on the next opening.
- Files live in the app's private storage (`files/downloads/`, `index.json` lists them).
- Playing: a copy on the phone is always what plays (PlaybackActivity `saved` extra), from where
  the library last says it was left. Watching a copy is recorded with it and reported with
  `POST /v1/playback/offline` (event id, acknowledged once) when Diamond can next be reached.
- Offline: with no Wi-Fi, mobile or wired network the app opens straight to Downloads and
  reconnects by itself when a network appears.
