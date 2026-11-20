# Search proxy operations

Since 2026-11-20 the search tunnel ends on **shadow**, reached over **ABT** (it was Tungsten,
in Hong Kong, over a Tailscale relay: slow, and retired). Only two HTTP clients in the daemon
use `TORRBABY_SEARCH_PROXY=socks5h://127.0.0.1:18780`: the torrent search sites' and TMDB's
(titles, details and posters). DNS for those requests resolves on shadow.

**Transmission never goes through it, nor through ABT at all.** Since 2026-11-20 it runs as jont
in its own group, `_torrbaby_bt` (gid 442), from `/Library/LaunchDaemons/org.torrbaby.transmission.plist`
with no proxy variables, and pf (anchor `com.apple/torrbaby-bt`, `/etc/pf.anchors/torrbaby-bt`,
kept loaded by `org.torrbaby.pf`) lets that group out only on en0, plus its RPC replies from
port 9091 on lo0; everything else it sends (ABT's and Tailscale's utun, en1, the local SOCKS
port) is dropped and logged. Proven by sending as gid 442 through each. brew's own agent is
off (`sh.brew.transmission-cli.plist.off`): never `brew services start transmission-cli`, which
would start an unrestricted copy. With that in place Diamond's use of ABT's exit is unrestricted
(the `[exit_only]` allowlist that briefly limited it is removed; backup on shadow:
`/etc/abt/server.toml.before-diamond-unrestricted`).

## Services

- Diamond: `~/Library/LaunchAgents/org.torrbaby.search-proxy.plist` runs
  `~/Library/Application Support/ABT/bin/abt socks ~/.config/abt/search.toml`, a SOCKS5 proxy on
  127.0.0.1:18780 through ABT's exit as device `diamond` (search.toml is client.toml's key and
  server lines plus `socks`; owner only, not in this repository). Earlier agents (SSH to
  Tungsten, then SSH to shadow) are kept under `~/Library/Application Support/Torrbaby/backups/`.
- Shadow: nothing specific to Torrbaby. The SSH account `torrbaby-search` (sshd `PermitOpen`)
  from the first move is unused and can be removed.

## Validation (2026-11-20)

Through `abt socks`: TMDB API 1.2 s, poster 0.55 s, apibay 0.5-0.7 s, nyaa 1.1 s; example.com
refused by the exit in 0.2 s while the allowlist stood. Find's title search (12 results) went from 13-15 s to 0.6-2.6 s.

## Rollback

Put a backed-up plist back, `launchctl bootout` and `bootstrap` the agent. Removing
`TORRBABY_SEARCH_PROXY` from the daemon plist sends searches and TMDB straight from Diamond.
