package com.torrbaby.tv;

import android.graphics.*;
import android.os.*;
import android.util.Base64;
import android.util.LruCache;
import android.widget.ImageView;
import java.lang.ref.WeakReference;
import java.util.*;
import java.util.concurrent.*;

/** Artwork only: bounded memory, coalesced requests, no disk/video cache. */
final class Art {
  private final Api api;
  private final ExecutorService pool = Executors.newFixedThreadPool(3);
  private final Handler main = new Handler(Looper.getMainLooper());
  private final Map<String, List<WeakReference<ImageView>>> waiting = new HashMap<>();
  private final LruCache<String, Bitmap> cache =
      new LruCache<String, Bitmap>(32 * 1024 * 1024) {
        protected int sizeOf(String k, Bitmap b) {
          return b.getByteCount();
        }
      };

  Art(Api a) {
    api = a;
  }

  void load(ImageView view, String id, String variant) {
    String key = id + "/" + variant;
    view.setTag(key);
    view.setImageDrawable(null);
    Bitmap cached = cache.get(key);
    if (cached != null) {
      view.setImageBitmap(cached);
      return;
    }
    synchronized (waiting) {
      List<WeakReference<ImageView>> group = waiting.get(key);
      if (group != null) {
        group.add(new WeakReference<>(view));
        return;
      }
      group = new ArrayList<>();
      group.add(new WeakReference<>(view));
      waiting.put(key, group);
    }
    pool.execute(
        () -> {
          Bitmap result = null;
          try {
            byte[] raw =
                Base64.decode(
                    api.request("GET", "/v1/library/artwork/" + Api.part(id) + "/" + variant, null)
                        .getString("data"),
                    Base64.DEFAULT);
            BitmapFactory.Options o = new BitmapFactory.Options();
            o.inJustDecodeBounds = true;
            BitmapFactory.decodeByteArray(raw, 0, raw.length, o);
            int max = variant.equals("poster") ? 300 : variant.equals("episode") ? 400 : 1280;
            o.inSampleSize = 1;
            while (o.outWidth / o.inSampleSize > max) o.inSampleSize *= 2;
            o.inJustDecodeBounds = false;
            result = BitmapFactory.decodeByteArray(raw, 0, raw.length, o);
            if (result != null) cache.put(key, result);
          } catch (Exception ignored) {
          }
          Bitmap bitmap = result;
          main.post(
              () -> {
                List<WeakReference<ImageView>> group;
                synchronized (waiting) {
                  group = waiting.remove(key);
                }
                if (group != null && bitmap != null)
                  for (WeakReference<ImageView> ref : group) {
                    ImageView target = ref.get();
                    if (target != null && key.equals(target.getTag()))
                      target.setImageBitmap(bitmap);
                  }
              });
        });
  }

  void close() {
    pool.shutdownNow();
    synchronized (waiting) {
      waiting.clear();
    }
    cache.evictAll();
  }
}
