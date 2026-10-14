package uvms;

import static uvms.Media.*;

import com.google.gson.*;
import io.netty.bootstrap.ServerBootstrap;
import io.netty.buffer.Unpooled;
import io.netty.channel.*;
import io.netty.channel.nio.NioEventLoopGroup;
import io.netty.channel.socket.SocketChannel;
import io.netty.channel.socket.nio.NioServerSocketChannel;
import io.netty.handler.codec.http.*;
import io.netty.handler.codec.http.websocketx.*;
import java.io.*;
import java.nio.file.*;
import java.util.*;
import java.util.concurrent.*;

public final class Gateway {
  static final Gson JSON = new Gson();

  static String failure(Exception e) {
    String message = e.getMessage();
    var known =
        Set.of(
            "nonmonotonic RTP clock",
            "H264 configuration changed",
            "frame queue limit",
            "access unit limit",
            "FU limit",
            "RTSP header limit",
            "RTSP body limit",
            "STAP truncated",
            "STAP size",
            "source ended or slow consumer",
            "MP4 timestamp or first IDR",
            "B slices unsupported",
            "parameter sets changed",
            "viewer write failed",
            "viewer closed",
            "compatible H264 codec unavailable",
            "SDP missing field",
            "H264 track required",
            "H264 SDP incomplete",
            "SDP parameter sets required");
    return e.getClass().getSimpleName()
        + (message != null && known.contains(message) ? ": " + message : "");
  }

  static final class Stream {
    String url_env;
    boolean record;
  }

  static final class Config {
    String bind = "127.0.0.1",
        web_root = "/opt/uvms/web",
        recording_dir = "/var/lib/uvms-java/recordings",
        default_stream = "camera_live";
    int port = 8088, rtsp_port = 8558, max_sessions = 32, ice_udp_min = 50200, ice_udp_max = 50300;
    int ice_udp_workers = 16;
    long segment_seconds = 10, recording_limit_bytes = 268435456;
    Map<String, Stream> streams = new TreeMap<>();

    void validate() {
      if (streams.isEmpty()
          || !streams.containsKey(default_stream)
          || max_sessions < 1
          || max_sessions > 1024
          || ice_udp_min < 1
          || ice_udp_min > 65535
          || ice_udp_workers < 1
          || ice_udp_workers > 16
          || segment_seconds < 2
          || recording_limit_bytes < 1048576)
        throw new IllegalArgumentException("invalid configuration");
      for (var entry : streams.entrySet())
        if (!entry.getKey().matches("[A-Za-z0-9_-]{1,64}")
            || entry.getValue().url_env == null
            || System.getenv(entry.getValue().url_env) == null)
          throw new IllegalArgumentException("invalid stream configuration");
    }
  }

  final Config config;
  final Hub hub;
  final SharedUdp udp;
  final Semaphore slots;
  final NioEventLoopGroup boss = new NioEventLoopGroup(1), workers = new NioEventLoopGroup(4);
  volatile boolean closed;
  Channel server;
  Relay relay;

  Gateway(Config cfg) throws Exception {
    config = cfg;
    cfg.validate();
    slots = new Semaphore(cfg.max_sessions);
    hub = new Hub(cfg);
    udp = new SharedUdp(cfg.ice_udp_min, cfg.ice_udp_workers);
  }

  public static void main(String[] args) throws Exception {
    String file = System.getenv().getOrDefault("UVMS_JAVA_CONFIG", "config.json");
    Config cfg = JSON.fromJson(Files.readString(Path.of(file)), Config.class);
    Gateway app = new Gateway(cfg);
    Runtime.getRuntime().addShutdownHook(new Thread(app::close));
    app.start();
    System.out.println("UVMS Java ready HTTP " + cfg.port + " RTSP " + cfg.rtsp_port);
    app.server.closeFuture().sync();
  }

  void start() throws Exception {
    hub.startRecorders();
    relay = new Relay(this);
    relay.start();
    server =
        new ServerBootstrap()
            .group(boss, workers)
            .channel(NioServerSocketChannel.class)
            .childOption(ChannelOption.TCP_NODELAY, true)
            .childOption(
                ChannelOption.WRITE_BUFFER_WATER_MARK,
                new WriteBufferWaterMark(128 * 1024, 512 * 1024))
            .childHandler(
                new ChannelInitializer<SocketChannel>() {
                  protected void initChannel(SocketChannel ch) {
                    ch.pipeline()
                        .addLast(
                            new HttpServerCodec(),
                            new HttpObjectAggregator(128 * 1024),
                            new Handler());
                  }
                })
            .bind(config.bind, config.port)
            .sync()
            .channel();
  }

  void close() {
    if (closed) return;
    closed = true;
    if (server != null) server.close();
    if (relay != null) relay.close();
    udp.close();
    hub.close();
    workers.shutdownGracefully();
    boss.shutdownGracefully();
  }

  static void send(Channel channel, WebSocketFrame frame) throws Exception {
    if (!channel.isActive()) {
      frame.release();
      throw new IOException("viewer closed");
    }
    var future = channel.writeAndFlush(frame);
    if (!future.await(5000) || !future.isSuccess()) throw new IOException("viewer write failed");
  }

  final class Handler extends SimpleChannelInboundHandler<Object> {
    Channel channel;
    String stream;
    boolean rtc;
    final java.util.concurrent.atomic.AtomicBoolean slotHeld =
        new java.util.concurrent.atomic.AtomicBoolean();

    void releaseSlot() {
      if (slotHeld.compareAndSet(true, false)) slots.release();
    }

    volatile boolean stopped, paused, reset;
    volatile String switchTo;
    Thread media;

    protected void channelRead0(ChannelHandlerContext ctx, Object message) throws Exception {
      channel = ctx.channel();
      if (message instanceof FullHttpRequest req) {
        String path = new QueryStringDecoder(req.uri()).path();
        if ("websocket".equalsIgnoreCase(req.headers().get("Upgrade"))) {
          String origin = req.headers().get("Origin"), host = req.headers().get("Host");
          if (origin != null
              && !origin.equals("http://" + host)
              && !origin.equals("https://" + host)) {
            http(ctx, 403, "text/plain", ascii("origin mismatch"));
            return;
          }
          rtc = path.startsWith("/webrtc_playnow/");
          String[] parts = path.split("/");
          stream = parts[parts.length - 1];
          if (rtc && (parts.length != 5 || !"udp".equals(parts[3]))) {
            http(ctx, 501, "text/plain", ascii("UDP only"));
            return;
          }
          if (!config.streams.containsKey(stream)) {
            http(ctx, 404, "text/plain", new byte[0]);
            return;
          }
          if (!slots.tryAcquire()) {
            http(ctx, 503, "text/plain", new byte[0]);
            return;
          }
          slotHeld.set(true);
          var handshake =
              new WebSocketServerHandshakerFactory("ws://" + host + path, null, true, 128 * 1024)
                  .newHandshaker(req);
          if (handshake == null) {
            releaseSlot();
            http(ctx, 400, "text/plain", new byte[0]);
            return;
          }
          handshake
              .handshake(channel, req)
              .addListener(
                  f -> {
                    if (!f.isSuccess()) {
                      releaseSlot();
                      return;
                    }
                    if (rtc) {

                      Thread.startVirtualThread(
                          () -> {
                            try {
                              Hub.Feed feed = hub.acquire(stream);
                              if (stopped || !channel.isActive()) {
                                releaseSlot();
                                return;
                              }
                              Rtc peer = new Rtc(Gateway.this, feed);
                              rtcPeer = peer;
                              if (stopped) {
                                peer.close();
                                releaseSlot();
                                return;
                              }
                              send(channel, new TextWebSocketFrame("H264|-|-|"));
                              Thread.startVirtualThread(
                                  () -> {
                                    try {
                                      Thread.sleep(12000);
                                      if (!peer.signaled) channel.close();
                                    } catch (InterruptedException ignored) {
                                    }
                                  });
                            } catch (Exception e) {
                              channel.close();
                              releaseSlot();
                            }
                          });
                    } else {

                      media = Thread.startVirtualThread(this::mse);
                    }
                  });
          return;
        }
        Thread.startVirtualThread(
            () -> {
              try {
                route(ctx, path);
              } catch (Exception e) {
                http(ctx, 500, "text/plain", ascii("request failed"));
              }
            });
      } else if (message instanceof PingWebSocketFrame p)
        ctx.writeAndFlush(new PongWebSocketFrame(p.content().retain()));
      else if (message instanceof CloseWebSocketFrame) ctx.close();
      else if (message instanceof TextWebSocketFrame text) {
        String command = text.text();
        if (rtc) {
          Rtc peer = rtcPeer;
          if (peer != null && !peer.signaled) {
            peer.signaled = true;
            Thread.startVirtualThread(
                () -> {
                  try {
                    String answer =
                        peer.answer(
                            JSON.fromJson(command, JsonObject.class).get("sdp").getAsString());
                    send(channel, new TextWebSocketFrame(answer));
                    send(channel, new CloseWebSocketFrame());
                    channel.close();
                    peer.run();
                  } catch (Exception e) {
                    System.err.println("WebRTC session failed: " + e.getClass().getSimpleName());
                    peer.close();
                    channel.close();
                  } finally {
                    releaseSlot();
                  }
                });
          }
        } else if (command.equalsIgnoreCase("pause")) paused = true;
        else if (command.equalsIgnoreCase("continue")) {
          paused = false;
          reset = true;
        } else if (command.startsWith("switch:")
            && config.streams.containsKey(command.substring(7))) {
          switchTo = command.substring(7);
          reset = true;
          paused = false;
        } else channel.close();
      }
    }

    volatile Rtc rtcPeer;

    public void channelInactive(ChannelHandlerContext ctx) {
      stopped = true;
      if (rtcPeer != null && !rtcPeer.signaled) {
        rtcPeer.close();
        releaseSlot();
      }
      if (media != null) media.interrupt();
    }

    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
      ctx.close();
    }

    void mse() {
      long next = 0;
      try {
        while (!stopped && !closed) {
          if (switchTo != null) {
            stream = switchTo;
            switchTo = null;
          }
          reset = false;
          Hub.Feed feed = hub.acquire(stream);
          Mp4 mux = new Mp4(feed.video);
          send(channel, new BinaryWebSocketFrame(Unpooled.wrappedBuffer(mux.envelope())));
          long base = -1, offset = next;
          try (var sub = feed.subscribe(true)) {
            while (!stopped && !closed && !reset) {
              Frame f = sub.next();
              if (f == null || paused) continue;
              if (base < 0) {
                if (!f.key()) continue;
                base = f.timestamp();
              }
              long ts = f.timestamp() - base + offset;
              send(channel, new BinaryWebSocketFrame(Unpooled.wrappedBuffer(mux.media(f, ts))));
              next = ts + f.duration();
            }
          }
        }
      } catch (Exception e) {
        if (!stopped && !closed) System.err.println("MSE " + stream + " ended: " + failure(e));
      } finally {
        releaseSlot();
        channel.close();
      }
    }
  }

  void route(ChannelHandlerContext ctx, String path) throws Exception {
    if (path.equals("/healthz")) {
      json(ctx, Map.of("status", "ok", "media_engine", "java", "version", "0.1.0"));
      return;
    }
    if (path.equals("/api/streams")) {
      var streams = new ArrayList<Map<String, Object>>();
      config.streams.forEach(
          (id, s) ->
              streams.add(
                  Map.of(
                      "id",
                      id,
                      "mode",
                      "native",
                      "online",
                      hub.online(id),
                      "record",
                      s.record,
                      "recording",
                      hub.recording.getOrDefault(id, "off"))));
      json(
          ctx,
          Map.of(
              "streams",
              streams,
              "video_only",
              true,
              "default_stream",
              config.default_stream,
              "rtsp_port",
              config.rtsp_port));
      return;
    }
    if (path.equals("/api/recordings")) {
      json(
          ctx,
          Map.of(
              "files",
              hub.files(),
              "used_bytes",
              hub.used.get(),
              "limit_bytes",
              config.recording_limit_bytes));
      return;
    }
    if (path.startsWith("/api/recordings/")) {
      String[] parts = path.split("/");
      if (parts.length != 5 || !config.streams.containsKey(parts[3]) || !Hub.safeName(parts[4])) {
        http(ctx, 404, "text/plain", new byte[0]);
        return;
      }
      Path p = Path.of(config.recording_dir, parts[3], parts[4]);
      if (!Files.isRegularFile(p, LinkOption.NOFOLLOW_LINKS)) {
        http(ctx, 404, "text/plain", new byte[0]);
        return;
      }
      var response = new DefaultHttpResponse(HttpVersion.HTTP_1_1, HttpResponseStatus.OK);
      response
          .headers()
          .set("Content-Type", "video/mp4")
          .set("Content-Length", Files.size(p))
          .set("Connection", "close");
      ctx.writeAndFlush(response).sync();
      try (var input = Files.newInputStream(p)) {
        byte[] buffer = new byte[65536];
        int n;
        while ((n = input.read(buffer)) >= 0) {
          var future =
              ctx.writeAndFlush(new DefaultHttpContent(Unpooled.copiedBuffer(buffer, 0, n)));
          if (!future.await(5000) || !future.isSuccess()) break;
        }
      }
      ctx.writeAndFlush(LastHttpContent.EMPTY_LAST_CONTENT)
          .addListener(ChannelFutureListener.CLOSE);
      return;
    }
    String name =
        switch (path) {
          case "/" -> "index.html";
          case "/app.js" -> "app.js";
          case "/style.css" -> "style.css";
          default -> null;
        };
    if (name == null) {
      http(ctx, 404, "text/plain", new byte[0]);
      return;
    }
    http(
        ctx,
        200,
        name.endsWith("js")
            ? "text/javascript"
            : name.endsWith("css") ? "text/css" : "text/html; charset=utf-8",
        Files.readAllBytes(Path.of(config.web_root, name)));
  }

  static void json(ChannelHandlerContext ctx, Object value) {
    http(
        ctx,
        200,
        "application/json",
        JSON.toJson(value).getBytes(java.nio.charset.StandardCharsets.UTF_8));
  }

  static void http(ChannelHandlerContext ctx, int code, String mime, byte[] body) {
    var r =
        new DefaultFullHttpResponse(
            HttpVersion.HTTP_1_1, HttpResponseStatus.valueOf(code), Unpooled.wrappedBuffer(body));
    r.headers()
        .set("Content-Type", mime)
        .set("Content-Length", body.length)
        .set("Cache-Control", "no-store")
        .set("Connection", "close");
    ctx.writeAndFlush(r).addListener(ChannelFutureListener.CLOSE);
  }
}
