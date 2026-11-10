# Search proxy operations

Deployed 2026-11-10. Only the daemon's indexer HTTP client uses
`TORRBABY_SEARCH_PROXY=socks5h://127.0.0.1:18780`. DNS for those requests resolves
on Tungsten. Transmission, media playback, TMDB, and global routing are unchanged.

## Services

- Diamond: `~/Library/LaunchAgents/org.torrbaby.search-proxy.plist`, ordinary
  `/usr/bin/ssh -NT -D 127.0.0.1:18780`, dedicated `torrbaby-search` account.
- Tungsten: `sshd-torrbaby.service` runs the installed OpenSSH daemon on its
  private Tailnet address, port 2222. This is **not Tailscale SSH**: ordinary
  public-key authentication and a pinned OpenSSH host key are required.
- Public port 22 was intermittent from Diamond. The additional listener binds
  only the private address and accepts only the dedicated account. Existing
  public SSH and Tailscale SSH settings were not changed.
- No custom runtime application is installed on Tungsten. The tunnel and server
  use OpenSSH, launchd, and systemd.

Diamond's dedicated private key stays in `~/.ssh/torrbaby_search_ed25519`.
Its verified host key is pinned in `~/.ssh/torrbaby_search_known_hosts`.
Neither is stored in this repository. The account has no shell sessions, agent
forwarding, remote forwarding, PTY, or tunnel-device access. `PermitOpen` allows:

```
apibay.org:443 torrents-csv.com:443 nyaa.si:443 www.1lou.me:80
share.dmhy.org:443 mikanani.me:443
```

The tunnel restarts through launchd, with SSH keepalives and a 30-second restart
throttle. Logs are in `~/Library/Logs/Torrbaby/search-proxy*.log` on Diamond.

## Validation

All six sources returned HTTP 200 through the SOCKS proxy. An unrelated
`example.com:443` request was refused by the destination allowlist.
Live API: China `逐玉 全40` returned 16 results in 0.78s; listing 964046 resolved
43 files (40 episodes), 29,038,658,781 bytes, in 15.81s; World CSV search took
1.14s. Cross-market provider selection returned 400. Upstream failures remain
possible; these measurements are successful checks, not a latency guarantee.

## Rollback

Before enabling the environment variable, the daemon plist, SQLite database,
and three pending subtitle jobs were backed up on Diamond under
`~/Library/Application Support/Torrbaby/backups/search-proxy-20261110-100342`.
All three jobs were requeued after the brief restart.

To revert routing, remove only `TORRBABY_SEARCH_PROXY` from the daemon plist and
restart the daemon, preserving pending subtitle jobs. Then boot out the dedicated
search-proxy LaunchAgent. Do not restore the old database or touch the separate
Firefox proxy. Tungsten's dedicated service/account can subsequently be disabled
if no longer needed. Client builds do not need to change for proxy configuration.
