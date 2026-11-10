package com.torrbaby.tv;

import android.graphics.*;
import android.os.*;
import android.util.Base64;
import android.util.LruCache;
import android.widget.ImageView;
import java.lang.ref.WeakReference;
import java.util.*;
import java.util.concurrent.*;

/** Artwork only: bounded memory/disk cache, coalesced requests, no video cache. */
final class Art {
  private final Api api;
  private final BrowseCache disk;
  private final ThreadPoolExecutor pool = new ThreadPoolExecutor(3, 3, 0, TimeUnit.MILLISECONDS,
      new PriorityBlockingQueue<Runnable>());
  private final java.util.concurrent.atomic.AtomicLong sequence = new java.util.concurrent.atomic.AtomicLong();
  private final Map<String, Job> jobs = new HashMap<>();
  private final class Job implements Runnable, Comparable<Job> {
    final String key;
    final long order = sequence.getAndIncrement();
    final Runnable work;
    volatile int priority;
    Job(String key, int priority, Runnable work) { this.key=key; this.priority=priority; this.work=work; }
    public int compareTo(Job other) { int c=Integer.compare(priority,other.priority); return c!=0?c:Long.compare(order,other.order); }
    public void run() { work.run(); }
  }

  // Drop obsolete queued requests when switching screens; the three active reads can finish into cache.
  void beginScreen() {
    synchronized(waiting) {
      for(Runnable task:new ArrayList<>(pool.getQueue())) {
        Job job=(Job)task;
        if(pool.remove(job)) { waiting.remove(job.key); jobs.remove(job.key); }
      }
    }
  }

  void prioritize() {
    synchronized(waiting) {
      for(Job job:jobs.values()) {
        int priority=3;
        List<WeakReference<ImageView>> group=waiting.get(job.key);
        if(group!=null)for(WeakReference<ImageView> ref:group) {
          ImageView view=ref.get();
          if(view==null || !job.key.equals(view.getTag()))continue;
          android.graphics.Rect rect=new android.graphics.Rect();
          if(view.getGlobalVisibleRect(rect))priority=Math.min(priority,1);
          for(android.view.ViewParent p=view.getParent();p instanceof android.view.View;p=p.getParent())
            if(((android.view.View)p).isFocused()) { priority=0;break; }
        }
        if(priority!=job.priority && pool.remove(job)) { job.priority=priority;pool.execute(job); }
      }
    }
  }
  private final Handler main = new Handler(Looper.getMainLooper());
  private final Map<String, List<WeakReference<ImageView>>> waiting = new HashMap<>();
  private final LruCache<String, Bitmap> cache =
      new LruCache<String, Bitmap>(32 * 1024 * 1024) {
        protected int sizeOf(String k, Bitmap b) {
          return b.getByteCount();
        }
      };

  Art(android.content.Context context, Api a) {
    api = a;
    disk = new BrowseCache(context, a);
  }

  void load(ImageView view, String id, String variant, String revision) {
    String key = id + "/" + variant + "/" + revision;
    if (view != null) {
      if (key.equals(view.getTag()) && view.getDrawable() != null) return;
      view.setTag(key);
      view.setImageDrawable(null);
    }
    Bitmap cached = cache.get(key);
    if (cached != null) {
      if(view!=null)view.setImageBitmap(cached);
      return;
    }
    synchronized (waiting) {
      List<WeakReference<ImageView>> group = waiting.get(key);
      if (group != null) {
        if(view!=null)group.add(new WeakReference<>(view));
        return;
      }
      group = new ArrayList<>();
      if(view!=null)group.add(new WeakReference<>(view));
      waiting.put(key, group);
    }
    Job job = new Job(key, view==null?3:variant.equals("backdrop")?0:2,
        () -> {
          Bitmap result = null;
          try {
            byte[] raw = disk.read("art/" + key, 0);
            if (raw == null) {
              raw = Base64.decode(api.request("GET", "/v1/library/artwork/" + Api.part(id) + "/" + variant, null).getString("data"), Base64.DEFAULT);
              disk.write("art/" + key, raw);
            }
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
                  jobs.remove(key);
                }
                if (group != null && bitmap != null)
                  for (WeakReference<ImageView> ref : group) {
                    ImageView target = ref.get();
                    if (target != null && key.equals(target.getTag()))
                    {
                      target.setImageBitmap(bitmap);
                      StartupTrace.mark(variant.equals("backdrop") ? "First backdrop assigned" : "First thumbnail assigned");
                    }
                  }
              });
        });
    synchronized(waiting) { jobs.put(key,job); }
    pool.execute(job);
  }

  void prefetch(Library.Title title) { load(null,title.id,"poster",title.poster); }

  void close() {
    pool.shutdownNow();
    synchronized (waiting) {
      waiting.clear(); jobs.clear();
    }
    cache.evictAll();
  }
}
