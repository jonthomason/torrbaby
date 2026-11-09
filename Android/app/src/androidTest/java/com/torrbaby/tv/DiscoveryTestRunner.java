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
  private boolean catalogCheck;
  public void onCreate(Bundle args) {
    super.onCreate(args); catalogCheck = args != null && args.getBoolean("catalog", false);
    if (args != null && "true".equals(args.getString("catalog"))) catalogCheck = true;
    start();
  }
  public void onStart() {
    if (catalogCheck) {
      Bundle result = new Bundle();
      try {
        String raw = "{\"titles\":[{\"id\":\"test\",\"assets\":[{\"id\":\"episode\",\"season\":1,\"position\":12.5,\"complete\":true,\"media_info\":{\"state\":\"ready\",\"probe\":{\"ignored\":[1,null,{}]},\"audio_tracks\":[{\"language\":\"eng\",\"codec\":\"aac\"}],\"subtitle_tracks\":[],\"prepared\":[{\"language\":\"zh-Hans\",\"state\":\"ready\",\"generated\":true}]}}]}]}";
        org.json.JSONObject compact = CatalogJson.parse(raw);
        Library library = Library.parse(compact);
        Library.Asset asset = library.titles.get(0).assets.get(0);
        if (!asset.complete || asset.position != 12.5 || asset.season != 1
            || asset.mediaInfo.has("probe")
            || asset.mediaInfo.getJSONArray("audio_tracks").getJSONObject(0).has("codec")
            || !MediaLanguages.lines(asset.mediaInfo).get(1).contains("Chinese (Simplified)"))
          throw new Exception("Catalog projection lost browsing information");
        Library.parse(CatalogJson.parse(compact.toString()));
        result.putString("stream", "PASS: compact catalog preserves browsing, progress and generated languages\n");
        finish(-1, result);
      } catch (Exception error) { result.putString("stream", "FAIL: " + error); finish(0, result); }
      return;
    }
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
