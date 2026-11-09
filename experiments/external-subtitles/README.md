# External subtitle experiment — 2026-11-10

Isolated from production Torrbaby and Diamond. No daemon changes or production
playback changes. User explicitly prefers Torrbaby-owned subtitle selection
(Off/language list) over Apple's subtitle controls; retain native transport/PiP.

## Results

- Existing owned two-minute AVC/audio test fixture: Android/.local/sample.mp4.
- Generated subtitle-only MP4 is 1,489 bytes. Existing local ffmpeg was used only
  as a fixture authoring tool, not introduced as a production dependency.
- AVMutableComposition references the original audiovisual tracks and separate
  English/Chinese tx3g tracks; no movie export or full media duplication.
- macOS assembled in 12–14ms; player ready in 0.17–0.38s. Exact seeks to 1s, 6s,
  and 2s passed. These are LOCAL fixture results, not remote startup benchmarks.
- Subtitle-only source reports a legible group. Composition has both subtitle
  tracks but no legible selection group. Explicit AVPlayerItemTrack.isEnabled
  switching eng/zho/off yielded no AVPlayerItemLegibleOutput callbacks.
- Repeating with composition .text tracks instead of .subtitle did not fix it;
  item reported an operation error. This variant is only in local-state.
- iPadOS 26.5 simulator: composition ready in 0.32–0.40s, native video and seek
  controls work, native subtitle choice count is zero. Native captions were not
  visibly rendered in the first cue.
- Added a diagnostic time-synchronized UILabel to contentOverlayView and custom
  Off/English/Chinese segmented control. This is a capability probe, not a real
  WebVTT renderer or proposed final UI.
- Activated actual native PiP through Device Hub: delegate confirmed PiP started,
  video appeared in floating window; subtitle label remained in the original
  app behind the PiP placeholder. The PiP window contained no overlay captions.
- Stopped the test app after inspection. All probe players now explicitly mute
  audio. User permits audio when useful but requires stopping it after testing.

## Conclusion

A plain overlay fails the required captions-in-PiP behavior. The composition
approach tested here also fails native caption rendering, even with our own
track controls. This does not prove all possible composition approaches fail.
Do not replace production HLS based on these experiments.

Next candidate to investigate: a virtual MP4 served by Diamond, combining a
small generated header/subtitle payload with byte ranges from the untouched
original movie. Native player would see actual subtitle tracks, without a full
copy on disk. Requires careful container offset/timing/range validation and
format-specific support; not implemented or proven here. Native menu selection
would still be replaced by app-owned selection using the native media group.

## Reproduce desktop probe

From repository root, with the existing ffmpeg installation and owned fixture:

```sh
mkdir -p local-state/external-subtitles
ffmpeg -v error -n -i experiments/external-subtitles/en.srt \
  -i experiments/external-subtitles/zh.srt -map 0 -map 1 -c:s mov_text \
  -metadata:s:s:0 language=eng -metadata:s:s:1 language=zho \
  local-state/external-subtitles/captions.mp4
swiftc -parse-as-library experiments/external-subtitles/Probe.swift \
  -o local-state/external-subtitles/probe
local-state/external-subtitles/probe \
  "file://$PWD/Android/.local/sample.mp4" local-state/external-subtitles/captions.mp4
```

The isolated iPad app source is iPadProbe.swift. Its installed bundle identifier
is org.torrbaby.subtitle-experiment; it bundles sample.mp4 and captions.mp4.
App binary and generated media live in ignored local-state/external-subtitles.
Simulator: 4E2EEF4D-440B-44AB-B4E3-58C9963DEE46. No main app was replaced.
