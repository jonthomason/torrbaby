//! One-use pairing grants and independently revocable device credentials.
use crate::storage::{Database, now};
use anyhow::{Result, bail};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn secret() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
fn digest(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn authorized(db: &Database, bearer: &str) -> bool {
    let Some(token) = bearer.strip_prefix("Bearer ") else {
        return false;
    };
    if token.len() != 64 {
        return false;
    }
    db.connection
        .query_row(
            "SELECT 1 FROM device_credentials WHERE digest=?",
            [digest(token)],
            |_| Ok(()),
        )
        .optional()
        .ok()
        .flatten()
        .is_some()
}

pub fn handle(db: &mut Database, method: &str, path: &str, body: &Value) -> Result<Value> {
    if method == "POST" && path == "/v1/pairings" {
        db.connection
            .execute("DELETE FROM pairing_grants WHERE expires<?", [now()])?;
        let count: i64 =
            db.connection
                .query_row("SELECT COUNT(*) FROM pairing_grants", [], |r| r.get(0))?;
        if count >= 16 {
            bail!("Too many pairing codes. Close an existing code or wait five minutes.")
        }
        let code = secret();
        let id = uuid::Uuid::new_v4().to_string();
        let expires = now() + 300.0;
        db.connection.execute(
            "INSERT INTO pairing_grants(id,digest,expires) VALUES(?,?,?)",
            params![id, digest(&code), expires],
        )?;
        return Ok(json!({"id":id,"code":code,"expires":expires}));
    }
    if method == "POST" && path == "/v1/pairings/redeem" {
        let code = body["code"].as_str().unwrap_or("");
        if code.len() != 64 {
            bail!("Invalid or expired connection code")
        }
        let name = body["name"].as_str().unwrap_or("New device").trim();
        let platform = body["platform"].as_str().unwrap_or("unknown");
        if name.is_empty() || name.len() > 200 || platform.len() > 40 {
            bail!("Invalid device name")
        }
        let tx = db.connection.transaction()?;
        let id: Option<String> = tx
            .query_row(
                "SELECT id FROM pairing_grants WHERE digest=? AND expires>? AND client_id IS NULL",
                params![digest(code), now()],
                |r| r.get(0),
            )
            .optional()?;
        let Some(id) = id else {
            bail!("Invalid or expired connection code")
        };
        let client = uuid::Uuid::new_v4().to_string();
        let token = secret();
        tx.execute(
            "INSERT INTO clients(id,name,platform,created,last_seen) VALUES(?,?,?,?,?)",
            params![client, name, platform, now(), now()],
        )?;
        tx.execute(
            "INSERT INTO device_credentials(client_id,digest) VALUES(?,?)",
            params![client, digest(&token)],
        )?;
        tx.execute(
            "UPDATE pairing_grants SET client_id=? WHERE id=?",
            params![client, id],
        )?;
        tx.commit()?;
        return Ok(json!({"client_id":client,"token":token}));
    }
    if let Some(id) = path.strip_prefix("/v1/pairings/") {
        if method == "DELETE" {
            db.connection
                .execute("DELETE FROM pairing_grants WHERE id=?", [id])?;
            return Ok(json!({"ok":true}));
        }
        if method == "GET" {
            let result: Option<(f64, Option<String>)> = db
                .connection
                .query_row(
                    "SELECT expires,client_id FROM pairing_grants WHERE id=?",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((expires, client)) = result else {
                bail!("Connection code no longer exists")
            };
            return Ok(json!({"expires":expires,"paired":client.is_some(),"client_id":client}));
        }
    }
    if method == "DELETE" {
        if let Some(id) = path.strip_prefix("/v1/device-credentials/") {
            db.connection
                .execute("DELETE FROM device_credentials WHERE client_id=?", [id])?;
            return Ok(json!({"ok":true}));
        }
    }
    bail!("Unknown pairing action")
}

/// Remote pairing (docs/remote-pairing.md): a device that is never at home shows a QR code with
/// a secret; a paired device hands the secret to Diamond; the new device claims its setup
/// through bronze, sealed with a key from the secret, which bronze never sees.
pub mod remote {
    use super::*;
    use base64::Engine;
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct Grant { key: [u8; 32], name: String, platform: String, expires: f64 }
    static GRANTS: Mutex<Option<HashMap<String, Grant>>> = Mutex::new(None);
    const LIFETIME: f64 = 600.0;

    fn b64() -> base64::engine::GeneralPurpose { base64::engine::general_purpose::URL_SAFE_NO_PAD }

    /// The grant's id and sealing key from the new device's secret.
    pub fn derive(secret: &[u8]) -> (String, [u8; 32]) {
        let mut hasher = Sha256::new();
        hasher.update(b"torrbaby-pair-id-v1");
        hasher.update(secret);
        let id: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>()[..32].to_owned();
        struct Len32;
        impl ring::hkdf::KeyType for Len32 { fn len(&self) -> usize { 32 } }
        let mut key = [0u8; 32];
        ring::hkdf::Salt::new(ring::hkdf::HKDF_SHA256, b"torrbaby-pair-v1").extract(secret)
            .expand(&[b"seal"], Len32).expect("32 bytes is a valid HKDF length")
            .fill(&mut key).expect("filled");
        (id, key)
    }

    /// AES-256-GCM: random 12-byte nonce ‖ ciphertext ‖ tag, associated data `id`.
    pub fn seal(key: &[u8; 32], id: &str, plaintext: &[u8]) -> Result<Vec<u8>> {
        use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
        use ring::rand::{SecureRandom, SystemRandom};
        let mut nonce = [0u8; 12];
        SystemRandom::new().fill(&mut nonce).map_err(|_| anyhow::anyhow!("No random numbers"))?;
        let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).map_err(|_| anyhow::anyhow!("Bad key"))?);
        let mut data = plaintext.to_vec();
        key.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from(id.as_bytes()), &mut data)
            .map_err(|_| anyhow::anyhow!("Sealing failed"))?;
        Ok([nonce.as_slice(), &data].concat())
    }

    /// POST /v1/pairings/remote, from a paired device: Diamond keeps the grant for 10 minutes.
    pub fn grant(body: &Value) -> Result<Value> {
        let secret = b64().decode(body["secret"].as_str().unwrap_or("")).map_err(|_| anyhow::anyhow!("This code isn't a Torrbaby setup code"))?;
        if secret.len() != 32 { bail!("This code isn't a Torrbaby setup code") }
        let name = body["name"].as_str().unwrap_or("New device").trim().chars().take(200).collect::<String>();
        let platform = body["platform"].as_str().unwrap_or("unknown").chars().take(40).collect::<String>();
        let (id, key) = derive(&secret);
        let expires = now() + LIFETIME;
        let mut held = GRANTS.lock().map_err(|_| anyhow::anyhow!("Pairing unavailable"))?;
        let grants = held.get_or_insert_with(HashMap::new);
        grants.retain(|_, g| g.expires > now());
        if grants.len() >= 16 && !grants.contains_key(&id) { bail!("Too many setup codes waiting. Try again in a few minutes.") }
        grants.insert(id.clone(), Grant { key, name: if name.is_empty() { "New device".into() } else { name }, platform, expires });
        Ok(json!({"id": id, "expires": expires}))
    }

    /// POST /v1/pairings/remote/claim, from the new device through bronze (no credentials):
    /// waiting until granted; then the device's credentials and Diamond's details (`setup`:
    /// server, relay, routes), sealed. The grant is used once.
    pub fn claim(db: &mut Database, body: &Value, setup: Value) -> Result<Value> {
        let id = body["id"].as_str().unwrap_or("");
        let grant = {
            let mut held = GRANTS.lock().map_err(|_| anyhow::anyhow!("Pairing unavailable"))?;
            let grants = held.get_or_insert_with(HashMap::new);
            grants.retain(|_, g| g.expires > now());
            grants.remove(id)
        };
        let Some(grant) = grant else { return Ok(json!({"waiting": true})) };
        let client = uuid::Uuid::new_v4().to_string();
        let token = secret();
        let tx = db.connection.transaction()?;
        tx.execute("INSERT INTO clients(id,name,platform,created,last_seen) VALUES(?,?,?,?,?)", params![client, grant.name, grant.platform, now(), now()])?;
        tx.execute("INSERT INTO device_credentials(client_id,digest) VALUES(?,?)", params![client, digest(&token)])?;
        tx.commit()?;
        let mut payload = setup;
        payload["v"] = json!(1);
        payload["client_id"] = json!(client);
        payload["token"] = json!(token);
        let sealed = seal(&grant.key, id, payload.to_string().as_bytes())?;
        Ok(json!({"sealed": b64().encode(sealed)}))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        /// The vector in docs/remote-pairing.md: S = 32 bytes 0x00..0x1f.
        #[test]
        fn derivation_matches_the_documented_vector() {
            let secret: Vec<u8> = (0u8..32).collect();
            let (id, key) = derive(&secret);
            let key_hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
            println!("secret b64 {}\nid {id}\nkey {key_hex}", b64().encode(&secret));
            // Opening what was sealed gives the plaintext back (and fails with another id).
            use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
            let sealed = seal(&key, &id, b"{\"v\":1}").unwrap();
            let open = |aad: &str| {
                let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &key).unwrap());
                let mut data = sealed[12..].to_vec();
                key.open_in_place(Nonce::try_assume_unique_for_key(&sealed[..12]).unwrap(), Aad::from(aad.as_bytes()), &mut data).map(|p| p.to_vec())
            };
            assert_eq!(open(&id).unwrap(), b"{\"v\":1}");
            assert!(open("another").is_err());
            // The documented sealed message: nonce 00..0b, plaintext {"v":1,"token":"t"}.
            let fixed = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &key).unwrap());
            let mut data = br#"{"v":1,"token":"t"}"#.to_vec();
            let nonce: [u8; 12] = std::array::from_fn(|i| i as u8);
            fixed.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from(id.as_bytes()), &mut data).unwrap();
            let sealed = b64().encode([nonce.as_slice(), &data].concat());
            println!("sealed {sealed}");
            assert_eq!(sealed, "AAECAwQFBgcICQoLFh39D-g3CmZl7iLvhggJBrTzw2klgw-1rECGYU5IvYKQ3jQ");
        }
    }
}
