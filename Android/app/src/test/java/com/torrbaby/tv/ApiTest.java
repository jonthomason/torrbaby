package com.torrbaby.tv;

import static org.junit.Assert.*;

import org.junit.Test;

public class ApiTest {
  @Test
  public void requestsKeepTheirOriginalProfileAfterSwitching() throws Exception {
    java.util.List<String> received = new java.util.ArrayList<>();
    java.net.ServerSocket server = new java.net.ServerSocket(0, 3, java.net.InetAddress.getByName("127.0.0.1"));
    server.setSoTimeout(5000);
    java.util.concurrent.ExecutorService worker = java.util.concurrent.Executors.newSingleThreadExecutor();
    java.util.concurrent.Future<?> serving = worker.submit(() -> {
      try {
        for (int n = 0; n < 3; n++) {
          try (java.net.Socket socket = server.accept()) {
            socket.setSoTimeout(5000);
            java.io.BufferedReader input = new java.io.BufferedReader(new java.io.InputStreamReader(socket.getInputStream()));
            String profile = null, line;
            while ((line = input.readLine()) != null && !line.isEmpty()) {
              if (line.toLowerCase(java.util.Locale.ROOT).startsWith("x-torrbaby-profile:"))
                profile = line.substring(line.indexOf(':') + 1).trim();
            }
            received.add(profile);
            socket.getOutputStream().write("HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}".getBytes(java.nio.charset.StandardCharsets.UTF_8));
          }
        }
      } catch (Exception e) { throw new RuntimeException(e); }
    });
    try {
      String base = "http://127.0.0.1:" + server.getLocalPort();
      Api original = new Api(base, "token", "device", "profile-jon", "Jon");
      Api switched = new Api(base, "token", "device", "profile-alex", "Alex");
      switched.request("POST", "/v1/playback/progress", null);
      original.request("POST", "/v1/playback/progress", null);
      new Api(base, "token", "device").request("POST", "/v1/playback/progress", null);
      serving.get(5, java.util.concurrent.TimeUnit.SECONDS);
      assertEquals(java.util.Arrays.asList("profile-alex", "profile-jon", null), received);
    } finally { server.close(); worker.shutdownNow(); }
  }

  @Test
  public void rejectsCredentialBearingAndNonServerAddresses() throws Exception {
    for (String s :
        new String[] {
          "file:///etc/passwd",
          "http://u:secret@host",
          "http://host/path",
          "http://host?token=secret",
          "javascript:alert(1)",
          "http://host#fragment"
        }) {
      try {
        Api.validateBase(s);
        fail(s);
      } catch (Exception expected) {
      }
    }
    assertEquals("http://host:8765", Api.validateBase(" http://host:8765/ "));
  }

  @Test
  public void signedMediaCannotEscapeServer() throws Exception {
    Api a = new Api("http://host:8765", "token", "client");
    assertEquals("http://host:8765/v1/stream/a/0?sig=abc", a.url("/v1/stream/a/0?sig=abc"));
    for (String path :
        new String[] {
          "http://evil/v1/file",
          "https://host:8765/v1/file",
          "//evil/v1/file",
          "/elsewhere",
          "/v1/../../secret",
          "http://user@host:8765/v1/file"
        }) {
      try {
        a.url(path);
        fail(path);
      } catch (Exception expected) {
      }
    }
  }

  @Test
  public void titleIdsKeepLiteralColonsExpectedByServer() {
    assertEquals("tmdb:movie:348", Api.part("tmdb:movie:348"));
    assertEquals("a%2Fb%20c", Api.part("a/b c"));
  }
}
