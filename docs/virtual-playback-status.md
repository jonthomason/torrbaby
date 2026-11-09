# Priority: virtual native playback

Active goal: play MKV and inject generated English/Chinese subtitles without
rewriting the entire movie. Client startup, seeking, and playback should have
minimal visible performance impact. Use bounded Diamond concurrency and keep
index construction/subtitle assembly off request-serving threads.

Current implementation work (not yet deployed):
- Persistent MKV packet indexes, packed metadata, original read-only payloads.
- Source revision validation, authenticated virtual range URLs, single preparation
  worker, cached opened readers, 256KB stream chunks on blocking worker threads.
- Native tx3g tracks constructed from completed subtitle artifacts and virtually
  appended alongside a replacement moov. Original mdat bytes are untouched.
- Apple preparation flow requests the virtual container rather than HLS copying.

Acceptance before completion: real MKV and MP4 native playback; generated English
and Chinese selection and visible captions; seeks, iPad playback/PiP; bounded
storage and responsive range serving; deploy server/client builds and record
remaining codec restrictions. No destructive conversion or modification of seeding
files. Keep the daemon headless. Preserve parallel profile/library work.

Known remaining engineering work: eliminate temporary sparse-file allocation;
avoid repeat MKV indexing for changes to available subtitles; pre-index completed
catalog items in the background; verify full Apple client integration and physical
device behavior. Do not claim general codec transcoding: unsupported encoded media
must fail clearly or take a separately supported fallback.

## Combined playback evidence

2026-11-10: full Philosopher's Stone MKV served from Diamond with its actual
completed English and generated Chinese subtitle artifacts. Native desktop ready
0.518s; English/Chinese/Off selection plus forward/back/end seeks passed. iPad
simulator ready 0.81s; generated Chinese visibly rendered over the movie and inside
native PiP. No overlay subtitle renderer. Apple's subtitle handler must be `sbtl`:
`text` exposed language choices but did not render our tx3g samples.

## Offline and authenticated path

- Downloaded the complete authenticated virtual Harry Potter response into a
  disposable local MP4: 2,859,839,947 bytes in 18.82s. Native local-file playback
  ready in 0.254s; English/Chinese/Off and forward/back/end seeks passed with no
  server involved in playback.
- Isolated authenticated server preparation took 19.1s cold; warmed HEAD/range
  requests 2–6ms. Altered virtual identity/signature denied. No daemon restart yet.
- Offline client now persists virtual job identity, size, MP4 extension and ETag;
  resumable downloads use the authenticated virtual download endpoint. Existing
  downloads with no virtual identity retain their original endpoint.
- Shared base MKV indexes prevent reindexing when subtitle availability changes.
  A bounded idle worker preindexes completed MKV files; it yields admission while
  an indexed stream has a retained reader. Concurrent original-file streams are
  not yet used as an admission signal. No general transcoding was added.
- Clean release snapshot is local-state/virtual-mp4/release-snapshot. This excludes
  parallel profile/lifecycle edits. Apple iOS/macOS91 builds compile. Simulator
  debug build compiles; full app UI acceptance and OTA publication pending.
