# Search proxy operations

Since 2026-11-20 the search tunnel ends on **shadow**, reached over **ABT** (it was Tungsten,
in Hong Kong, over a Tailscale relay: slow, and retired). Only two HTTP clients in the daemon
use `TORRBABY_SEARCH_PROXY=socks5h://127.0.0.1:18780`: the torrent search sites' and TMDB's
(titles, details and posters). DNS for those requests resolves on shadow.

**Transmission never goes through it, and cannot.** Transmission has no proxy setting and its
sockets use the home network (checked: every peer socket on 192.168.1.x or the China Telecom
IPv6, none on ABT's 198.18.0.2). And shadow itself refuses: ABT's exit lets the `diamond`
device reach only the destinations below, TCP only (abt-server `[exit_only]`, ABT commit
a92e71a), so no peer, tracker or DHT node is reachable through shadow whatever Diamond sends,
its ABT VPN included. Refusals are logged: `journalctl -u abt-server | grep "exit refused"`.

## Services

- Diamond: `~/Library/LaunchAgents/org.torrbaby.search-proxy.plist` runs
  `~/Library/Application Support/ABT/bin/abt socks ~/.config/abt/search.toml`, a SOCKS5 proxy on
  127.0.0.1:18780 through ABT's exit as device `diamond` (search.toml is client.toml's key and
  server lines plus `socks`; owner only, not in this repository). Earlier agents (SSH to
  Tungsten, then SSH to shadow) are kept under `~/Library/Application Support/Torrbaby/backups/`.
- Shadow: `/etc/abt/server.toml`:

```
[exit_only]
diamond = ["api.tmdb.org:443", "image.tmdb.org:443", "apibay.org:443", "torrents-csv.com:443",
           "nyaa.si:443", "www.1lou.me:80", "share.dmhy.org:443", "mikanani.me:443"]
```

  A new search site needs its `host:port` added there and abt-server restarted (ABT's
  deploy/shadow.sh arms the dead-man switch). Backup before the change:
  `/etc/abt/server.toml.before-exit-only`. The SSH account `torrbaby-search` on shadow
  (sshd `PermitOpen` with the same list) is unused now and can be removed.

## Validation (2026-11-20)

Through `abt socks`: TMDB API 1.2 s, poster 0.55 s, apibay 0.5-0.7 s, nyaa 1.1 s; example.com
refused by the exit in 0.2 s. Find's title search (12 results) went from 13-15 s to 0.6-2.6 s.

## Rollback

Put a backed-up plist back, `launchctl bootout` and `bootstrap` the agent. Removing
`TORRBABY_SEARCH_PROXY` from the daemon plist sends searches and TMDB straight from Diamond.
