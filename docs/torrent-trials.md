# Torrent trial sets (macOS first)

Find → Compare releases → select 3–5 releases → Start trial. Trials continues in
Diamond without the Mac app. The fastest candidate with any downloaded data is
promoted automatically after five minutes or after all candidates reach their
measurement limit. Zero-progress sets stop and notify the user for review.

The initial measurement target is 256 MiB per candidate with an 8 MiB/s download
cap and three-second polling. This is a soft cap: in-flight data and polling can
overshoot. If Diamond is down, Transmission can continue until the daemon returns.
Speed is total downloaded bytes divided by observed elapsed time, including startup;
connected and sending peers are displayed separately. This short trial cannot promise
that a swarm will remain healthy through the whole download.

New torrents start paused in source/.torrbaby-trials/<set>/<candidate>. These paths
are excluded from library discovery and automatic organization. Existing hashes
are excluded from the trial and are never stopped or deleted. Duplicate candidates
share the existing torrent and are excluded too. Persisted records recover an add
from its isolated directory if Transmission succeeded before a crash/timeout.

The winner is moved to the normal download folder through Transmission before
alternatives are removed with delete_local_data. Exact hash and isolated directory
ownership are checked. Destination collisions pause for review rather than overwrite.
Unexpected changes stop cleanup. No source files outside trial directories are deleted.

Known hashes deduplicate World and China search rows. Hashes learned from single-
attachment BT rows are persisted; multi-attachment posts are not collapsed into one
hash because they may represent several releases. Mac refreshes known hashes locally
without querying search providers. Ordinary BT Add also records single-release hashes.

Validation uses a fake local Transmission service: automatic promotion/move, loser
file removal, existing-torrent preservation, daemon restart recovery, zero-progress
pause, cancellation, and persisted hash mappings. No Chinese index queries or real
trial torrents were used for testing. Run cargo build, then python3
scripts/check_torrent_trials.py. Focused Rust selection/ownership checks are included.

Release: Mac build 124, signed Developer ID, installed at ~/Applications/Torrbaby.app.
Diamond trial/hash routes verified HTTP 200 after deployment. Search controls and
empty Trials panel visually checked in English and Simplified Chinese; populated
trial behavior checked with the local fixture, not live source searches.
