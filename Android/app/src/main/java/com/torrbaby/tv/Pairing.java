package com.torrbaby.tv;

import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.SecureRandom;
import java.util.Base64;
import javax.crypto.Cipher;
import javax.crypto.Mac;
import javax.crypto.spec.GCMParameterSpec;
import javax.crypto.spec.SecretKeySpec;
import org.json.JSONObject;

/**
 * Setting up a TV that is never at home (docs/remote-pairing.md, version 1). The TV shows a QR
 * code holding a fresh secret S; a paired phone hands S to Diamond; the TV then claims its setup
 * through bronze, sealed with a key derived from S, so bronze can neither read nor forge it.
 */
final class Pairing {
  /** A code is good for 10 minutes on Diamond. */
  static final long LIFETIME_MILLIS = 10 * 60 * 1000;

  final byte[] secret;
  final String id;
  final byte[] key;

  Pairing(byte[] secret) throws Exception {
    if (secret.length != 32) throw new IllegalArgumentException("The secret is 32 bytes.");
    this.secret = secret.clone();
    MessageDigest sha = MessageDigest.getInstance("SHA-256");
    sha.update("torrbaby-pair-id-v1".getBytes(StandardCharsets.US_ASCII));
    id = hex(sha.digest(secret)).substring(0, 32);
    key = hkdf(secret, "torrbaby-pair-v1".getBytes(StandardCharsets.US_ASCII), "seal".getBytes(StandardCharsets.US_ASCII), 32);
  }

  static Pairing fresh() throws Exception {
    byte[] s = new byte[32];
    new SecureRandom().nextBytes(s);
    return new Pairing(s);
  }

  /** What the QR code holds. */
  String code(String name) throws Exception {
    return new JSONObject().put("app", "torrbaby").put("kind", "join").put("v", 1)
        .put("secret", b64(secret)).put("name", name).put("platform", "android-tv").toString();
  }

  /** The claim's request body. */
  JSONObject claim() throws Exception { return new JSONObject().put("id", id); }

  /**
   * Opens a claim reply: null while not yet granted (any reply without "sealed"), the setup when
   * it opens. A sealed reply that does not open was not from Diamond: it throws, and the caller
   * keeps polling.
   */
  JSONObject open(JSONObject reply) throws Exception {
    String sealed = reply.optString("sealed", "");
    if (sealed.isEmpty()) return null;
    byte[] data = Base64.getUrlDecoder().decode(sealed);
    if (data.length < 12 + 16) throw new SecurityException("The setup reply is too short.");
    Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
    cipher.init(Cipher.DECRYPT_MODE, new SecretKeySpec(key, "AES"), new GCMParameterSpec(128, data, 0, 12));
    cipher.updateAAD(id.getBytes(StandardCharsets.US_ASCII));
    JSONObject setup = new JSONObject(new String(cipher.doFinal(data, 12, data.length - 12), StandardCharsets.UTF_8));
    if (setup.optInt("v") != 1 || setup.optString("token").isEmpty() || setup.optString("client_id").isEmpty())
      throw new SecurityException("The setup reply is incomplete.");
    return setup;
  }

  /** Seals as Diamond does (tests only: the TV never seals). */
  byte[] seal(byte[] nonce, byte[] plaintext) throws Exception {
    Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
    cipher.init(Cipher.ENCRYPT_MODE, new SecretKeySpec(key, "AES"), new GCMParameterSpec(128, nonce));
    cipher.updateAAD(id.getBytes(StandardCharsets.US_ASCII));
    byte[] body = cipher.doFinal(plaintext);
    byte[] out = new byte[nonce.length + body.length];
    System.arraycopy(nonce, 0, out, 0, nonce.length);
    System.arraycopy(body, 0, out, nonce.length, body.length);
    return out;
  }

  /** HKDF-SHA256 (RFC 5869). */
  static byte[] hkdf(byte[] ikm, byte[] salt, byte[] info, int length) throws Exception {
    Mac mac = Mac.getInstance("HmacSHA256");
    mac.init(new SecretKeySpec(salt, "HmacSHA256"));
    byte[] prk = mac.doFinal(ikm);
    mac.init(new SecretKeySpec(prk, "HmacSHA256"));
    byte[] out = new byte[length], block = new byte[0];
    for (int filled = 0, counter = 1; filled < length; counter++) {
      mac.update(block);
      mac.update(info);
      mac.update((byte) counter);
      block = mac.doFinal();
      int n = Math.min(block.length, length - filled);
      System.arraycopy(block, 0, out, filled, n);
      filled += n;
    }
    return out;
  }

  static String b64(byte[] data) { return Base64.getUrlEncoder().withoutPadding().encodeToString(data); }

  static String hex(byte[] data) {
    StringBuilder s = new StringBuilder();
    for (byte b : data) s.append(String.format("%02x", b));
    return s.toString();
  }
}
