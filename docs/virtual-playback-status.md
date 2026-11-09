# Priority: virtual native playback

Active goal: play MKV and inject generated English/Chinese subtitles without
rewriting the entire movie. Client startup, seeking, and playback should have
minimal visible performance impact. Use bounded Diamond concurrency and keep
index construction/subtitle assembly off request-serving threads.

Deployed implementation (Apple build 91, Diamond Rust daemon):
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

Remaining engineering work: eliminate temporary sparse-file allocation; verify
physical-device behavior across more codecs. Shared base indexes, background
indexing, and full Apple client integration are implemented. Do not claim general codec transcoding: unsupported encoded media
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
  requests 2–6ms. Altered virtual identity/signature denied. Verified again in production after deployment.
- Offline client now persists virtual job identity, size, MP4 extension and ETag;
  resumable downloads use the authenticated virtual download endpoint. Existing
  downloads with no virtual identity retain their original endpoint.
- Shared base MKV indexes prevent reindexing when subtitle availability changes.
  A bounded idle worker preindexes completed MKV files; it yields admission while
  an indexed stream has a retained reader. Concurrent original-file streams are
  not yet used as an admission signal. No general transcoding was added.
- Clean release snapshot is local-state/virtual-mp4/release-snapshot. This excludes
  parallel profile/lifecycle edits. Apple iOS/macOS91 builds compile. Simulator
  full-app download and offline playback passed with the server tunnel stopped.
  macOS build 91 installed and launched; OTA 91 published and fetched successfully.

## Release and laced audio follow-up

- OTA: https://jons-m5-mbp.bengal-locrian.ts.net/ota/torrbaby/index.html (build 91).
- Full offline Harry Potter file includes English/Chinese native subtitle tracks;
  no network is required for its playback. Disposable full-file validation copy
  was deleted; the simulator retains its user-visible downloaded item.
- Production Harry Potter native startup: 0.516s. Warm authenticated ranges after
  the lacing deployment: 2–12ms; preparation reused its index in 0.022s.
- Discovered upstream Matroska lace frames all carry the block timestamp. Exposed
  lace_index and added exact per-frame codec duration; retained strict audio gap
  validation. Xiph/fixed/EBML lace checks pass.
- Seinfeld The Betrayal indexes in 3.99s; native ready 0.344s, English/Chinese/Off
  selection and forward/back/end seeks pass. Decoded video at beginning/middle/end.
  The Gymnast, The Butter Shave, and PONIES Night Moves also index successfully.
- Lacing fix deployed; old failed jobs retry once with preparation_version=3.
  Existing successful indexes remain valid. Latest deployment backup path is in
  Diamond's ~/Library/Application Support/Torrbaby/last-virtual-release-backup.txt.
- Launchctl bootstrap briefly raced bootout; a second bootstrap succeeded. Live
  authenticated playback checks passed. Saved subtitle work was re-admitted.
- Outlander AAC-LC with SBR sync extension is supported and deployed. Original
  ASC and audio bytes are preserved; output sample rate and per-packet sample
  count reflect SBR. Native startup 0.293s; both audio tracks decoded to 48kHz PCM
  at beginning/middle/end, with native caption selection and seeks passing.
- Production Matrix MP4 with actual generated Chinese and English captions:
  preparation 2.046s, native startup 0.364s; both languages, Off, and seeks pass.
- Matroska audio mapping reference: https://www.matroska.org/technical/codec_specs.html
  and output-frequency definition: https://www.matroska.org/technical/elements.html
- Remaining unsupported cases include codecs outside AVC/HEVC + AAC/AC3/EAC3,
  some AAC profiles/layouts, variable frame rate, and unusual track timing.
  These are rejected explicitly rather than silently transcoded. Bitmap subtitles
  are not extracted or OCRed. Source movie files remain untouched.

Parallel profile/library-lifecycle work remains uncommitted in the shared worktree
and is excluded from the clean release snapshot. Do not deploy the entire tree.

## Final bundle recovery and deployment guard

The first lacing/AAC deployments mistakenly used the CLI experiment bundle, which
contained only torrbaby and omitted the three Swift helpers. Playback worked but
translation jobs failed with "Start subtitle translator". Restored the complete
signed bundle from the known-good backup, retaining the tested new Rust binary.
The complete bundle is local-state/virtual-mp4/full-server/Torrbaby Server.app.
Production signature verification and authenticated stream checks pass. Three
failed jobs re-admitted directly; remaining 18 returned to the normal ordered
queue with their retry delay cleared. Confirmed the translation helper running
and reporting cue progress again. No source media was modified.

Always run scripts/verify_server_bundle.sh before deployment, including for
experimental bundles. It requires the translator, extractor, and segmenter as
well as the Rust binary and verifies the complete signature. Keep bootout and
bootstrap in separate SSH invocations; immediate bootstrap in the same SSH
process failed even with short retries. The latest complete-bundle backup is
rust-20261110-064605 (this backup contains the incomplete bundle; for helper
recovery use rust-20261110-063832 or rust-20261110-062811).
