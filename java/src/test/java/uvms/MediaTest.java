package uvms;

import static org.junit.jupiter.api.Assertions.*;
import static uvms.Media.*;

import java.nio.*;
import java.util.*;
import org.junit.jupiter.api.Test;

class MediaTest {
  @Test
  void overloadRecoversOnlyAtIdrAndRecordingDoesNotDrop() {
    Frame key = new Frame(hex("0000000265b8"), 0, 9000, true);
    Frame delta = new Frame(hex("0000000241e0"), 9000, 9000, false);
    var live = new Hub.FrameQueue(true);
    live.offer(delta);
    assertTrue(live.isEmpty());
    live.offer(key);
    for (int i = 0; i < 32; i++) live.offer(delta);
    assertTrue(live.isEmpty());
    live.offer(delta);
    assertTrue(live.isEmpty());
    live.offer(key);
    live.offer(delta);
    assertSame(key, live.poll());
    assertSame(delta, live.poll());
    var recording = new Hub.FrameQueue(false);
    for (int i = 0; i < 32; i++) assertTrue(recording.offer(delta));
    assertFalse(recording.offer(key));
    assertEquals(32, recording.size());
  }

  @Test
  void sharedNalIndexAndMixedPacketizationDoNotMutateSource() {
    byte[] large = new byte[4000];
    Arrays.fill(large, (byte) 0x55);
    large[0] = 0x65;
    byte[] avcc = join(hex("000000026764"), u32(large.length), large, hex("0000000268ee"));
    byte[] original = avcc.clone();
    var frame = new Frame(avcc, 100, 90, true);
    assertSame(frame.units, frame.withDuration(100).units);
    var first = new Rtp().packetize(frame, 96);
    var second = new Rtp().packetize(frame, 100);
    assertArrayEquals(original, avcc);
    assertEquals(first.size(), second.size());
    for (int i = 0; i < first.size(); i++) {
      assertArrayEquals(
          Arrays.copyOfRange(first.get(i), 12, first.get(i).length),
          Arrays.copyOfRange(second.get(i), 12, second.get(i).length));
      assertEquals(i == first.size() - 1, (first.get(i)[1] & 128) != 0);
    }
    var bits = new Bits(hex("650000030080"));
    assertEquals(0, bits.read(24));
    assertEquals(128, bits.read(8));
    assertThrows(IllegalArgumentException.class, () -> bits.read(1));
  }

  static byte[] hex(String s) {
    return HexFormat.of().parseHex(s);
  }

  @Test
  void rfc3711KeyDerivation() throws Exception {
    byte[] key = hex("E1F97A0D3E018BE0D64FA32C06DE4139"),
        salt = hex("0EC675AD498AFEEBB6960B3AABE6");
    assertArrayEquals(hex("C61E7A93744F39EE10734AFE3FF7A087"), Rtc.Srtp.derive(key, salt, 0, 16));
    assertArrayEquals(
        hex("CEBE321F6FF7716B6FD4AB49AF256A156D38BAA4"), Rtc.Srtp.derive(key, salt, 1, 20));
    assertArrayEquals(hex("30CBBC08863D8C85D49DB34A9AE1"), Rtc.Srtp.derive(key, salt, 2, 14));
  }

  @Test
  void rtcpAuthenticationAndReplay() throws Exception {
    byte[] key = new byte[16], salt = new byte[14];
    var sender = new Rtc.Srtp(key, salt);
    var receiver = new Rtc.Srtp(key, salt);
    byte[] message = hex("80c80006010203040000000000000000000000000000000000000000");
    byte[] encrypted = sender.rtcp(message);
    byte[] altered = encrypted.clone();
    altered[9] ^= 1;
    assertNull(receiver.unprotectRtcp(altered));
    assertArrayEquals(message, receiver.unprotectRtcp(encrypted));
    assertNull(receiver.unprotectRtcp(encrypted));
  }

  @Test
  void sourceSpsDimensionsAndMalformedInput() {
    var v = video(hex("6742c016da0280bfe5c0440000030004000003007a3c58ba80"), hex("68ce3c80"));
    assertEquals(640, v.width());
    assertEquals(360, v.height());
    assertThrows(IllegalArgumentException.class, () -> video(new byte[] {0x67}, new byte[] {0x68}));
    assertThrows(IllegalArgumentException.class, () -> nals(hex("ffffffff")));
  }

  @Test
  void originalMseEnvelopeAndMonotonicFragments() {
    var v = video(hex("6742c016da0280bfe5c0440000030004000003007a3c58ba80"), hex("68ce3c80"));
    var mux = new Mp4(v);
    byte[] init = mux.envelope();
    assertEquals(1, init[0]);
    assertEquals(8, init[1]);
    int xml =
        Short.toUnsignedInt(ByteBuffer.wrap(init).order(ByteOrder.LITTLE_ENDIAN).getShort(18));
    assertEquals("ftyp", new String(init, 24 + xml, 4));
    Frame f = new Frame(hex("0000000265b8"), 0, 9000, true);
    byte[] media = mux.media(f, 90000);
    assertEquals(2, media[0]);
    assertEquals(1000, ByteBuffer.wrap(media).order(ByteOrder.LITTLE_ENDIAN).getInt(6));
    assertThrows(IllegalArgumentException.class, () -> mux.fragment(f, 90000));
    assertThrows(
        IllegalArgumentException.class,
        () -> new Mp4(v).fragment(new Frame(hex("0000000241a0"), 0, 9000, false), 0));
  }

  @Test
  void fragmentationPreservesCompressedPayload() {
    byte[] nal = new byte[5001];
    nal[0] = 0x65;
    for (int i = 1; i < nal.length; i++) nal[i] = (byte) i;
    var rtp = new Rtp();
    var packets = rtp.packetize(new Frame(join(u32(nal.length), nal), 123456, 9000, true), 96);
    var assembled = new java.io.ByteArrayOutputStream();
    assembled.write(nal[0]);
    for (int i = 0; i < packets.size(); i++) {
      byte[] p = packets.get(i);
      assertEquals(123456, ByteBuffer.wrap(p).getInt(4));
      assertEquals(i == packets.size() - 1, (p[1] & 128) != 0);
      assembled.write(p, 14, p.length - 14);
    }
    assertArrayEquals(nal, assembled.toByteArray());
  }
}
