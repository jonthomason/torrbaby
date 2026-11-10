package com.torrbaby.tv;

import java.io.*;
import java.net.*;
import java.nio.charset.StandardCharsets;
import org.json.*;

final class Api {
  final String base, token, client, profile, profileName;
  JSONObject profileAppearance = new JSONObject();
  volatile String libraryRaw = "", libraryRevision = "";
  private boolean supportsRevision = true;

  boolean libraryChanged() throws Exception {
    if (!supportsRevision || libraryRevision.isEmpty()) return true;
    try { return !libraryRevision.equals(request("GET", "/v1/library/revision", null).optString("revision")); }
    catch (HttpException e) { if(e.status==404) {supportsRevision=false;return true;} throw e; }
  }

  Api(String base, String token, String client) throws Exception {
    this(base, token, client, "", "");
  }

  Api(String base, String token, String client, String profile, String profileName) throws Exception {
    this.profile = profile;
    this.profileName = profileName;
    this.base = validateBase(base);
    this.token = token.trim();
    this.client = client;
  }

  static String validateBase(String input) throws Exception {
    String value = input.trim().replaceAll("/+$", "");
    URI u = new URI(value);
    if (!("http".equals(u.getScheme()) || "https".equals(u.getScheme()))
        || u.getHost() == null
        || u.getUserInfo() != null
        || u.getQuery() != null
        || u.getFragment() != null
        || (!u.getPath().isEmpty() && !u.getPath().equals("/")))
      throw new Exception("Enter a server address such as http://192.168.1.10:8765");
    return value;
  }

  String url(String path) throws Exception {
    URI relative = new URI(path), b = new URI(base);
    URI u = b.resolve(relative).normalize();
    if (!sameOrigin(b, u) || !u.getPath().startsWith("/v1/"))
      throw new Exception("The server returned an invalid media address.");
    return u.toString();
  }

  static boolean sameOrigin(URI a, URI b) {
    return a.getScheme().equalsIgnoreCase(b.getScheme())
        && a.getHost() != null
        && a.getHost().equalsIgnoreCase(b.getHost())
        && port(a) == port(b)
        && b.getUserInfo() == null;
  }

  static int port(URI u) {
    return u.getPort() < 0 ? ("https".equals(u.getScheme()) ? 443 : 80) : u.getPort();
  }

  static String part(String value) {
    try {
      return URLEncoder.encode(value, "UTF-8").replace("+", "%20").replace("%3A", ":");
    } catch (Exception e) {
      throw new IllegalArgumentException(e);
    }
  }

  JSONObject request(String method, String path, JSONObject body) throws Exception {
    Object value = requestValue(method, path, body);
    if (!(value instanceof JSONObject)) throw new IOException("Expected a server object.");
    return (JSONObject) value;
  }

  JSONArray array(String path) throws Exception {
    Object value = requestValue("GET", path, null);
    if (!(value instanceof JSONArray)) throw new IOException("Expected a server list.");
    return (JSONArray) value;
  }

  private Object requestValue(String method, String path, JSONObject body) throws Exception {
    if (path.equals("/v1/library")) StartupTrace.mark("Library HTTP started");
    HttpURLConnection c = (HttpURLConnection) new URL(url(path)).openConnection();
    c.setConnectTimeout(8000);
    c.setReadTimeout(path.endsWith("/remove") && method.equals("POST") ? 90000 : path.equals("/v1/search/torrents/detail") ? 45000 : path.startsWith("/v1/search/") ? 30000 : path.equals("/v1/torrents") && method.equals("POST") ? 45000 : 15000);
    c.setRequestMethod(method);
    c.setInstanceFollowRedirects(false);
    c.setRequestProperty("Accept", "application/json");
    if (!profile.isEmpty()) c.setRequestProperty("X-Torrbaby-Profile", profile);
    if (!token.isEmpty()) c.setRequestProperty("Authorization", "Bearer " + token);
    try {
      if (body != null) {
        c.setDoOutput(true);
        c.setRequestProperty("Content-Type", "application/json");
        try (OutputStream o = c.getOutputStream()) {
          o.write(body.toString().getBytes(StandardCharsets.UTF_8));
        }
      }
      int code = c.getResponseCode();
      if (path.equals("/v1/library")) StartupTrace.mark("Library HTTP headers received");
      if (code == 401 || code == 403)
        throw new AuthException("The connection has expired or was revoked. Link this TV again.");
      InputStream in = code < 400 ? c.getInputStream() : c.getErrorStream();
      String raw = in == null ? "{}" : read(in, 16 * 1024 * 1024);
      if (path.equals("/v1/library")) StartupTrace.mark("Library HTTP body received");
      Object json;
      try {
        json = path.equals("/v1/library") ? CatalogJson.parse(raw) : new JSONTokener(raw).nextValue();
        if (path.equals("/v1/library")) StartupTrace.mark("Library JSON decoded");
      } catch (JSONException e) {
        throw new IOException("The server returned an unexpected response (HTTP " + code + ").");
      }
      if (code < 200 || code >= 300)
        throw new HttpException(
            code,
            Library.string(json instanceof JSONObject ? (JSONObject) json : null, "error").isEmpty()
                ? "Server request failed (HTTP " + code + ")."
                : Library.string(json instanceof JSONObject ? (JSONObject) json : null, "error"));
      if (path.equals("/v1/library")) { libraryRaw = json.toString(); libraryRevision = ((JSONObject)json).optString("revision"); }
      return json;
    } finally {
      c.disconnect();
    }
  }

  static String read(InputStream input, int limit) throws IOException {
    try (InputStream in = input;
        ByteArrayOutputStream out = new ByteArrayOutputStream()) {
      byte[] b = new byte[16384];
      int n;
      while ((n = in.read(b)) != -1) {
        if (out.size() + n > limit) throw new IOException("Server response is too large.");
        out.write(b, 0, n);
      }
      return out.toString("UTF-8");
    }
  }

  void register() throws Exception {
    request(
        "POST",
        "/v1/clients",
        new JSONObject()
            .put("client_id", client)
            .put("name", android.os.Build.MODEL)
            .put("platform", "other"));
  }

  static final class HttpException extends IOException {
    final int status;

    HttpException(int status, String message) {
      super(message);
      this.status = status;
    }
  }

  static final class AuthException extends IOException {
    AuthException(String message) {
      super(message);
    }
  }
}
