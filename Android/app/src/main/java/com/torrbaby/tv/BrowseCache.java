package com.torrbaby.tv;

import android.content.Context;
import android.util.AtomicFile;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.security.MessageDigest;
import java.util.Arrays;
import java.util.Comparator;

/** Disposable, bounded browsing cache; never contains credentials or video. */
final class BrowseCache {
  private final File root;
  private final String scope;
  BrowseCache(Context context, Api api) {
    root = new File(context.getCacheDir(), "browse-v1");
    root.mkdirs();
    scope = hash(api.base + "\n" + api.token);
  }
  private static String hash(String value) {
    try {
      byte[] digest = MessageDigest.getInstance("SHA-256").digest(value.getBytes(StandardCharsets.UTF_8));
      StringBuilder result = new StringBuilder();
      for (byte b : digest) result.append(String.format("%02x", b & 255));
      return result.toString();
    } catch (Exception e) { throw new IllegalStateException(e); }
  }
  byte[] read(String key, long maxAge) {
    synchronized (BrowseCache.class) {
      try {
        File file = new File(root, scope + "-" + hash(key));
        if (!file.isFile() || file.length() > 16 * 1024 * 1024
            || (maxAge > 0 && System.currentTimeMillis() - file.lastModified() > maxAge)) return null;
        return new AtomicFile(file).readFully();
      } catch (Exception ignored) { return null; }
    }
  }
  void write(String key, byte[] bytes) {
    if (bytes.length > 16 * 1024 * 1024) return;
    synchronized (BrowseCache.class) {
      AtomicFile file = new AtomicFile(new File(root, scope + "-" + hash(key)));
      FileOutputStream out = null;
      try {
        out = file.startWrite(); out.write(bytes); file.finishWrite(out);
        File[] files = root.listFiles();
        if (files == null) return;
        Arrays.sort(files, Comparator.comparingLong(File::lastModified));
        long total = 0;
        for (File f : files) total += f.length();
        for (File f : files) {
          if (total <= 80L * 1024 * 1024) break;
          long size = f.length(); if (f.delete()) total -= size;
        }
      } catch (Exception ignored) { if (out != null) file.failWrite(out); }
    }
  }
}
