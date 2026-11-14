package com.torrbaby.tv;

import android.content.Context;
import android.net.nsd.*;
import android.os.*;
import java.util.*;

/**
 * The same DNS-SD service used by tvOS; credentials never travel in TXT records. Two searches
 * run side by side: Android's (NsdManager) and the app's own question to the network (Mdns),
 * because Android's daemon stops on some TVs and then fails every search at once. The search
 * has failed only when both have.
 */
final class ServerDiscovery implements AutoCloseable {
  interface Listener {
    void changed(Map<String, String> servers);

    void failed();
  }

  private final NsdManager manager;
  private final Handler main = new Handler(Looper.getMainLooper());
  private final Listener listener;
  private final Map<String, String> servers = new LinkedHashMap<>();
  private final Set<String> present = new HashSet<>();
  private final ArrayDeque<NsdServiceInfo> pending = new ArrayDeque<>();
  private volatile boolean closed;
  private boolean resolving, systemFailed, ownFinished;
  private final NsdManager.DiscoveryListener browser =
      new NsdManager.DiscoveryListener() {
        public void onDiscoveryStarted(String type) {}

        public void onDiscoveryStopped(String type) {}

        public void onStartDiscoveryFailed(String type, int error) {
          main.post(() -> { systemFailed = true; failedIfBoth(); });
        }

        public void onStopDiscoveryFailed(String type, int error) {}

        public void onServiceFound(NsdServiceInfo service) {
          main.post(
              () -> {
                if (closed || !present.add(service.getServiceName())) return;
                pending.add(service);
                resolveNext();
              });
        }

        public void onServiceLost(NsdServiceInfo service) {
          main.post(
              () -> {
                if (closed) return;
                present.remove(service.getServiceName());
                servers.remove(service.getServiceName());
                publish();
              });
        }
      };

  ServerDiscovery(Context context, Listener listener) {
    manager = (NsdManager) context.getSystemService(Context.NSD_SERVICE);
    this.listener = listener;
  }

  void start() {
    Thread own = new Thread(() -> {
      Map<String, String> answered = Mdns.find(8000, () -> closed);
      main.post(() -> {
        if (closed) return;
        ownFinished = true;
        if (answered.isEmpty()) { failedIfBoth(); return; }
        // Android's own answer for the same service, when it has one, stays.
        boolean added = false;
        for (Map.Entry<String, String> server : answered.entrySet())
          if (servers.putIfAbsent(server.getKey(), server.getValue()) == null) added = true;
        if (added) publish();
      });
    }, "Server search");
    own.setDaemon(true);
    own.start();
    try {
      manager.discoverServices("_torrbaby._tcp.", NsdManager.PROTOCOL_DNS_SD, browser);
    } catch (RuntimeException e) {
      systemFailed = true;
    }
  }

  private void failedIfBoth() {
    if (!closed && systemFailed && ownFinished && servers.isEmpty()) listener.failed();
  }

  private void publish() {
    listener.changed(new LinkedHashMap<>(servers));
  }

  @SuppressWarnings("deprecation")
  private void resolveNext() {
    if (closed || resolving || pending.isEmpty()) return;
    NsdServiceInfo service = pending.remove();
    resolving = true;
    try {
      manager.resolveService(
          service,
          new NsdManager.ResolveListener() {
            public void onResolveFailed(NsdServiceInfo info, int error) {
              main.post(
                  () -> {
                    resolving = false;
                    resolveNext();
                  });
            }

            public void onServiceResolved(NsdServiceInfo info) {
              main.post(
                  () -> {
                    resolving = false;
                    if (closed) return;
                    if (present.contains(info.getServiceName())
                        && info.getHost() != null
                        && info.getPort() > 0) {
                      String host = info.getHost().getHostAddress();
                      if (host.contains(":")) host = "[" + host.replace("%", "%25") + "]";
                      servers.put(info.getServiceName(), "http://" + host + ":" + info.getPort());
                      publish();
                    }
                    resolveNext();
                  });
            }
          });
    } catch (RuntimeException e) {
      resolving = false;
      resolveNext();
    }
  }

  public void close() {
    if (closed) return;
    closed = true;
    main.removeCallbacksAndMessages(null);
    pending.clear();
    try {
      manager.stopServiceDiscovery(browser);
    } catch (RuntimeException ignored) {
    }
  }
}
