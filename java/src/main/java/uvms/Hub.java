package uvms;

import static uvms.Media.*;

import java.io.*;
import java.nio.file.*;
import java.util.*;
import java.util.concurrent.*;
import java.util.concurrent.atomic.*;

final class Hub implements AutoCloseable {
  final Gateway.Config config;
  final ConcurrentHashMap<String, CompletableFuture<Feed>> feeds = new ConcurrentHashMap<>();
  final ConcurrentHashMap<String, String> recording = new ConcurrentHashMap<>();
  final AtomicLong used = new AtomicLong();
  final List<Thread> recorders = new CopyOnWriteArrayList<>();
  volatile boolean closed;

  Hub(Gateway.Config config) throws Exception {
    this.config = config;
    Path root = Path.of(config.recording_dir);
    Files.createDirectories(root);
    try (var files = Files.walk(root)) {
      for (Path p : files.filter(Files::isRegularFile).toList()) used.addAndGet(Files.size(p));
    }
  }

  Feed acquire(String id) throws Exception {
    if (!config.streams.containsKey(id) || closed) throw new IOException("unknown stream");
    var fresh = new CompletableFuture<Feed>();
    var current = feeds.putIfAbsent(id, fresh);
    if (current == null) {
      current = fresh;
      Thread.startVirtualThread(
          () -> {
            try {
              var feed = new Feed(id, new Rtsp(System.getenv(config.streams.get(id).url_env)));
              if (closed) {
                feed.close();
                throw new IOException("shutdown");
              }
              fresh.complete(feed);
              feed.run();
            } catch (Exception e) {
              Exception cause = e;
              while (cause.getCause() instanceof Exception nested) cause = nested;
              System.err.println("Input " + id + " unavailable: " + Gateway.failure(cause));
              fresh.completeExceptionally(new IOException("camera unavailable"));
            } finally {
              feeds.remove(id, fresh);
            }
          });
    }
    Feed result = current.get(12, TimeUnit.SECONDS);
    if (result.closed) throw new IOException("camera reconnecting");
    return result;
  }

  boolean online(String id) {
    var f = feeds.get(id);
    return f != null && f.isDone() && !f.isCompletedExceptionally() && !f.getNow(null).closed;
  }

  final class Feed implements AutoCloseable {
    final String id;
    final Rtsp input;
    final Video video;
    final List<Subscription> subscribers = new CopyOnWriteArrayList<>();
    volatile boolean closed;
    long unused = System.nanoTime();

    Feed(String id, Rtsp input) {
      this.id = id;
      this.input = input;
      video = input.video;
    }

    Subscription subscribe() {
      return subscribe(false);
    }

    Subscription subscribe(boolean live) {
      if (closed) throw new IllegalStateException("source closed");
      var s = new Subscription(this, live);
      subscribers.add(s);
      return s;
    }

    void run() {
      try {
        while (!closed && !Hub.this.closed) {
          Frame f = input.next();
          if (subscribers.isEmpty()) {
            if (System.nanoTime() - unused > 30_000_000_000L) break;
          } else unused = System.nanoTime();
          for (var s : subscribers) if (!s.queue.offer(f)) s.close();
        }
      } catch (Exception e) {
        if (!closed && !Hub.this.closed)
          System.err.println("Input " + id + " ended: " + Gateway.failure(e));
      } finally {
        close();
      }
    }

    public void close() {
      closed = true;
      input.close();
      for (var s : subscribers) s.close();
    }
  }

  static final class Subscription implements AutoCloseable {
    final Feed feed;
    final FrameQueue queue;
    volatile boolean closed;

    Subscription(Feed f, boolean live) {
      feed = f;
      queue = new FrameQueue(live);
    }

    Frame next() throws Exception {
      Frame f = queue.poll(1, TimeUnit.SECONDS);
      if (closed || feed.closed) throw new IOException("source ended or slow consumer");
      return f;
    }

    public void close() {
      closed = true;
      feed.subscribers.remove(this);
      queue.clear();
    }
  }

  // Live consumers recover at an IDR after overload. Recording keeps strict
  // delivery: overflowing its queue remains an error, not silent frame loss.
  static final class FrameQueue extends ArrayBlockingQueue<Frame> {
    final boolean live;
    boolean waiting;

    FrameQueue(boolean live) {
      super(32);
      this.live = live;
      waiting = live;
    }

    @Override
    public synchronized boolean offer(Frame frame) {
      if (!live) return super.offer(frame);
      if (waiting && !frame.key()) return true;
      if (super.offer(frame)) {
        waiting = false;
        return true;
      }
      clear();
      waiting = true;
      if (frame.key()) {
        super.offer(frame);
        waiting = false;
      }
      return true;
    }
  }

  synchronized void reserve(long bytes) throws IOException {
    if (bytes < 0 || used.get() > config.recording_limit_bytes - bytes)
      throw new IOException("storage_full");
    used.addAndGet(bytes);
  }

  void startRecorders() {
    config.streams.forEach(
        (id, s) -> {
          recording.put(id, s.record ? "connecting" : "off");
          if (s.record) recorders.add(Thread.startVirtualThread(() -> record(id)));
        });
  }

  void record(String id) {
    boolean full = false;
    while (!closed && !full) {
      Path partial = null;
      OutputStream out = null;
      try {
        Feed feed = acquire(id);
        try (var sub = feed.subscribe()) {
          Mp4 mux = null;
          long start = 0;
          while (!closed) {
            Frame f = sub.next();
            if (f == null) continue;
            if (out != null && f.key() && f.timestamp() - start >= config.segment_seconds * 90000) {
              out.close();
              out = null;
              publish(partial);
              partial = null;
            }
            if (out == null) {
              if (!f.key()) continue;
              mux = new Mp4(feed.video);
              byte[] first = mux.fragment(f, 0);
              reserve(mux.init.length + first.length);
              Path dir = Path.of(config.recording_dir, id);
              Files.createDirectories(dir);
              partial =
                  dir.resolve(
                      System.currentTimeMillis()
                          + "-"
                          + UUID.randomUUID().toString().replace("-", "")
                          + ".partial");
              out =
                  new BufferedOutputStream(
                      Files.newOutputStream(partial, StandardOpenOption.CREATE_NEW));
              out.write(mux.init);
              out.write(first);
              start = f.timestamp();
              recording.put(id, "recording");
            } else {
              byte[] fragment = mux.fragment(f, f.timestamp() - start);
              reserve(fragment.length);
              out.write(fragment);
            }
          }
        }
      } catch (Exception e) {
        full = "storage_full".equals(e.getMessage());
        recording.put(id, full ? "storage_full" : "retrying");
      } finally {
        try {
          if (out != null) out.close();
          if (partial != null) publish(partial);
        } catch (IOException ignored) {
        }
      }
      if (!closed && !full)
        try {
          Thread.sleep(2000);
        } catch (InterruptedException e) {
          break;
        }
    }
    if (closed) recording.put(id, "stopped");
  }

  static void publish(Path p) throws IOException {
    Files.move(
        p,
        p.resolveSibling(p.getFileName().toString().replace(".partial", ".mp4")),
        StandardCopyOption.ATOMIC_MOVE);
  }

  List<Map<String, Object>> files() throws IOException {
    var list = new ArrayList<Map<String, Object>>();
    for (String id : config.streams.keySet()) {
      Path dir = Path.of(config.recording_dir, id);
      if (!Files.isDirectory(dir)) continue;
      try (var paths = Files.list(dir)) {
        for (Path p :
            paths
                .filter(
                    f ->
                        Files.isRegularFile(f, LinkOption.NOFOLLOW_LINKS)
                            && safeName(f.getFileName().toString()))
                .toList())
          list.add(
              Map.of(
                  "stream",
                  id,
                  "file",
                  p.getFileName().toString(),
                  "bytes",
                  Files.size(p),
                  "url",
                  "/api/recordings/" + id + "/" + p.getFileName()));
      }
    }
    list.sort(Comparator.comparing(m -> (String) m.get("file"), Comparator.reverseOrder()));
    return list;
  }

  static boolean safeName(String name) {
    return name.matches("[a-fA-F0-9-]{1,85}\\.mp4");
  }

  public void close() {
    closed = true;
    feeds
        .values()
        .forEach(
            f -> {
              if (f.isDone() && !f.isCompletedExceptionally()) f.getNow(null).close();
            });
    for (Thread t : recorders)
      try {
        t.join(10000);
      } catch (InterruptedException e) {
        Thread.currentThread().interrupt();
      }
  }
}
