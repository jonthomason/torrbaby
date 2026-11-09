# Automatic TV connection

Confirmed user direction: sideloaded TV clients on the trusted home LAN should
find Diamond and work automatically. No pairing code, token entry, phone approval,
or account creation as part of TV connection. This supersedes the four-digit code
proposal and the later mobile-approval suggestion.

Implementation plan for tvOS and Android TV:

- Advertise the Torrbaby service with Bonjour/mDNS; browse on the TV and remember
  the selected server. One discovered server connects automatically; multiple
  servers may need a simple server chooser.
- Bootstrap a device credential automatically for clients arriving from explicitly
  trusted local network ranges. Retain device identification/revocation separately
  from user profiles. Do not trust a client-supplied platform flag or forwarded IP
  header as proof of LAN membership; check the transport peer at the daemon.
- Inspect Diamond's listener and actual network interfaces before enabling LAN
  service. Current production listener uses its Tailnet address. Do not broaden to
  WAN or change routers/firewalls as a side effect of this app work.
- Test discovery and direct connectivity behind slate-seven. mDNS may not traverse
  that router boundary; no promise that Tailnet connectivity transports multicast.
  Seek a routed discovery/address solution if needed, keeping normal TV use automatic.
- Profiles remain a separate lightweight feature: mobile usually remembers one;
  shared TVs offer easy switching.

Implemented in build66: tvOS browses Bonjour and automatically registers the first
server when unconfigured. An address-only fallback remains for routed networks.
Diamond advertises _torrbaby._tcp and listens on 192.168.1.44:8765 alongside its
existing Tailnet listener. Only socket peers in 192.168.1.0/24 may bootstrap a
device credential. Forwarded headers confer no trust; normal APIs remain authenticated.
Registration is limited to ten attempts per peer per minute and 256 stored devices.

Verified: simulator compilation, live Bonjour resolution, both listeners require
API credentials, Tailnet registration with a spoofed LAN header is rejected, and
isolated registration/credential/rate-limit smoke checks. Physical Apple TV discovery
and discovery across slate-seven are still unverified. Android adoption is pending.
