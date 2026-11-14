# Remote pairing: a device that is never at home

A new device (an Android TV, a family iPhone) that will never be on the home network still has
to learn three things it cannot get through bronze unchecked: its credentials, Diamond's
identity (the certificate fingerprint that end-to-end encryption pins), and how to reach
Diamond (bronze's address and fingerprint). bronze carries the exchange but must be unable to
read or forge it.

## The idea

The new device shows a QR code holding a fresh 256-bit secret. A device that is already paired
(and knows Diamond's identity) scans it and hands the secret to Diamond over its own trusted
channel. The new device then asks Diamond, through bronze, for its setup; Diamond answers
sealed with a key derived from the secret. bronze never sees the secret, so it can neither read
nor forge the answer. A typed code would be too short: bronze could guess it offline.

If nobody with a paired phone is near the new device, its owner photographs the QR code and
sends the picture; the secret is single-use and expires in 10 minutes.

## Protocol (version 1)

Notation: `S` is 32 random bytes from the new device's secure random source. `b64` is base64url
without padding. `hex` is lowercase hex.

1. **New device** makes `S` and computes
   - `id  = hex(SHA-256("torrbaby-pair-id-v1" ‖ S))[0..32]` (32 hex characters)
   - `key = HKDF-SHA256(ikm = S, salt = "torrbaby-pair-v1", info = "seal", length = 32)`
   It shows a QR code with the JSON
   `{"app":"torrbaby","kind":"join","v":1,"secret":"<b64 S>","name":"<device name>","platform":"<android-tv|ios|…>"}`.

2. **Paired device** scans it and sends, authenticated as itself:
   `POST /v1/pairings/remote` with `{"secret":"<b64 S>","name":…,"platform":…}`.
   Diamond derives `id` and `key` the same way and keeps the grant in memory for 10 minutes
   (at most 16 at once). Reply: `{"id":"<id>","expires":<unix seconds>}`.

3. **New device**, through bronze's relay (plain: it has no identity to pin yet; the reply is
   sealed), polls every 2 s:
   `POST /v1/pairings/remote/claim` with `{"id":"<id>"}` (no credentials).
   - Not granted yet (or unknown or expired id): HTTP 200, `{"waiting":true}`.
   - Granted: Diamond creates the client and its credential (as `/v1/pairings/redeem` does),
     forgets the grant (single use), and replies `{"sealed":"<b64 nonce ‖ ciphertext>"}` where
     the ciphertext is AES-256-GCM under `key`, 12-byte random `nonce`, associated data = the
     ASCII `id`, over the JSON
     ```
     {"v":1,"client_id":…,"token":…,
      "server":{"id":…,"name":…,"identity":"<64 hex>"},
      "relay":{"address":"ip:port","fingerprint":"<64 hex>"},
      "routes":{"home":…,"away":…}}
     ```
   The new device opens it with `key`; a failure to open means it was not from Diamond and is
   ignored (the device keeps polling until its 10 minutes run out).

4. **New device** keeps the credentials, pins `server.identity` for end-to-end encryption of
   every relayed stream from then on, and uses the relay (and later the direct path).

## Bootstrapping bronze

A device that has never been paired needs bronze's address and certificate fingerprint before
step 3. The apps carry them built in (`47.112.18.69:7443`,
`c0fcde9603eec131f8d4a7ace67e7aad2900e5f9b6a59c6818106b0a51d6d7d6`); pairing replaces them with
Diamond's current values. These are not secret: end-to-end encryption, not bronze's
certificate, protects the traffic.

## Test vector

(server/pairing.rs `derivation_matches_the_documented_vector`; id and key also checked with
Python's hashlib/hmac.)

- `S` = bytes 0x00, 0x01, … 0x1f; as b64: `AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8`
- `id` = `7eba2290a9f2307e661a6331d864dfac`
- `key` = `f8b6511f9653cedb74f0fcf38c10ae5d89513a82eb9460b42af1cb1d523f45ba`
- Sealing `{"v":1,"token":"t"}` with nonce bytes 0x00 … 0x0b and associated data `id` gives
  `sealed` = `AAECAwQFBgcICQoLFh39D-g3CmZl7iLvhggJBrTzw2klgw-1rECGYU5IvYKQ3jQ`
