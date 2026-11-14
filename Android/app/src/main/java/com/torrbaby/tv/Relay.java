package com.torrbaby.tv;

import android.content.Context;
import android.content.SharedPreferences;
import org.json.JSONObject;

/**
 * Reaching Diamond away from home: one loopback base URL (http://127.0.0.1:port) whose every
 * connection goes by a direct path punched through both NATs when one is open, else by
 * Torrbaby's relay on bronze (the Rust code the iPhone uses: Android/native over
 * Apple/Direct/src/proxy.rs). Rust keeps the paths: the relay open, bronze asked every 6 s for
 * this TV's public address, punching with backoff, dormant after 2 minutes idle. This class tells
 * it about network changes and logs its status line (logcat tag TorrbabyDirect). Once Diamond's
 * identity is pinned, both paths are encrypted end to end to Diamond, so bronze carries traffic
 * it cannot read.
 */
final class Relay {
  /** Built in for a TV that has never been paired; pairing replaces them (docs/remote-pairing.md). */
  static final String DEFAULT_ADDRESS = "47.112.18.69:7443",
      DEFAULT_FINGERPRINT = "c0fcde9603eec131f8d4a7ace67e7aad2900e5f9b6a59c6818106b0a51d6d7d6";

  private static final String TAG = "TorrbabyDirect";
  private static final boolean LOADED = load();
  private static int port;
  private static String stateDir = "";
  private static boolean watching;
  private static java.util.concurrent.ScheduledExecutorService reporter;

  private static boolean load() {
    try { System.loadLibrary("tbandroid"); return true; } catch (Throwable missing) { return false; }
  }

  private static native int nativeStart(String address, String fingerprint, String token, String stateDir);
  private static native boolean nativeAlive();
  private static native void nativeStop();
  private static native void nativeIdentity(String fingerprint);
  private static native void nativeClient(String id);
  private static native void nativeNetwork(String signature, String iface);
  private static native String nativeStatus();

  /**
   * Once, from the activity: tells Rust about the default network now and whenever it changes
   * (a change that holds for 3 s drops the direct path and moves the relay to the new network).
   */
  static synchronized void watchNetwork(Context context) {
    if (!LOADED || watching) return;
    watching = true;
    Context app = context.getApplicationContext();
    stateDir = app.getFilesDir().getAbsolutePath();
    android.net.ConnectivityManager connectivity = app.getSystemService(android.net.ConnectivityManager.class);
    if (connectivity == null) return;
    try {
      connectivity.registerDefaultNetworkCallback(new android.net.ConnectivityManager.NetworkCallback() {
        @Override public void onLinkPropertiesChanged(android.net.Network network, android.net.LinkProperties link) { tell(link); }
        @Override public void onLost(android.net.Network network) { nativeNetwork("none", ""); }
      });
    } catch (RuntimeException refused) {
      android.util.Log.w(TAG, "No network callback: " + refused);
    }
  }

  /** The network's signature: its interface, IPv4 addresses and default gateways. */
  private static void tell(android.net.LinkProperties link) {
    String name = link.getInterfaceName() == null ? "" : link.getInterfaceName();
    java.util.List<String> addresses = new java.util.ArrayList<>(), gateways = new java.util.ArrayList<>();
    for (android.net.LinkAddress address : link.getLinkAddresses())
      if (address.getAddress() instanceof java.net.Inet4Address) addresses.add(address.getAddress().getHostAddress());
    for (android.net.RouteInfo route : link.getRoutes())
      if (route.isDefaultRoute() && route.getGateway() instanceof java.net.Inet4Address) gateways.add(route.getGateway().getHostAddress());
    java.util.Collections.sort(addresses);
    java.util.Collections.sort(gateways);
    String signature = name + " " + String.join(",", addresses) + (gateways.isEmpty() ? "" : " via " + String.join(",", gateways));
    nativeNetwork(signature, name);
  }

  /** The proxy's status line in logcat every 30 s while it runs. */
  private static void report() {
    if (reporter != null) return;
    reporter = java.util.concurrent.Executors.newSingleThreadScheduledExecutor(r -> {
      Thread thread = new Thread(r, "Relay status");
      thread.setDaemon(true);
      return thread;
    });
    reporter.scheduleWithFixedDelay(() -> {
      try { android.util.Log.i(TAG, "status: " + nativeStatus()); } catch (Throwable ignored) {}
    }, 5, 30, java.util.concurrent.TimeUnit.SECONDS);
  }

  /**
   * The base URL away from home (http://127.0.0.1:port), starting the proxy when it is not
   * running; "" when bronze cannot be reached. The URL stays the same while the proxy runs,
   * whichever path carries it. `identity` pins Diamond's identity (empty: plain, only for
   * claiming a setup, which is sealed); `token` lets Rust ask Diamond for a direct path (empty:
   * relay only). Blocks up to 7 s when starting: call off the main thread.
   */
  static synchronized String base(String relayAddress, String relayFingerprint, String identity, String client, String token) {
    if (!LOADED) return "";
    nativeIdentity(identity == null ? "" : identity);
    nativeClient(client == null ? "" : client);
    // Already running for this relay: it takes the token and answers at once, same port.
    port = nativeStart(relayAddress, relayFingerprint, token == null ? "" : token, stateDir);
    if (port > 0) report();
    return port > 0 ? "http://127.0.0.1:" + port : "";
  }

  /** The way to Diamond for a paired TV, from what pairing (or Diamond at home) taught it; "" when it has none. */
  static String base(Context context, String client, String token) {
    Remote remote = Remote.load(context);
    if (remote.identity.isEmpty()) return "";
    return base(remote.relayAddress, remote.relayFingerprint, remote.identity, client, token);
  }

  static synchronized boolean isRelay(String base) { return port > 0 && base.equals("http://127.0.0.1:" + port); }

  /** At home (Diamond found on this network) or disconnected: nothing goes through bronze. */
  static synchronized void stop() {
    if (!LOADED) return;
    if (nativeAlive()) android.util.Log.i(TAG, "stopping: " + nativeStatus());
    nativeStop();
    nativeIdentity("");
    port = 0;
    if (reporter != null) { reporter.shutdownNow(); reporter = null; }
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
