package com.torrbaby.tv;

import java.net.*;
import java.nio.charset.StandardCharsets;
import java.util.*;

/**
 * The server's Bonjour service asked for directly, without Android's mDNS daemon. On some TVs
 * (JMGO, Android 11) that daemon stops and NsdManager then fails every search at once, which
 * left the TV unable to find a server sitting on the same network. This sends the same
 * question itself from an ordinary socket; a responder answers such a question straight back to
 * the asking port, so no multicast lock or permission is needed. The address used is the one
 * the answer came from: the server itself, known to be reachable from here.
 */
final class Mdns {
  private static final String[] SERVICE = {"_torrbaby", "_tcp", "local"};
  private static final int PTR = 12, SRV = 33;
  private static final long[] ASK_AT = {0, 300, 1000, 2000, 4000};

  /** Service name → base URL of the first server to answer within `millis`; empty when none did. */
  static Map<String, String> find(long millis, java.util.function.BooleanSupplier stop) {
    Map<String, String> servers = new LinkedHashMap<>();
    try (MulticastSocket socket = new MulticastSocket()) {
      socket.setTimeToLive(255);
      InetAddress group = InetAddress.getByName("224.0.0.251");
      Map<String, List<String>> instances = new LinkedHashMap<>();
      long start = System.currentTimeMillis(), end = start + millis;
      int asked = 0;
      byte[] buffer = new byte[9000];
      while (servers.isEmpty() && !stop.getAsBoolean()) {
        long now = System.currentTimeMillis();
        if (now >= end) break;
        if (asked < ASK_AT.length && now - start >= ASK_AT[asked]) {
          byte[] question = question(instances.values());
          socket.send(new DatagramPacket(question, question.length, group, 5353));
          asked++;
        }
        long next = asked < ASK_AT.length ? Math.min(end, start + ASK_AT[asked]) : end;
        socket.setSoTimeout((int) Math.max(1, Math.min(250, next - now)));
        DatagramPacket packet = new DatagramPacket(buffer, buffer.length);
        try { socket.receive(packet); } catch (SocketTimeoutException quiet) { continue; }
        try { read(packet, instances, servers); } catch (RuntimeException malformed) { /* Not an answer for us. */ }
      }
    } catch (Exception ignored) {
      // No network, or sending refused: the caller's other search (or the relay) decides.
    }
    return servers;
  }

  /** Who offers the service, and where each instance already named is. */
  private static byte[] question(Collection<List<String>> instances) {
    java.io.ByteArrayOutputStream out = new java.io.ByteArrayOutputStream();
    int count = 1 + instances.size();
    out.write(0); out.write(0); out.write(0); out.write(0); out.write(count >> 8); out.write(count);
    for (int n = 0; n < 6; n++) out.write(0);
    name(out, Arrays.asList(SERVICE)); ask(out, PTR);
    for (List<String> instance : instances) { name(out, instance); ask(out, SRV); }
    return out.toByteArray();
  }

  private static void name(java.io.ByteArrayOutputStream out, List<String> labels) {
    for (String label : labels) {
      byte[] bytes = label.getBytes(StandardCharsets.UTF_8);
      out.write(Math.min(bytes.length, 63));
      out.write(bytes, 0, Math.min(bytes.length, 63));
    }
    out.write(0);
  }

  /** Class IN with the "answer me directly" bit. */
  private static void ask(java.io.ByteArrayOutputStream out, int type) {
    out.write(type >> 8); out.write(type); out.write(0x80); out.write(1);
  }

  private static void read(DatagramPacket packet, Map<String, List<String>> instances, Map<String, String> servers) {
    byte[] d = packet.getData();
    int length = packet.getLength();
    if (length < 12 || (d[2] & 0x80) == 0) return;
    int questions = u16(d, 4), records = u16(d, 6) + u16(d, 8) + u16(d, 10);
    int[] at = {12};
    for (int n = 0; n < questions; n++) { labels(d, length, at); at[0] += 4; }
    Map<String, Integer> ports = new HashMap<>();
    for (int n = 0; n < records && at[0] < length; n++) {
      List<String> owner = labels(d, length, at);
      if (at[0] + 10 > length) return;
      int type = u16(d, at[0]), size = u16(d, at[0] + 8), data = at[0] + 10;
      at[0] = data + size;
      if (at[0] > length) return;
      if (type == PTR && key(owner).equals(key(Arrays.asList(SERVICE)))) {
        List<String> instance = labels(d, length, new int[] {data});
        if (instance.size() == SERVICE.length + 1) instances.put(key(instance), instance);
      } else if (type == SRV && size >= 7) {
        ports.put(key(owner), u16(d, data + 4));
      }
    }
    if (!(packet.getAddress() instanceof Inet4Address)) return;
    for (Map.Entry<String, List<String>> instance : instances.entrySet()) {
      Integer port = ports.get(instance.getKey());
      if (port != null && port > 0)
        servers.put(instance.getValue().get(0), "http://" + packet.getAddress().getHostAddress() + ":" + port);
    }
  }

  private static int u16(byte[] d, int at) { return (d[at] & 0xff) << 8 | (d[at + 1] & 0xff); }

  private static String key(List<String> labels) { return String.join("\u0000", labels).toLowerCase(Locale.ROOT); }

  /** A DNS name at `at[0]` (compression pointers followed); `at[0]` ends after the name. */
  private static List<String> labels(byte[] d, int length, int[] at) {
    List<String> labels = new ArrayList<>();
    int p = at[0], jumps = 0;
    boolean jumped = false;
    while (true) {
      if (p >= length) throw new IllegalArgumentException();
      int n = d[p] & 0xff;
      if (n == 0) { if (!jumped) at[0] = p + 1; return labels; }
      if ((n & 0xc0) == 0xc0) {
        if (p + 1 >= length || ++jumps > 16) throw new IllegalArgumentException();
        if (!jumped) at[0] = p + 2;
        jumped = true;
        p = (n & 0x3f) << 8 | (d[p + 1] & 0xff);
      } else {
        if (n > 63 || p + 1 + n > length) throw new IllegalArgumentException();
        labels.add(new String(d, p + 1, n, StandardCharsets.UTF_8));
        p += 1 + n;
      }
    }
  }
}
