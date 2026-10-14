package uvms;

import static uvms.Media.*;

import java.nio.*;
import java.security.SecureRandom;
import java.util.*;

final class Rtp {
  final int ssrc = new SecureRandom().nextInt();
  int sequence = new SecureRandom().nextInt(65536);
  long packets, octets, timestamp;
  long sentAt;

  List<byte[]> packetize(Frame frame, int payloadType) {
    var out = new ArrayList<byte[]>();
    for (int i = 0; i < frame.units.size(); i++) {
      Nal nal = frame.units.get(i);
      boolean last = i == frame.units.size() - 1;
      if (nal.length() <= 1100) {
        byte[] packet = header(nal.length(), frame.timestamp(), payloadType, last);
        System.arraycopy(frame.avcc, nal.offset(), packet, 12, nal.length());
        out.add(packet);
      } else {
        int type = frame.avcc[nal.offset()] & 255;
        for (int at = 1; at < nal.length(); ) {
          int n = Math.min(1098, nal.length() - at);
          boolean end = at + n == nal.length();
          byte[] packet = header(n + 2, frame.timestamp(), payloadType, last && end);
          packet[12] = (byte) ((type & 224) | 28);
          packet[13] = (byte) ((type & 31) | (at == 1 ? 128 : 0) | (end ? 64 : 0));
          System.arraycopy(frame.avcc, nal.offset() + at, packet, 14, n);
          out.add(packet);
          at += n;
        }
      }
    }
    timestamp = frame.timestamp();
    sentAt = System.nanoTime();
    return out;
  }

  private byte[] header(int length, long ts, int pt, boolean marker) {
    byte[] packet = new byte[12 + length];
    ByteBuffer.wrap(packet)
        .put((byte) 128)
        .put((byte) (pt | (marker ? 128 : 0)))
        .putShort((short) sequence++)
        .putInt((int) ts)
        .putInt(ssrc);
    packets++;
    octets += length;
    return packet;
  }

  byte[] report() {
    long ms = System.currentTimeMillis(),
        ntpSeconds = ms / 1000 + 2208988800L,
        fraction = ((ms % 1000) << 32) / 1000;
    byte[] sr =
        join(
            new byte[] {(byte) 128, (byte) 200, 0, 6},
            u32(
                ssrc,
                ntpSeconds,
                fraction,
                timestamp + (System.nanoTime() - sentAt) * 90000 / 1_000_000_000L,
                packets,
                octets));
    byte[] name = ascii("uvms-java"),
        chunk = join(u32(ssrc), new byte[] {1, (byte) name.length}, name, new byte[] {0});
    chunk = Arrays.copyOf(chunk, (chunk.length + 3) & ~3);
    return join(sr, new byte[] {(byte) 129, (byte) 202}, u16(chunk.length / 4), chunk);
  }
}
