package uvms;

import static uvms.Media.*;

import java.io.*;
import java.net.*;
import java.nio.*;
import java.nio.charset.StandardCharsets;
import java.security.*;
import java.util.*;
import java.util.regex.*;

/** Direct RTSP/TCP camera input. Credentials never leave this connection. */
final class Rtsp implements AutoCloseable {
  final Socket socket = new Socket();
  final InputStream in;
  final OutputStream out;
  final String base, user, password;
  String session, challenge;
  int cseq, nc, channel = 0, payloadType;
  Video video;
  long keepalive = System.nanoTime();
  final ArrayDeque<Frame> ready = new ArrayDeque<>();
  int lastSequence = -1;
  long timestamp = -1, origin = -1, extended = 0, lastRaw = -1;
  boolean damaged = false, waiting = true;
  final ByteArrayOutputStream au = new ByteArrayOutputStream(), fu = new ByteArrayOutputStream();
  boolean fragmentOpen;
  Frame pending;

  record Response(int status, Map<String, String> headers, byte[] body) {}

  Rtsp(String address) throws Exception {
    URI u = URI.create(address);
    if (!"rtsp".equals(u.getScheme()) || u.getHost() == null)
      throw new IOException("RTSP URL invalid");
    String info = u.getRawUserInfo();
    String[] parts = info == null ? new String[] {"", ""} : info.split(":", 2);
    user = decode(parts[0]);
    password = parts.length > 1 ? decode(parts[1]) : "";
    base =
        new URI("rtsp", null, u.getHost(), u.getPort(), u.getPath(), u.getQuery(), null)
            .toASCIIString();
    try {
      socket.connect(new InetSocketAddress(u.getHost(), u.getPort() < 0 ? 554 : u.getPort()), 5000);
      socket.setSoTimeout(10000);
      socket.setTcpNoDelay(true);
      in = new BufferedInputStream(socket.getInputStream(), 131072);
      out = socket.getOutputStream();
      request("OPTIONS", base, Map.of());
      Response describe = request("DESCRIBE", base, Map.of("Accept", "application/sdp"));
      String sdp = new String(describe.body, StandardCharsets.UTF_8),
          section = null,
          control = null,
          fmtp = null;
      for (String candidate : sdp.split("(?m)(?=^m=)"))
        if (candidate.startsWith("m=video ")) {
          Matcher m =
              Pattern.compile("(?m)^a=rtpmap:(\\d+) H264/90000\\s*$", Pattern.CASE_INSENSITIVE)
                  .matcher(candidate);
          if (m.find()) {
            section = candidate;
            payloadType = Integer.parseInt(m.group(1));
            break;
          }
        }
      if (section == null) throw new IOException("H264 track required");
      for (String line : section.split("\\r?\\n")) {
        if (line.startsWith("a=control:")) control = line.substring(10).trim();
        if (line.startsWith("a=fmtp:" + payloadType + " ")) fmtp = line;
      }
      if (control == null || fmtp == null) throw new IOException("H264 SDP incomplete");
      Matcher sets = Pattern.compile("sprop-parameter-sets=([^;\\s]+)").matcher(fmtp);
      if (!sets.find()) throw new IOException("SDP parameter sets required");
      String[] pair = sets.group(1).split(",");
      video = video(Base64.getDecoder().decode(pair[0]), Base64.getDecoder().decode(pair[1]));
      String content =
          describe.headers.getOrDefault("content-base", base.endsWith("/") ? base : base + "/");
      String track =
          control.startsWith("rtsp://") ? control : URI.create(content).resolve(control).toString();
      Response setup =
          request("SETUP", track, Map.of("Transport", "RTP/AVP/TCP;unicast;interleaved=0-1"));
      String sid = setup.headers.get("session");
      if (sid == null) throw new IOException("missing RTSP session");
      session = sid.split(";")[0];
      Matcher inter =
          Pattern.compile("interleaved=(\\d+)-(\\d+)")
              .matcher(setup.headers.getOrDefault("transport", ""));
      if (!inter.find()) throw new IOException("TCP transport required");
      channel = Integer.parseInt(inter.group(1));
      request("PLAY", base, Map.of("Range", "npt=0.000-"));
    } catch (Exception e) {
      socket.close();
      throw new IOException("RTSP camera connection failed", e);
    }
  }

  static String decode(String s) {
    return URLDecoder.decode(s.replace("+", "%2B"), StandardCharsets.UTF_8);
  }

  static String digest(String alg, String value) throws Exception {
    return HexFormat.of()
        .formatHex(MessageDigest.getInstance(alg).digest(value.getBytes(StandardCharsets.UTF_8)));
  }

  String authorization(String method, String uri) throws Exception {
    if (challenge == null) return null;
    if (challenge.regionMatches(true, 0, "Basic", 0, 5))
      return "Basic "
          + Base64.getEncoder()
              .encodeToString((user + ":" + password).getBytes(StandardCharsets.UTF_8));
    var p = new HashMap<String, String>();
    Matcher m = Pattern.compile("(\\w+)=(?:\"([^\"]*)\"|([^,\\s]+))").matcher(challenge);
    while (m.find())
      p.put(m.group(1).toLowerCase(Locale.ROOT), m.group(2) != null ? m.group(2) : m.group(3));
    String algorithm = p.getOrDefault("algorithm", "MD5"),
        hash = algorithm.toUpperCase(Locale.ROOT).startsWith("SHA-256") ? "SHA-256" : "MD5";
    if (!Set.of("MD5", "MD5-SESS", "SHA-256", "SHA-256-SESS")
        .contains(algorithm.toUpperCase(Locale.ROOT)))
      throw new IOException("Digest algorithm unsupported");
    String realm = p.get("realm"),
        nonce = p.get("nonce"),
        cn = UUID.randomUUID().toString().replace("-", ""),
        count = String.format("%08x", ++nc);
    String ha1 = digest(hash, user + ":" + realm + ":" + password);
    if (algorithm.toLowerCase(Locale.ROOT).endsWith("-sess"))
      ha1 = digest(hash, ha1 + ":" + nonce + ":" + cn);
    String qop = p.get("qop");
    if (qop != null && !Arrays.asList(qop.split("\\s*,\\s*")).contains("auth"))
      throw new IOException("Digest qop unsupported");
    String response =
        digest(
            hash,
            ha1
                + ":"
                + nonce
                + ":"
                + (qop != null ? count + ":" + cn + ":auth:" : "")
                + digest(hash, method + ":" + uri));
    return "Digest username=\""
        + user
        + "\", realm=\""
        + realm
        + "\", nonce=\""
        + nonce
        + "\", uri=\""
        + uri
        + "\", response=\""
        + response
        + "\", algorithm="
        + algorithm
        + (qop != null ? ", qop=auth, nc=" + count + ", cnonce=\"" + cn + "\"" : "")
        + (p.containsKey("opaque") ? ", opaque=\"" + p.get("opaque") + "\"" : "");
  }

  Response request(String method, String uri, Map<String, String> headers) throws Exception {
    for (int retry = 0; retry < 2; retry++) {
      StringBuilder text =
          new StringBuilder(
              method
                  + " "
                  + uri
                  + " RTSP/1.0\r\nCSeq: "
                  + (++cseq)
                  + "\r\nUser-Agent: UVMS-Java\r\n");
      if (session != null) text.append("Session: ").append(session).append("\r\n");
      String auth = authorization(method, uri);
      if (auth != null) text.append("Authorization: ").append(auth).append("\r\n");
      headers.forEach((k, v) -> text.append(k).append(": ").append(v).append("\r\n"));
      out.write(text.append("\r\n").toString().getBytes(StandardCharsets.UTF_8));
      out.flush();
      Response response;
      do {
        response = read();
      } while (response == null);
      if (response.status == 401 && retry == 0 && !user.isEmpty()) {
        challenge = response.headers.get("www-authenticate");
        continue;
      }
      if (response.status != 200) throw new IOException("RTSP status " + response.status);
      return response;
    }
    throw new IOException("camera authentication failed");
  }

  Response read() throws Exception {
    int first = in.read();
    if (first < 0) throw new EOFException();
    if (first == 36) {
      int ch = in.read(), a = in.read(), b = in.read();
      if (b < 0) throw new EOFException();
      byte[] packet = in.readNBytes((a << 8) | b);
      if (packet.length != ((a << 8) | b)) throw new EOFException();
      if (ch == channel && video != null) rtp(packet);
      return null;
    }
    var h = new ByteArrayOutputStream();
    h.write(first);
    int state = 0;
    while (state != 4) {
      int v = in.read();
      if (v < 0) throw new EOFException();
      h.write(v);
      if (h.size() > 16384) throw new IOException("RTSP header limit");
      state =
          switch (state) {
            case 0 -> v == 13 ? 1 : 0;
            case 1 -> v == 10 ? 2 : 0;
            case 2 -> v == 13 ? 3 : 0;
            default -> v == 10 ? 4 : 0;
          };
    }
    String[] lines = h.toString(StandardCharsets.UTF_8).split("\r\n");
    int code = Integer.parseInt(lines[0].split(" ")[1]);
    var headers = new HashMap<String, String>();
    for (int i = 1; i < lines.length; i++) {
      int colon = lines[i].indexOf(':');
      if (colon > 0)
        headers.put(
            lines[i].substring(0, colon).toLowerCase(Locale.ROOT),
            lines[i].substring(colon + 1).trim());
    }
    int n = Integer.parseInt(headers.getOrDefault("content-length", "0"));
    if (n < 0 || n > 1048576) throw new IOException("RTSP body limit");
    byte[] body = in.readNBytes(n);
    if (body.length != n) throw new EOFException();
    return new Response(code, headers, body);
  }

  Frame next() throws Exception {
    while (ready.isEmpty()) {
      if (System.nanoTime() - keepalive > 15_000_000_000L) {
        request("OPTIONS", base, Map.of());
        keepalive = System.nanoTime();
      }
      read();
    }
    return ready.remove();
  }

  void nal(byte[] n) throws IOException {
    int type = n[0] & 31;
    if (type == 7 && !Arrays.equals(video.sps(), n) || type == 8 && !Arrays.equals(video.pps(), n))
      throw new IOException("H264 configuration changed");
    if (au.size() + n.length + 4 > 32 * 1024 * 1024) throw new IOException("access unit limit");
    au.writeBytes(u32(n.length));
    au.writeBytes(n);
  }

  void rtp(byte[] p) throws Exception {
    if (p.length < 12 || (p[0] & 0xc0) != 0x80 || (p[1] & 127) != payloadType) return;
    int seq = ((p[2] & 255) << 8) | (p[3] & 255);
    long raw = Integer.toUnsignedLong(ByteBuffer.wrap(p).getInt(4));
    if (lastRaw >= 0) {
      long delta = (raw - lastRaw) & 0xffffffffL;
      if (delta > 0x7fffffffL) throw new IOException("nonmonotonic RTP clock");
      extended += delta;
    }
    lastRaw = raw;
    if (lastSequence >= 0 && seq != ((lastSequence + 1) & 65535)) {
      damaged = true;
      waiting = true;
      au.reset();
      fu.reset();
      fragmentOpen = false;
    }
    lastSequence = seq;
    if (timestamp != extended) {
      au.reset();
      fu.reset();
      fragmentOpen = false;
      damaged = false;
      timestamp = extended;
    }
    int start = 12 + 4 * (p[0] & 15), end = p.length;
    if ((p[0] & 16) != 0) {
      if (start + 4 > end) return;
      start += 4 + 4 * (((p[start + 2] & 255) << 8) | (p[start + 3] & 255));
    }
    if ((p[0] & 32) != 0) end -= p[end - 1] & 255;
    if (start >= end) return;
    int type = p[start] & 31;
    if (type >= 1 && type <= 23) nal(Arrays.copyOfRange(p, start, end));
    else if (type == 24) {
      int at = start + 1;
      while (at < end) {
        if (at + 2 > end) throw new IOException("STAP truncated");
        int n = ((p[at] & 255) << 8) | (p[at + 1] & 255);
        at += 2;
        if (n < 1 || at + n > end) throw new IOException("STAP size");
        nal(Arrays.copyOfRange(p, at, at + n));
        at += n;
      }
    } else if (type == 28) {
      if (start + 2 >= end) return;
      int flags = p[start + 1] & 255;
      if ((flags & 128) != 0) {
        fu.reset();
        fu.write((p[start] & 224) | (flags & 31));
        fragmentOpen = true;
      } else if (!fragmentOpen) {
        damaged = true;
        return;
      }
      if (fu.size() + end - start > 32 * 1024 * 1024) throw new IOException("FU limit");
      fu.write(p, start + 2, end - start - 2);
      if ((flags & 64) != 0) {
        nal(fu.toByteArray());
        fragmentOpen = false;
        fu.reset();
      }
    } else {
      damaged = true;
    }
    if ((p[1] & 128) != 0) {
      if (!damaged && !fragmentOpen && au.size() > 0) {
        byte[] avcc = au.toByteArray();
        var units = index(avcc);
        byte[] accessUnit = avcc;
        boolean key = units.stream().anyMatch(n -> (accessUnit[n.offset()] & 31) == 5);
        if (key || !waiting) {
          if (key) {
            waiting = false;
            byte[] sps =
                units.stream().anyMatch(n -> (accessUnit[n.offset()] & 31) == 7)
                    ? new byte[0]
                    : join(u32(video.sps().length), video.sps());
            byte[] pps =
                units.stream().anyMatch(n -> (accessUnit[n.offset()] & 31) == 8)
                    ? new byte[0]
                    : join(u32(video.pps().length), video.pps());
            avcc = join(sps, pps, avcc);
          }
          if (origin < 0) origin = timestamp;
          Frame next = new Frame(avcc, timestamp - origin, 9000, key);
          if (pending != null) {
            long duration = next.timestamp() - pending.timestamp();
            if (duration > 0 && duration <= 900000) ready.add(pending.withDuration((int) duration));
          }
          pending = next;
          if (ready.size() > 128) throw new IOException("frame queue limit");
        }
      }
      au.reset();
      damaged = false;
    }
  }

  public void close() {
    try {
      socket.close();
    } catch (IOException ignored) {
    }
  }
}
