package com.torrbaby.tv;

import android.content.Context;
import android.content.SharedPreferences;
import org.json.JSONObject;

/**
 * Torrbaby's relay on bronze, for a TV away from home: QUIC to bronze (the Rust code the iPhone
 * uses, Android/native), with a loopback port whose every connection bronze splices to Diamond.
 * Once Diamond's identity is pinned, each relayed stream is TLS to Diamond end to end, so bronze
 * carries traffic it cannot read.
 */
final class Relay {
  /** Built in for a TV that has never been paired; pairing replaces them (docs/remote-pairing.md). */
  static final String DEFAULT_ADDRESS = "47.112.18.69:7443",
      DEFAULT_FINGERPRINT = "c0fcde9603eec131f8d4a7ace67e7aad2900e5f9b6a59c6818106b0a51d6d7d6";

  private static final boolean LOADED = load();
  private static int port;
  private static String address = "";

  private static boolean load() {
    try { System.loadLibrary("tbandroid"); return true; } catch (Throwable missing) { return false; }
  }

  private static native int nativeStart(String address, String fingerprint);
  private static native boolean nativeAlive();
  private static native void nativeStop();
  private static native void nativeIdentity(String fingerprint);
  private static native void nativeClient(String id);

  /**
   * The relay's base URL (http://127.0.0.1:port), opening it when it is not open; "" when bronze
   * cannot be reached. `identity` pins Diamond's identity (empty: plain, only for claiming a
   * setup, which is sealed). Blocks up to 5 s: call off the main thread.
   */
  static synchronized String base(String relayAddress, String relayFingerprint, String identity, String client) {
    if (!LOADED) return "";
    nativeIdentity(identity == null ? "" : identity);
    nativeClient(client == null ? "" : client);
    if (port > 0 && address.equals(relayAddress) && nativeAlive()) return "http://127.0.0.1:" + port;
    nativeStop();
    port = nativeStart(relayAddress, relayFingerprint);
    address = port > 0 ? relayAddress : "";
    return port > 0 ? "http://127.0.0.1:" + port : "";
  }

  /** The relay for a paired TV, from what pairing (or Diamond at home) taught it; "" when it has none. */
  static String base(Context context, String client) {
    Remote remote = Remote.load(context);
    if (remote.identity.isEmpty()) return "";
    return base(remote.relayAddress, remote.relayFingerprint, remote.identity, client);
  }

  static synchronized boolean isRelay(String base) { return port > 0 && base.equals("http://127.0.0.1:" + port); }

  static synchronized void stop() {
    if (!LOADED) return;
    nativeStop();
    nativeIdentity("");
    port = 0;
    address = "";
  }

  /**
   * What a TV needs to reach Diamond away from home: Diamond's identity, and bronze's address and
   * fingerprint. None of it is secret (end-to-end encryption, not bronze's certificate, protects
   * the traffic); it is learned only from a sealed pairing or from Diamond on the home network.
   * Kept with the connection, so disconnecting forgets it.
   */
  static final class Remote {
    final String identity, relayAddress, relayFingerprint;

    Remote(String identity, String relayAddress, String relayFingerprint) {
      this.identity = clean(identity);
      this.relayAddress = relayAddress == null || relayAddress.isEmpty() ? DEFAULT_ADDRESS : relayAddress;
      this.relayFingerprint = clean(relayFingerprint).isEmpty() ? DEFAULT_FINGERPRINT : clean(relayFingerprint);
    }

    private static String clean(String fingerprint) {
      String f = fingerprint == null ? "" : fingerprint.trim().toLowerCase(java.util.Locale.ROOT);
      return f.matches("[0-9a-f]{64}") ? f : "";
    }

    /** From a pairing's setup or /v1/ping's "server" ({identity, relay: {address, fingerprint}}). */
    static Remote from(String identity, JSONObject relay) {
      return new Remote(identity, relay == null ? "" : relay.optString("address"), relay == null ? "" : relay.optString("fingerprint"));
    }

    private static SharedPreferences prefs(Context c) { return c.getSharedPreferences("connection", Context.MODE_PRIVATE); }

    static Remote load(Context c) {
      SharedPreferences p = prefs(c);
      return new Remote(p.getString("remote_identity", ""), p.getString("remote_relay_address", ""), p.getString("remote_relay_fingerprint", ""));
    }

    /** Keeps it, unless it has no identity (nothing to pin). */
    void save(Context c) {
      if (identity.isEmpty()) return;
      prefs(c).edit().putString("remote_identity", identity).putString("remote_relay_address", relayAddress)
          .putString("remote_relay_fingerprint", relayFingerprint).apply();
    }
  }
}
