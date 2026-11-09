# Interface direction

Use imagery and clear controls first, with fewer words throughout every Torrbaby
app. This applies to all pages, not only setup.

- Prefer posters, thumbnails, progress graphics and recognizable system icons.
- Keep action labels short and specific. Add accessibility labels and tooltips
  where an icon alone would be ambiguous.
- Omit decorative slogans, redundant headings and explanations of implementation.
- Show explanations only where they help the person make a decision or recover
  from a problem. Keep destructive consequences explicit.
- Keep required provider attribution, useful errors and meaningful status.
- Mobile setup uses a centered QR scan action; no server-address or token fields.
- Keep the native Apple player and its controls.

## Torrent ordering (all clients, including Android)

Remember the selected order independently per list on each client:

| List | Default | Alternatives |
| --- | --- | --- |
| Downloading | Percentage complete, highest first | Date added (newest first), alphabetical |
| Seeding | Upload ratio, highest first | Date added (newest first) |
| Complete | Alphabetical | Date added (newest first), size (largest first) |

Use natural, case-insensitive title order for alphabetical sorting; break equal primary values by title then stable torrent identity. Sorting occurs after filtering/search. Downloads here means the torrent Downloading list, not the separate offline media download queue. Apple clients share `TorrentSort` in Models.swift; preferences are `torrentSort.downloading`, `torrentSort.seeding`, `torrentSort.complete`. Android implementation belongs to the concurrent Android session.

### App release identification

Every client must display its installed app version and build number at the top of Settings, including while disconnected. Use bundle/package metadata, never a hard-coded release string. Apple format: `Version 0.2.0 · Build 59`. Server version is separate. Android should use versionName/versionCode for the same presentation.

### Shared subtitle preferences (schema 8; client integration pending)

`GET /v1/subtitle-preferences` reads household defaults; `PUT` replaces them.
`GET`/`PUT /v1/library/titles/{id}/subtitle-preferences` reads/sets a whole-title
override; `DELETE` resets that title to inherited defaults. All require normal
API authentication. These preferences are shared across all clients, not keyed
by device. A TV series title covers its episodes.

PUT body: `enabled` boolean, `language` (`original`, `en`, `zh-Hans`), `font`
(`default`, `sans-serif`, `serif`, `monospace`), `size_percent` integer 50–200,
`text_opacity` and `background_opacity` integers 0–100. Unknown fields/invalid
values/unknown titles are rejected. Responses contain `defaults`, `effective`,
and `overridden`. Initial defaults: off, original language, default font, 100%
size/text opacity, 0% background opacity. Reset removes the entire override.
Changing a default does not change existing title overrides. Clients must apply
these through native player selection/styling and refresh when starting playback;
this API alone does not create a playable translated track.
