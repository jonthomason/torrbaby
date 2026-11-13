package com.torrbaby.tv;

/**
 * Where the server was found on the home network during this run of the app. It is never
 * saved: the address comes from DHCP and may change, so each launch finds the server again by
 * its Bonjour service. Until then requests go to a name that never resolves, fail at once, and
 * start that search.
 */
final class ServerRoute {
  static final String UNKNOWN = "http://torrbaby.invalid:8765";
  static volatile String found = "";

  static String current() { return found.isEmpty() ? UNKNOWN : found; }

  /** Finds the server by Bonjour, waiting up to `millis`; call off the main thread. */
  static String find(android.content.Context context, long millis) {
    java.util.concurrent.CountDownLatch done = new java.util.concurrent.CountDownLatch(1);
    String[] result = {""};
    ServerDiscovery[] search = new ServerDiscovery[1];
    new android.os.Handler(android.os.Looper.getMainLooper()).post(() -> {
      search[0] = new ServerDiscovery(context, new ServerDiscovery.Listener() {
        public void changed(java.util.Map<String, String> servers) {
          if (servers.isEmpty() || done.getCount() == 0) return;
          result[0] = servers.values().iterator().next();
          done.countDown();
        }
        public void failed() { done.countDown(); }
      });
      search[0].start();
    });
    try { done.await(millis, java.util.concurrent.TimeUnit.MILLISECONDS); } catch (InterruptedException ignored) { }
    new android.os.Handler(android.os.Looper.getMainLooper()).post(() -> { if (search[0] != null) search[0].close(); });
    if (!result[0].isEmpty()) found = result[0];
    return result[0];
  }
}
