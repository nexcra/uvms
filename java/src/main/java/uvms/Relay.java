package uvms;

import static uvms.Media.*;

import java.io.*;
import java.net.*;
import java.nio.charset.StandardCharsets;
import java.util.*;
import java.util.concurrent.*;

final class Relay implements AutoCloseable {
  final Gateway app;
  final ServerSocket server;
  final Set<Socket> clients = ConcurrentHashMap.newKeySet();
  volatile boolean closed;

  Relay(Gateway app) throws Exception {
    this.app = app;
    server = new ServerSocket();
    server.bind(new InetSocketAddress(app.config.bind, app.config.rtsp_port));
  }

  void start() {
    Thread.startVirtualThread(
        () -> {
          while (!closed)
            try {
              Socket s = server.accept();
              s.setTcpNoDelay(true);
              if (!app.slots.tryAcquire()) {
                s.close();
                continue;
              }
              clients.add(s);
              Thread.startVirtualThread(
                  () -> {
                    try {
                      serve(s);
                    } catch (Exception ignored) {
                    } finally {
                      clients.remove(s);
                      app.slots.release();
                      try {
                        s.close();
                      } catch (Exception ignored) {
                      }
                    }
                  });
            } catch (IOException e) {
              if (!closed) System.err.println("RTSP listener failed");
            }
        });
  }

  record Request(String method, String uri, String cseq, Map<String, String> headers) {}

  static Request read(InputStream in) throws Exception {
    int first;
    do {
      first = in.read();
      if (first < 0) throw new EOFException();
      if (first == 36) {
        int ch = in.read(), a = in.read(), b = in.read();
        if (b < 0) throw new EOFException();
        int n = (a << 8) | b;
        if (n > 8192) throw new IOException("RTCP limit");
        if (in.readNBytes(n).length != n) throw new EOFException();
      }
    } while (first == 36);
    ByteArrayOutputStream h = new ByteArrayOutputStream();
    h.write(first);
    int state = 0;
    while (state != 4) {
      int v = in.read();
      if (v < 0) throw new EOFException();
      h.write(v);
      if (h.size() > 16384) throw new IOException("header limit");
      state =
          switch (state) {
            case 0 -> v == 13 ? 1 : 0;
            case 1 -> v == 10 ? 2 : 0;
            case 2 -> v == 13 ? 3 : 0;
            default -> v == 10 ? 4 : 0;
          };
    }
    String[] lines = h.toString(StandardCharsets.US_ASCII).split("\r\n"),
        firstLine = lines[0].split(" ");
    var headers = new HashMap<String, String>();
    for (int i = 1; i < lines.length; i++) {
      int c = lines[i].indexOf(':');
      if (c > 0)
        headers.put(
            lines[i].substring(0, c).toLowerCase(Locale.ROOT), lines[i].substring(c + 1).trim());
    }
    int size = Integer.parseInt(headers.getOrDefault("content-length", "0"));
    if (size < 0 || size > 4096) throw new IOException("body limit");
    if (in.readNBytes(size).length != size) throw new EOFException();
    return new Request(firstLine[0], firstLine[1], headers.getOrDefault("cseq", "0"), headers);
  }

  static void response(
      OutputStream out,
      Request r,
      int code,
      String reason,
      Map<String, String> headers,
      byte[] body)
      throws IOException {
    StringBuilder text =
        new StringBuilder(
            "RTSP/1.0 "
                + code
                + " "
                + reason
                + "\r\nCSeq: "
                + r.cseq
                + "\r\nContent-Length: "
                + body.length
                + "\r\n");
    headers.forEach((k, v) -> text.append(k).append(": ").append(v).append("\r\n"));
    out.write(ascii(text.append("\r\n").toString()));
    out.write(body);
    out.flush();
  }

  static void interleaved(OutputStream out, int channel, byte[] p) throws IOException {
    out.write(36);
    out.write(channel);
    out.write(p.length >> 8);
    out.write(p.length);
    out.write(p);
  }

  void serve(Socket socket) throws Exception {
    var requests = new ArrayBlockingQueue<Request>(8);
    socket.setSoTimeout(60000);
    Thread reader =
        Thread.startVirtualThread(
            () -> {
              try {
                var input = new BufferedInputStream(socketInput(socket));
                while (!closed) requests.put(read(input));
              } catch (Exception ignored) {
                try {
                  socket.close();
                } catch (Exception e) {
                }
              }
            });
    var output = socket.getOutputStream();
    var writingSince = new java.util.concurrent.atomic.AtomicLong();
    Thread watchdog =
        Thread.startVirtualThread(
            () -> {
              try {
                while (!socket.isClosed()) {
                  Thread.sleep(1000);
                  long started = writingSince.get();
                  if (started != 0 && System.nanoTime() - started > 5_000_000_000L) socket.close();
                }
              } catch (Exception ignored) {
              }
            });
    var out =
        new BufferedOutputStream(
            new OutputStream() {
              public void write(int value) throws IOException {
                write(new byte[] {(byte) value}, 0, 1);
              }

              public void write(byte[] bytes, int offset, int length) throws IOException {
                writingSince.set(System.nanoTime());
                try {
                  output.write(bytes, offset, length);
                } finally {
                  writingSince.set(0);
                }
              }
            },
            131072);
    String sid = UUID.randomUUID().toString(), selected = null;
    Hub.Feed feed = null;
    Hub.Subscription sub = null;
    int channel = 0, rtcp = 1;
    boolean setup = false, playing = false, key = false;
    Rtp rtp = new Rtp();
    long report = 0;
    try {
      while (!closed && !socket.isClosed()) {
        Request req = requests.poll(playing ? 1 : 500, TimeUnit.MILLISECONDS);
        if (req != null) {
          if (req.method.equals("OPTIONS")) {
            response(
                out,
                req,
                200,
                "OK",
                Map.of("Public", "OPTIONS, DESCRIBE, SETUP, PLAY, GET_PARAMETER, TEARDOWN"),
                new byte[0]);
            continue;
          }
          URI uri = URI.create(req.uri);
          String[] paths = uri.getPath().replaceFirst("^/", "").split("/");
          String id = paths[0];
          if (!app.config.streams.containsKey(id)) {
            response(out, req, 404, "Not Found", Map.of(), new byte[0]);
            continue;
          }
          if (selected != null && !selected.equals(id)) {
            response(out, req, 455, "Method Not Valid In This State", Map.of(), new byte[0]);
            continue;
          }
          if (Set.of("PLAY", "GET_PARAMETER", "TEARDOWN").contains(req.method)
              && (!setup || !sid.equals(req.headers.getOrDefault("session", "").split(";")[0]))) {
            response(out, req, 454, "Session Not Found", Map.of(), new byte[0]);
            continue;
          }
          switch (req.method) {
            case "DESCRIBE" -> {
              feed = app.hub.acquire(id);
              selected = id;
              Video v = feed.video;
              String sdp =
                  "v=0\r\n"
                      + "o=- 1 1 IN IP4 127.0.0.1\r\n"
                      + "s=UVMS Java\r\n"
                      + "t=0 0\r\n"
                      + "a=control:*\r\n"
                      + "m=video 0 RTP/AVP 96\r\n"
                      + "c=IN IP4 0.0.0.0\r\n"
                      + "a=rtpmap:96 H264/90000\r\n"
                      + "a=fmtp:96 packetization-mode=1;profile-level-id="
                      + v.codec().substring(5)
                      + ";sprop-parameter-sets="
                      + Base64.getEncoder().encodeToString(v.sps())
                      + ","
                      + Base64.getEncoder().encodeToString(v.pps())
                      + "\r\na=control:trackID=0\r\n";
              response(
                  out,
                  req,
                  200,
                  "OK",
                  Map.of(
                      "Content-Type",
                      "application/sdp",
                      "Content-Base",
                      req.uri.replaceAll("/+$", "") + "/"),
                  ascii(sdp));
            }
            case "SETUP" -> {
              String transport = req.headers.getOrDefault("transport", "");
              var m =
                  java.util.regex.Pattern.compile("interleaved=(\\d+)-(\\d+)").matcher(transport);
              if (!transport.startsWith("RTP/AVP/TCP;")
                  || transport.contains("multicast")
                  || !m.find()) {
                response(out, req, 461, "Unsupported Transport", Map.of(), new byte[0]);
                continue;
              }
              channel = Integer.parseInt(m.group(1));
              rtcp = Integer.parseInt(m.group(2));
              if (channel < 0 || channel > 255 || rtcp < 0 || rtcp > 255 || channel == rtcp)
                throw new IOException("channels");
              feed = app.hub.acquire(id);
              selected = id;
              setup = true;
              response(
                  out,
                  req,
                  200,
                  "OK",
                  Map.of(
                      "Session",
                      sid + ";timeout=60",
                      "Transport",
                      "RTP/AVP/TCP;unicast;interleaved=" + channel + "-" + rtcp),
                  new byte[0]);
            }
            case "PLAY" -> {
              if (sub == null) sub = feed.subscribe(true);
              playing = true;
              response(out, req, 200, "OK", Map.of("Session", sid), new byte[0]);
            }
            case "GET_PARAMETER" ->
                response(out, req, 200, "OK", Map.of("Session", sid), new byte[0]);
            case "TEARDOWN" -> {
              response(out, req, 200, "OK", Map.of("Session", sid), new byte[0]);
              return;
            }
            default -> response(out, req, 405, "Method Not Allowed", Map.of(), new byte[0]);
          }
        }
        if (playing) {
          Frame f = sub.next();
          if (f != null && (key || f.key())) {
            key = true;
            for (byte[] p : rtp.packetize(f, 96)) interleaved(out, channel, p);
            if (System.nanoTime() - report > 5_000_000_000L) {
              interleaved(out, rtcp, rtp.report());
              report = System.nanoTime();
            }
            out.flush();
          }
        }
      }
    } finally {
      if (sub != null) sub.close();
      reader.interrupt();
      watchdog.interrupt();
      socket.close();
    }
  }

  static InputStream socketInput(Socket s) {
    try {
      return s.getInputStream();
    } catch (IOException e) {
      throw new UncheckedIOException(e);
    }
  }

  public void close() {
    closed = true;
    try {
      server.close();
    } catch (IOException ignored) {
    }
    for (Socket s : clients)
      try {
        s.close();
      } catch (IOException ignored) {
      }
  }
}
