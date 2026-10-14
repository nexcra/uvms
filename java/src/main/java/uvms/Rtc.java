package uvms;

import static uvms.Media.*;

import java.io.*;
import java.math.BigInteger;
import java.net.*;
import java.nio.*;
import java.nio.charset.StandardCharsets;
import java.security.*;
import java.security.spec.ECGenParameterSpec;
import java.util.*;
import java.util.concurrent.*;
import java.util.regex.*;
import java.util.zip.CRC32;
import javax.crypto.*;
import javax.crypto.spec.*;
import org.bouncycastle.asn1.x500.X500Name;
import org.bouncycastle.cert.jcajce.JcaX509v3CertificateBuilder;
import org.bouncycastle.operator.jcajce.JcaContentSignerBuilder;
import org.bouncycastle.tls.*;
import org.bouncycastle.tls.crypto.TlsCertificate;
import org.bouncycastle.tls.crypto.TlsCryptoParameters;
import org.bouncycastle.tls.crypto.impl.jcajce.*;

/** ICE-lite, DTLS 1.2 and SRTP sender, all implemented in Java/JCA/BC. */
final class Rtc implements AutoCloseable, SharedUdp.Peer {
  static final SecureRandom RANDOM = new SecureRandom();
  static final KeyPair KEY;
  static final byte[] CERT;
  static final String FINGERPRINT;

  static {
    try {
      KeyPairGenerator g = KeyPairGenerator.getInstance("EC");
      g.initialize(new ECGenParameterSpec("secp256r1"));
      KEY = g.generateKeyPair();
      long now = System.currentTimeMillis();
      X500Name name = new X500Name("CN=UVMS Java");
      CERT =
          new JcaX509v3CertificateBuilder(
                  name,
                  new BigInteger(120, RANDOM),
                  new Date(now - 60000),
                  new Date(now + 365L * 86400000),
                  name,
                  KEY.getPublic())
              .build(new JcaContentSignerBuilder("SHA256withECDSA").build(KEY.getPrivate()))
              .getEncoded();
      FINGERPRINT =
          HexFormat.ofDelimiter(":")
              .withUpperCase()
              .formatHex(MessageDigest.getInstance("SHA-256").digest(CERT));
    } catch (Exception e) {
      throw new ExceptionInInitializerError(e);
    }
  }

  final Gateway app;
  final Hub.Feed feed;
  final SharedUdp udp;
  final String ufrag = random(8), password = random(32);
  final Rtp rtp = new Rtp();
  final ArrayBlockingQueue<byte[]> dtls = new ArrayBlockingQueue<>(128);
  volatile InetSocketAddress remote;
  volatile boolean closed, signaled;
  volatile long consent = System.nanoTime();
  String remoteUfrag, remoteFingerprint;
  int payload;
  volatile Srtp inbound;
  final java.util.concurrent.atomic.AtomicReferenceArray<byte[]> history =
      new java.util.concurrent.atomic.AtomicReferenceArray<>(1024);

  Rtc(Gateway app, Hub.Feed feed) throws Exception {
    this.app = app;
    this.feed = feed;
    udp = app.udp;
    udp.register(this);
    Thread.startVirtualThread(
        () -> {
          try {
            Thread.sleep(15000);
            if (!signaled) close();
          } catch (InterruptedException ignored) {
          }
        });
  }

  static String random(int n) {
    byte[] b = new byte[n];
    RANDOM.nextBytes(b);
    return Base64.getUrlEncoder().withoutPadding().encodeToString(b);
  }

  static String line(String s, String name) throws IOException {
    Matcher m = Pattern.compile("(?m)^" + Pattern.quote(name) + "([^\\r\\n]+)").matcher(s);
    if (!m.find()) throw new IOException("SDP missing field");
    return m.group(1).trim();
  }

  String answer(String offer) throws Exception {
    if (offer.length() > 128 * 1024) throw new IOException("SDP limit");
    remoteUfrag = line(offer, "a=ice-ufrag:");
    remoteFingerprint = line(offer, "a=fingerprint:");
    if (!remoteFingerprint.toLowerCase(Locale.ROOT).startsWith("sha-256 "))
      throw new IOException("SHA256 fingerprint required");
    remoteFingerprint = remoteFingerprint.substring(8).replace(":", "");
    String videoSection = null;
    for (String section : offer.split("(?m)(?=^m=)"))
      if (section.startsWith("m=video ")) {
        videoSection = section;
        break;
      }
    if (videoSection == null) throw new IOException("video offer required");
    String mid = line(videoSection, "a=mid:");
    String fmtp = null;
    Matcher codec =
        Pattern.compile("(?m)^a=rtpmap:(\\d+) H264/90000\\s*$", Pattern.CASE_INSENSITIVE)
            .matcher(videoSection);
    while (codec.find()) {
      int pt = Integer.parseInt(codec.group(1));
      String f;
      try {
        f = line(videoSection, "a=fmtp:" + pt + " ");
      } catch (IOException e) {
        continue;
      }
      if (!f.contains("packetization-mode=1")) continue;
      Matcher profile = Pattern.compile("profile-level-id=([0-9a-fA-F]{6})").matcher(f);
      if (profile.find()
          && !profile.group(1).substring(0, 2).equalsIgnoreCase(feed.video.codec().substring(5, 7)))
        continue;
      payload = pt;
      fmtp = f;
      break;
    }
    if (fmtp == null) throw new IOException("compatible H264 codec unavailable");
    var addresses = new LinkedHashSet<String>();
    for (var interfaces = NetworkInterface.getNetworkInterfaces(); interfaces.hasMoreElements(); ) {
      var ni = interfaces.nextElement();
      if (!ni.isUp()) continue;
      for (var ips = ni.getInetAddresses(); ips.hasMoreElements(); ) {
        var ip = ips.nextElement();
        if (ip instanceof Inet4Address && !ip.isLoopbackAddress())
          addresses.add(ip.getHostAddress());
      }
    }
    addresses.add("127.0.0.1");
    StringBuilder s =
        new StringBuilder(
            "v=0\r\no=- "
                + System.currentTimeMillis()
                + " 2 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\na=group:BUNDLE "
                + mid
                + "\r\na=msid-semantic: WMS stream_label\r\na=ice-lite\r\nm=video "
                + udp.port()
                + " UDP/TLS/RTP/SAVPF "
                + payload
                + "\r\nc=IN IP4 0.0.0.0\r\na=mid:"
                + mid
                + "\r\na=sendonly\r\na=rtcp-mux\r\na=rtcp-rsize\r\na=ice-ufrag:"
                + ufrag
                + "\r\na=ice-pwd:"
                + password
                + "\r\na=fingerprint:sha-256 "
                + FINGERPRINT
                + "\r\na=setup:passive\r\na=rtpmap:"
                + payload
                + " H264/90000\r\na=fmtp:"
                + payload
                + " "
                + fmtp
                + "\r\na=msid:stream_label video_label\r\na=ssrc:"
                + Integer.toUnsignedString(rtp.ssrc)
                + " cname:uvms-java\r\na=ssrc:"
                + Integer.toUnsignedString(rtp.ssrc)
                + " msid:stream_label video_label\r\n");
    s.append("a=rtcp-fb:").append(payload).append(" nack\r\n");
    int foundation = 1;
    String candidate = null;
    for (String ip : addresses) {
      String c =
          "candidate:"
              + (foundation++)
              + " 1 udp 2130706431 "
              + ip
              + " "
              + udp.port()
              + " typ host";
      if (candidate == null) candidate = c;
      s.append("a=").append(c).append("\r\n");
    }
    s.append("a=end-of-candidates\r\n");
    return Gateway.JSON.toJson(Map.of("type", "answer", "sdp", s.toString()))
        + "|-|-|"
        + Gateway.JSON.toJson(Map.of("candidate", candidate, "sdpMid", mid, "sdpMLineIndex", 0));
  }

  static byte[] mac(String algorithm, byte[] key, byte[] value) throws Exception {
    Mac mac = Mac.getInstance(algorithm);
    mac.init(new SecretKeySpec(key, algorithm));
    return mac.doFinal(value);
  }

  public String localUfrag() {
    return ufrag;
  }

  public synchronized void receive(byte[] b, InetSocketAddress sender) throws Exception {
    if (closed || app.closed) return;
    if (SharedUdp.isStun(b)) stun(b, sender);
    else if (b.length > 0 && sender.equals(remote)) {
      int first = b[0] & 255;
      if (first >= 20 && first <= 63) dtls.offer(b);
      else if (first >= 128 && first <= 191 && inbound != null) feedback(inbound.unprotectRtcp(b));
    }
  }

  void feedback(byte[] data) throws Exception {
    if (data == null) return;
    for (int at = 0; at + 4 <= data.length; ) {
      int length = (Short.toUnsignedInt(ByteBuffer.wrap(data).getShort(at + 2)) + 1) * 4;
      if (length < 4 || at + length > data.length) return;
      if ((data[at + 1] & 255) == 205
          && (data[at] & 31) == 1
          && length >= 12
          && ByteBuffer.wrap(data).getInt(at + 8) == rtp.ssrc) {
        for (int n = at + 12; n + 4 <= at + length; n += 4) {
          int pid = Short.toUnsignedInt(ByteBuffer.wrap(data).getShort(n)),
              mask = Short.toUnsignedInt(ByteBuffer.wrap(data).getShort(n + 2));
          resend(pid);
          for (int bit = 0; bit < 16; bit++)
            if ((mask & (1 << bit)) != 0) resend((pid + bit + 1) & 65535);
        }
      }
      at += length;
    }
  }

  void resend(int sequence) throws IOException {
    byte[] packet = history.get(sequence % history.length());
    if (packet != null && Short.toUnsignedInt(ByteBuffer.wrap(packet).getShort(2)) == sequence)
      send(packet);
  }

  void stun(byte[] b, InetSocketAddress sender) throws Exception {
    if (remoteUfrag == null || !validStun(b, ufrag + ":" + remoteUfrag, password)) return;
    byte[] addr = sender.getAddress().getAddress();
    if (addr.length != 4) return;
    if (!udp.authenticated(this, sender)) return;
    remote = sender;
    consent = System.nanoTime();
    byte[] mapped =
        join(
            u16(0x20, 8),
            new byte[] {0, 1},
            u16(sender.getPort() ^ 0x2112),
            u32(ByteBuffer.wrap(addr).getInt() ^ 0x2112a442));
    byte[] h = Arrays.copyOf(b, 20);
    ByteBuffer.wrap(h).putShort(0, (short) 0x0101).putShort(2, (short) (mapped.length + 24));
    byte[] prefix = join(h, mapped);
    byte[] response = join(prefix, u16(8, 20), mac("HmacSHA1", ascii(password), prefix));
    ByteBuffer.wrap(response).putShort(2, (short) (response.length + 8 - 20));
    CRC32 crc = new CRC32();
    crc.update(response);
    response = join(response, u16(0x8028, 4), u32(crc.getValue() ^ 0x5354554e));
    udp.send(this, response, sender);
  }

  static boolean validStun(byte[] b, String expectedUsername, String password) throws Exception {
    if (!SharedUdp.isStun(b)) return false;
    ByteBuffer data = ByteBuffer.wrap(b);
    if (data.getShort(0) != 1 || Short.toUnsignedInt(data.getShort(2)) + 20 != b.length)
      return false;
    int mi = -1;
    String username = null;
    for (int at = 20; at + 4 <= b.length; ) {
      int type = Short.toUnsignedInt(data.getShort(at)),
          n = Short.toUnsignedInt(data.getShort(at + 2));
      if (at + 4 + n > b.length) return false;
      if (type == 6) {
        if (username != null) return false;
        username = new String(b, at + 4, n, StandardCharsets.UTF_8);
      }
      if (type == 8) {
        if (n != 20) return false;
        mi = at;
        break;
      }
      at += 4 + ((n + 3) & ~3);
    }
    if (mi < 0 || !expectedUsername.equals(username)) return false;
    byte[] input = Arrays.copyOf(b, mi);
    ByteBuffer.wrap(input).putShort(2, (short) (mi + 4));
    byte[] expected = mac("HmacSHA1", ascii(password), input);
    return MessageDigest.isEqual(expected, Arrays.copyOfRange(b, mi + 4, mi + 24));
  }

  void send(byte[] bytes) throws IOException {
    if (remote == null || closed) throw new IOException("ICE closed");
    udp.send(this, bytes, remote);
  }

  final class Transport implements DatagramTransport {
    public int getReceiveLimit() {
      return 16384;
    }

    public int getSendLimit() {
      return 1200;
    }

    public int receive(byte[] b, int off, int len, int wait) throws IOException {
      if (closed) throw new IOException("closed");
      try {
        byte[] p = dtls.poll(Math.max(1, wait), TimeUnit.MILLISECONDS);
        if (p == null) return -1;
        if (p.length > len) throw new IOException("DTLS datagram limit");
        System.arraycopy(p, 0, b, off, p.length);
        return p.length;
      } catch (InterruptedException e) {
        Thread.currentThread().interrupt();
        throw new IOException("interrupted");
      }
    }

    public void send(byte[] b, int off, int len) throws IOException {
      Rtc.this.send(Arrays.copyOfRange(b, off, off + len));
    }

    public void close() {
      Rtc.this.close();
    }
  }

  final class DtlsServer extends DefaultTlsServer {
    final JcaTlsCrypto crypto;
    volatile byte[] keys;

    DtlsServer(JcaTlsCrypto crypto) {
      super(crypto);
      this.crypto = crypto;
    }

    protected ProtocolVersion[] getSupportedVersions() {
      return new ProtocolVersion[] {ProtocolVersion.DTLSv12};
    }

    protected int[] getSupportedCipherSuites() {
      return new int[] {CipherSuite.TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256};
    }

    public int getHandshakeTimeoutMillis() {
      return 12000;
    }

    protected TlsCredentialedSigner getECDSASignerCredentials() throws IOException {
      var chain =
          new org.bouncycastle.tls.Certificate(
              new TlsCertificate[] {crypto.createCertificate(CERT)});
      return new JcaDefaultTlsCredentialedSigner(
          new TlsCryptoParameters(context),
          crypto,
          KEY.getPrivate(),
          chain,
          new SignatureAndHashAlgorithm(HashAlgorithm.sha256, SignatureAlgorithm.ecdsa));
    }

    public CertificateRequest getCertificateRequest() {
      Vector<SignatureAndHashAlgorithm> algorithms = new Vector<>();
      algorithms.add(new SignatureAndHashAlgorithm(HashAlgorithm.sha256, SignatureAlgorithm.ecdsa));
      algorithms.add(new SignatureAndHashAlgorithm(HashAlgorithm.sha256, SignatureAlgorithm.rsa));
      return new CertificateRequest(
          new short[] {ClientCertificateType.ecdsa_sign, ClientCertificateType.rsa_sign},
          algorithms,
          new Vector<>());
    }

    public void notifyClientCertificate(org.bouncycastle.tls.Certificate certificate)
        throws IOException {
      try {
        if (certificate.isEmpty()
            || !MessageDigest.isEqual(
                HexFormat.of().parseHex(remoteFingerprint),
                MessageDigest.getInstance("SHA-256")
                    .digest(certificate.getCertificateAt(0).getEncoded())))
          throw new IOException("peer certificate fingerprint mismatch");
      } catch (GeneralSecurityException | IllegalArgumentException e) {
        throw new IOException("peer certificate invalid");
      }
    }

    public Hashtable getServerExtensions() throws IOException {
      Hashtable extensions = super.getServerExtensions();
      UseSRTPData offered = TlsSRTPUtils.getUseSRTPExtension(clientExtensions);
      if (offered == null
          || Arrays.stream(offered.getProtectionProfiles())
              .noneMatch(p -> p == SRTPProtectionProfile.SRTP_AES128_CM_HMAC_SHA1_80))
        throw new IOException("SRTP profile required");
      TlsSRTPUtils.addUseSRTPExtension(
          extensions,
          new UseSRTPData(
              new int[] {SRTPProtectionProfile.SRTP_AES128_CM_HMAC_SHA1_80}, new byte[0]));
      return extensions;
    }

    public void notifyHandshakeComplete() throws IOException {
      super.notifyHandshakeComplete();
      keys = context.exportKeyingMaterial("EXTRACTOR-dtls_srtp", null, 60);
    }
  }

  void run() throws Exception {
    try {
      var crypto = new JcaTlsCryptoProvider().create(RANDOM);
      var server = new DtlsServer(crypto);
      DTLSTransport transport = new DTLSServerProtocol().accept(server, new Transport());
      Thread.startVirtualThread(
          () -> {
            try {
              byte[] buffer = new byte[16384];
              while (!closed) transport.receive(buffer, 0, buffer.length, 1000);
            } catch (Exception ignored) {
              close();
            }
          });
      Srtp srtp =
          new Srtp(
              Arrays.copyOfRange(server.keys, 16, 32), Arrays.copyOfRange(server.keys, 46, 60));
      inbound =
          new Srtp(Arrays.copyOfRange(server.keys, 0, 16), Arrays.copyOfRange(server.keys, 32, 46));
      long report = 0;
      boolean key = false;
      try (var sub = feed.subscribe(true)) {
        while (!closed && !app.closed && System.nanoTime() - consent < 30_000_000_000L) {
          Frame f = sub.next();
          if (f == null || (!key && !f.key())) continue;
          key = true;
          for (byte[] p : rtp.packetize(f, payload)) {
            byte[] protectedPacket = srtp.rtp(p);
            history.set(
                Short.toUnsignedInt(ByteBuffer.wrap(p).getShort(2)) % history.length(),
                protectedPacket);
            send(protectedPacket);
          }
          if (System.nanoTime() - report > 5_000_000_000L) {
            send(srtp.rtcp(rtp.report()));
            report = System.nanoTime();
          }
        }
      }
    } finally {
      close();
    }
  }

  public void close() {
    closed = true;
    udp.unregister(this);
    dtls.clear();
  }

  static final class Srtp {
    final byte[] enc, auth, salt, rtcpEnc, rtcpAuth, rtcpSalt;
    long rollover;
    int last = -1, rtcpIndex;
    final Cipher cipher;
    final Mac mac;
    long lastRtcp = -1, replay;

    Srtp(byte[] key, byte[] masterSalt) throws Exception {
      enc = derive(key, masterSalt, 0, 16);
      auth = derive(key, masterSalt, 1, 20);
      salt = derive(key, masterSalt, 2, 14);
      rtcpEnc = derive(key, masterSalt, 3, 16);
      rtcpAuth = derive(key, masterSalt, 4, 20);
      rtcpSalt = derive(key, masterSalt, 5, 14);
      cipher = Cipher.getInstance("AES/CTR/NoPadding");
      mac = Mac.getInstance("HmacSHA1");
    }

    static byte[] derive(byte[] key, byte[] salt, int label, int size) throws Exception {
      byte[] iv = Arrays.copyOf(salt, 16);
      iv[7] ^= (byte) label;
      Cipher c = Cipher.getInstance("AES/CTR/NoPadding");
      c.init(Cipher.ENCRYPT_MODE, new SecretKeySpec(key, "AES"), new IvParameterSpec(iv));
      return c.doFinal(new byte[size]);
    }

    byte[] protect(
        byte[] packet,
        int header,
        byte[] key,
        byte[] authKey,
        byte[] salt,
        long index,
        byte[] trailer)
        throws Exception {
      byte[] iv = Arrays.copyOf(salt, 16);
      for (int i = 0; i < 4; i++) iv[4 + i] ^= packet[header == 12 ? 8 + i : 4 + i];
      for (int i = 0; i < 6; i++) iv[13 - i] ^= (byte) (index >> (8 * i));
      cipher.init(Cipher.ENCRYPT_MODE, new SecretKeySpec(key, "AES"), new IvParameterSpec(iv));
      int trailerSize = header == 8 ? trailer.length : 0;
      byte[] protectedPacket = new byte[packet.length + trailerSize + 10];
      System.arraycopy(packet, 0, protectedPacket, 0, header);
      cipher.doFinal(packet, header, packet.length - header, protectedPacket, header);
      mac.init(new SecretKeySpec(authKey, "HmacSHA1"));
      mac.update(protectedPacket, 0, packet.length);
      mac.update(trailer);
      byte[] tag = mac.doFinal();
      if (trailerSize != 0)
        System.arraycopy(trailer, 0, protectedPacket, packet.length, trailerSize);
      System.arraycopy(tag, 0, protectedPacket, packet.length + trailerSize, 10);
      return protectedPacket;
    }

    byte[] rtp(byte[] p) throws Exception {
      int seq = Short.toUnsignedInt(ByteBuffer.wrap(p).getShort(2));
      if (last >= 0 && seq < last && last - seq > 32768) rollover++;
      last = seq;
      return protect(p, 12, enc, auth, salt, (rollover << 16) | seq, u32(rollover));
    }

    byte[] rtcp(byte[] p) throws Exception {
      int index = rtcpIndex++;
      return protect(p, 8, rtcpEnc, rtcpAuth, rtcpSalt, index, u32(index | 0x80000000));
    }

    byte[] unprotectRtcp(byte[] p) throws Exception {
      if (p.length < 22) return null;
      int end = p.length - 14, flag = ByteBuffer.wrap(p).getInt(end);
      long index = flag & 0x7fffffffL;
      mac.init(new SecretKeySpec(rtcpAuth, "HmacSHA1"));
      if (!MessageDigest.isEqual(
          Arrays.copyOf(mac.doFinal(Arrays.copyOf(p, p.length - 10)), 10),
          Arrays.copyOfRange(p, p.length - 10, p.length))) return null;
      long delta = index - lastRtcp;
      if (delta <= 0 && (-delta >= 64 || (replay & (1L << -delta)) != 0)) return null;
      if (delta > 0) {
        replay = delta >= 64 ? 1 : (replay << delta) | 1;
        lastRtcp = index;
      } else replay |= 1L << -delta;
      byte[] body = Arrays.copyOf(p, end);
      if (flag < 0) {
        byte[] iv = Arrays.copyOf(rtcpSalt, 16);
        for (int i = 0; i < 4; i++) iv[4 + i] ^= p[4 + i];
        for (int i = 0; i < 6; i++) iv[13 - i] ^= (byte) (index >> (8 * i));
        cipher.init(
            Cipher.DECRYPT_MODE, new SecretKeySpec(rtcpEnc, "AES"), new IvParameterSpec(iv));
        System.arraycopy(cipher.doFinal(p, 8, end - 8), 0, body, 8, end - 8);
      }
      return body;
    }
  }
}
