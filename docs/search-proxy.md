# Search proxy operations

Since 2026-11-20 the search tunnel ends on **shadow**, reached over **ABT** (it was Tungsten,
in Hong Kong, over a Tailscale relay: slow, and retired). Only two HTTP clients in the daemon
use `TORRBABY_SEARCH_PROXY=socks5h://127.0.0.1:18780`: the torrent search sites' and TMDB's
(titles, details and posters). DNS for those requests resolves on shadow.

**Transmission never goes through it, and cannot.** Transmission has no proxy setting and its
sockets use the home network (checked: every peer socket on 192.168.1.x or the China Telecom
IPv6, none on ABT's 198.18.0.2). And shadow itself refuses: the tunnel's account may open only
the destinations below, so no peer, tracker or other site is reachable through it whatever
Diamond sends.

## Services

- Diamond: `~/Library/LaunchAgents/org.torrbaby.search-proxy.plist`, ordinary
  `/usr/bin/ssh -NT -D 127.0.0.1:18780` as `torrbaby-search@72.212.83.129` (shadow), with
  `ProxyCommand "~/Library/Application Support/ABT/bin/abt" connect %h %p` and
  `HostKeyAlias=shadow.jonthomason.com`. The previous (Tungsten) agent is kept under
  `~/Library/Application Support/Torrbaby/backups/search-proxy-tungsten-*.plist`.
- Shadow: system account `torrbaby-search` (no shell, `restrict,port-forwarding` key) and
  `/etc/ssh/sshd_config.d/60-torrbaby-search.conf`: public key only, local forwarding only,
  no TTY, agent, X11, tunnel device or stream forwarding, `ForceCommand /usr/sbin/nologin`, and

```
PermitOpen api.tmdb.org:443 image.tmdb.org:443 apibay.org:443 torrents-csv.com:443
           nyaa.si:443 www.1lou.me:80 share.dmhy.org:443 mikanani.me:443
```

Diamond's private key stays in `~/.ssh/torrbaby_search_ed25519`; shadow's host key is pinned
in `~/.ssh/torrbaby_search_known_hosts` (read from shadow over an authenticated session).
Neither is in this repository. A new search site needs its `host:port` added to `PermitOpen`
(`sshd -t`, then `systemctl reload ssh`).

## Validation (2026-11-20)

Through the tunnel: TMDB API 3.0 s (first request, tunnel setup), poster 0.76 s, apibay 1.2 s,
torrents-csv 1.2 s, nyaa 1.7 s. `example.com:443` and `api.themoviedb.org:443` refused in 0.2 s.

## Rollback

Put the backed-up plist back, `launchctl bootout` and `bootstrap` the agent. Removing
`TORRBABY_SEARCH_PROXY` from the daemon plist sends searches and TMDB straight from Diamond.
