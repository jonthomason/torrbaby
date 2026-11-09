package com.torrbaby.tv;

import android.app.Instrumentation;
import android.net.nsd.*;
import android.os.*;
import java.util.Map;

/** Device smoke test against Android's real DNS-SD stack, without a production server. */
public final class DiscoveryTestRunner extends Instrumentation {
  private NsdManager manager;
  private ServerDiscovery discovery;
  private boolean done, registered;
  private final String serviceName = "Torrbaby discovery test " + System.currentTimeMillis();
  private final Handler main = new Handler(Looper.getMainLooper());
  private final NsdManager.RegistrationListener registration = new NsdManager.RegistrationListener() {
    public void onServiceRegistered(NsdServiceInfo info) { registered = true; }
    public void onRegistrationFailed(NsdServiceInfo info, int error) { main.post(() -> complete("Registration failed: " + error)); }
    public void onServiceUnregistered(NsdServiceInfo info) {}
    public void onUnregistrationFailed(NsdServiceInfo info, int error) {}
  };
  public void onCreate(Bundle args) { super.onCreate(args); start(); }
  public void onStart() {
    main.post(() -> {
      manager = (NsdManager)getTargetContext().getSystemService(android.content.Context.NSD_SERVICE);
      discovery = new ServerDiscovery(getTargetContext(), new ServerDiscovery.Listener() {
        public void failed() { complete("Discovery failed"); }
        public void changed(Map<String, String> servers) {
          for (Map.Entry<String, String> entry : servers.entrySet())
            if (entry.getKey().startsWith(serviceName)) {
              try {
                String address = Api.validateBase(entry.getValue());
                if (!address.endsWith(":18765")) throw new Exception("Wrong port");
                complete(null);
              } catch (Exception error) { complete(error.toString()); }
            }
        }
      });
      discovery.start();
      NsdServiceInfo info = new NsdServiceInfo();
      info.setServiceName(serviceName); info.setServiceType("_torrbaby._tcp."); info.setPort(18765);
      manager.registerService(info, NsdManager.PROTOCOL_DNS_SD, registration);
      main.postDelayed(() -> complete("Timed out discovering registered service"), 20000);
    });
  }
  private void complete(String error) {
    if (done) return;
    done = true; main.removeCallbacksAndMessages(null);
    if (discovery != null) discovery.close();
    if (registered) manager.unregisterService(registration);
    Bundle result = new Bundle();
    result.putString("stream", error == null ? "PASS: native DNS-SD discovery resolves Torrbaby service and port\n" : "FAIL: " + error + "\n");
    finish(error == null ? -1 : 0, result);
  }
}
