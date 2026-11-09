# Offline watch progress

The WIP app stores the latest local watch report independently of its media file.
Deleting a downloaded copy retains this report. Each new report has a durable UUID
and a separate observation timestamp; pausing a transfer, pinning, deleting or
acknowledging it does not change that observation timestamp.

When connected, the app registers its client and posts:

`POST /v1/playback/offline`

```json
{"event_id":"UUID","client_id":"UUID","asset_id":"asset-ID","position":25,"duration":100,"recorded_at":1794220000,"reached_end":false}
```

Normal API authentication is required. The server resolves the asset to the same
media identity used by streaming. It does not read the media file to accept watch
progress. Missing catalog assets leave reports pending rather than silently losing
history.

Schema 7 adds a receipt table keyed by client/event UUID. Retries return the same
receipt and do not reapply progress. The app clears only the exact acknowledged
event, so an in-flight response cannot erase a newer local report. A report older
than the server's last playback activity is acknowledged but not applied. A newer
report is deferred without acknowledgment while another streaming session has
reported within 30 seconds and has not ended. It retries on later refreshes.

On acceptance, the server updates shared position/duration/watched state and
invalidates the previous stream session. Replaying from the beginning can clear
watched state; ending or reaching 95 percent marks it watched. Playback source
files and torrent states are never changed by this route.

Ordering currently uses device wall-clock observation time. More than five minutes
in the future is rejected; smaller positive skew is clamped to server receive time.
Devices need reasonably correct clocks. Receipts currently persist indefinitely;
retention/compaction remains future work. This is not a vector-clock reconciliation
protocol. Before playing a downloaded copy while connected, the app reads
`GET /v1/library/assets/{asset-id}/progress` with a three-second request timeout.
A strictly newer shared observation updates the local resume position and
supersedes older pending reports. A failed request falls back to local progress.
Imported watched state never triggers local deletion. Library Play prefers a
ready download, as does same-season next-episode playback. This still needs
physical-device acceptance checks; native background transfers and clock skew
remain material limits.

Validation: temporary HTTP fixture covers authentication, retry deduplication,
newer/stale ordering, active playback deferral/retry, completion, future time rejection
and schema version. Local storage fixture covers progress surviving file deletion
and restart, stale acknowledgments, and durable acknowledgment of the latest event.
