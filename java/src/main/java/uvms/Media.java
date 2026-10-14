package uvms;

import java.io.*;
import java.nio.*;
import java.nio.charset.StandardCharsets;
import java.util.*;

/** Compressed H264 access units; the same AVCC and ISO-BMFF boundary as the Rust port. */
final class Media {
  record Video(byte[] sps, byte[] pps, int width, int height) {
    String codec() {
      return "avc1." + HexFormat.of().withUpperCase().formatHex(sps, 1, 4);
    }
  }

  // One immutable access unit and NAL index shared by every subscriber. Ownership of
  // avcc transfers here; producers and consumers must not mutate its bytes.
  static final class Frame {
    final byte[] avcc;
    final long timestamp;
    final int duration;
    final boolean key;
    final List<Nal> units;

    Frame(byte[] avcc, long timestamp, int duration, boolean key) {
      this(avcc, timestamp, duration, key, index(avcc));
    }

    private Frame(byte[] avcc, long timestamp, int duration, boolean key, List<Nal> units) {
      this.avcc = avcc;
      this.timestamp = timestamp;
      this.duration = duration;
      this.key = key;
      this.units = units;
    }

    Frame withDuration(int duration) {
      return new Frame(avcc, timestamp, duration, key, units);
    }

    byte[] avcc() {
      return avcc;
    }

    long timestamp() {
      return timestamp;
    }

    int duration() {
      return duration;
    }

    boolean key() {
      return key;
    }
  }

  record Nal(int offset, int length) {}

  static List<Nal> index(byte[] avcc) {
    var result = new ArrayList<Nal>();
    for (int at = 0; at < avcc.length; ) {
      if (avcc.length - at < 4) throw new IllegalArgumentException("truncated AVCC");
      int length = ByteBuffer.wrap(avcc).getInt(at);
      at += 4;
      if (length < 1 || length > avcc.length - at)
        throw new IllegalArgumentException("invalid NAL size");
      result.add(new Nal(at, length));
      at += length;
    }
    return List.copyOf(result);
  }

  static byte[] join(byte[]... values) {
    int size = 0;
    for (var v : values) size = Math.addExact(size, v.length);
    byte[] out = new byte[size];
    int at = 0;
    for (var v : values) {
      System.arraycopy(v, 0, out, at, v.length);
      at += v.length;
    }
    return out;
  }

  static byte[] u32(long... v) {
    var b = ByteBuffer.allocate(v.length * 4);
    for (long n : v) b.putInt((int) n);
    return b.array();
  }

  static byte[] u16(int... v) {
    var b = ByteBuffer.allocate(v.length * 2);
    for (int n : v) b.putShort((short) n);
    return b.array();
  }

  static byte[] ascii(String s) {
    return s.getBytes(StandardCharsets.US_ASCII);
  }

  static byte[] box(String type, byte[]... data) {
    byte[] b = join(data);
    return join(u32(b.length + 8), ascii(type), b);
  }

  static byte[] full(String type, int version, int flags, byte[]... data) {
    return box(type, u32(((long) version << 24) | flags), join(data));
  }

  static List<byte[]> nals(byte[] avcc) {
    var out = new ArrayList<byte[]>();
    var b = ByteBuffer.wrap(avcc);
    while (b.hasRemaining()) {
      if (b.remaining() < 4) throw new IllegalArgumentException("truncated AVCC");
      int n = b.getInt();
      if (n < 1 || n > b.remaining()) throw new IllegalArgumentException("invalid NAL size");
      byte[] nal = new byte[n];
      b.get(nal);
      out.add(nal);
    }
    return out;
  }

  static final class Bits {
    final byte[] data;
    final int end;
    int at, zeros, current, remaining;

    Bits(byte[] nal) {
      this(nal, 0, nal.length);
    }

    Bits(byte[] nal, int offset, int length) {
      data = nal;
      at = offset + 1;
      end = offset + length;
    }

    int read(int n) {
      if (n < 0 || n > 31) throw new IllegalArgumentException("truncated H264 bits");
      int x = 0;
      while (n-- > 0) {
        if (remaining == 0) {
          if (at >= end) throw new IllegalArgumentException("truncated H264 bits");
          current = data[at++] & 255;
          if (zeros >= 2 && current == 3) {
            zeros = 0;
            if (at >= end) throw new IllegalArgumentException("truncated H264 bits");
            current = data[at++] & 255;
          }
          zeros = current == 0 ? zeros + 1 : 0;
          remaining = 8;
        }
        x = (x << 1) | ((current >> --remaining) & 1);
      }
      return x;
    }

    int ue() {
      int z = 0;
      while (read(1) == 0)
        if (++z > 29) throw new IllegalArgumentException("H264 integer overflow");
      return (1 << z) - 1 + read(z);
    }

    int se() {
      int x = ue();
      return (x & 1) != 0 ? (x + 1) / 2 : -x / 2;
    }
  }

  static Video video(byte[] sps, byte[] pps) {
    if (sps.length < 4 || pps.length == 0 || (sps[0] & 31) != 7 || (pps[0] & 31) != 8)
      throw new IllegalArgumentException("parameter sets");
    Bits b = new Bits(sps);
    int profile = b.read(8);
    b.read(16);
    b.ue();
    int chroma = 1, separate = 0;
    if (Set.of(100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135).contains(profile)) {
      chroma = b.ue();
      if (chroma > 3) throw new IllegalArgumentException("chroma");
      if (chroma == 3) separate = b.read(1);
      b.ue();
      b.ue();
      b.read(1);
      if (b.read(1) != 0)
        for (int i = 0; i < (chroma == 3 ? 12 : 8); i++)
          if (b.read(1) != 0) {
            int last = 8, next = 8;
            for (int j = 0; j < (i < 6 ? 16 : 64); j++) {
              if (next != 0) next = Math.floorMod(last + b.se(), 256);
              if (next != 0) last = next;
            }
          }
    }
    b.ue();
    int order = b.ue();
    if (order == 0) b.ue();
    else if (order == 1) {
      b.read(1);
      b.se();
      b.se();
      int n = b.ue();
      if (n > 255) throw new IllegalArgumentException("POC");
      for (int i = 0; i < n; i++) b.se();
    } else if (order != 2) throw new IllegalArgumentException("POC");
    b.ue();
    b.read(1);
    int w = b.ue(), h = b.ue(), frame = b.read(1);
    if (frame == 0) b.read(1);
    b.read(1);
    int l = 0, r = 0, t = 0, d = 0;
    if (b.read(1) != 0) {
      l = b.ue();
      r = b.ue();
      t = b.ue();
      d = b.ue();
    }
    int array = separate != 0 ? 0 : chroma;
    int width = (w + 1) * 16 - (l + r) * (array == 1 || array == 2 ? 2 : 1),
        height = (h + 1) * 16 * (2 - frame) - (t + d) * (array == 1 ? 2 : 1) * (2 - frame);
    if (width < 1 || height < 1 || width > 8192 || height > 8192)
      throw new IllegalArgumentException("dimensions");
    return new Video(sps, pps, width, height);
  }

  static final class Mp4 {
    final Video video;
    final byte[] init;
    int sequence = 1;
    long previous = -1;

    Mp4(Video v) {
      video = v;
      byte[] matrix = u32(65536, 0, 0, 0, 65536, 0, 0, 0, 0x40000000);
      byte[] ftyp = box("ftyp", ascii("isom"), u32(512), ascii("isomiso2iso5avc1mp41"));
      byte[] mvhd =
          full(
              "mvhd",
              0,
              0,
              new byte[8],
              u32(1000, 0, 65536),
              u16(256),
              new byte[10],
              matrix,
              new byte[24],
              u32(2));
      byte[] tkhd =
          full(
              "tkhd",
              0,
              7,
              new byte[8],
              u32(1, 0, 0),
              new byte[16],
              matrix,
              u32(v.width << 16, v.height << 16));
      byte[] mdhd = full("mdhd", 0, 0, new byte[8], u32(90000, 0), u16(0x55c4, 0));
      byte[] hdlr =
          full("hdlr", 0, 0, new byte[4], ascii("vide"), new byte[12], ascii("VideoHandler\0"));
      byte[] avcc =
          join(
              new byte[] {1, v.sps[1], v.sps[2], v.sps[3], (byte) 255, (byte) 225},
              u16(v.sps.length),
              v.sps,
              new byte[] {1},
              u16(v.pps.length),
              v.pps);
      if (Set.of(100, 110, 122, 144).contains(v.sps[1] & 255)) {
        Bits b = new Bits(v.sps);
        b.read(24);
        b.ue();
        int c = b.ue();
        if (c == 3) b.read(1);
        avcc =
            join(
                avcc,
                new byte[] {(byte) (252 | c), (byte) (248 | b.ue()), (byte) (248 | b.ue()), 0});
      }
      byte[] avc1 =
          box(
              "avc1",
              new byte[6],
              u16(1),
              new byte[16],
              u16(v.width, v.height),
              u32(0x480000, 0x480000, 0),
              u16(1),
              new byte[32],
              u16(24, 65535),
              box("avcC", avcc));
      byte[] stbl =
          box(
              "stbl",
              full("stsd", 0, 0, u32(1), avc1),
              full("stts", 0, 0, u32(0)),
              full("stsc", 0, 0, u32(0)),
              full("stsz", 0, 0, u32(0, 0)),
              full("stco", 0, 0, u32(0)));
      byte[] minf =
          box(
              "minf",
              full("vmhd", 0, 1, new byte[8]),
              box("dinf", full("dref", 0, 0, u32(1), full("url ", 0, 1))),
              stbl);
      init =
          join(
              ftyp,
              box(
                  "moov",
                  mvhd,
                  box("trak", tkhd, box("mdia", mdhd, hdlr, minf)),
                  box("mvex", full("trex", 0, 0, u32(1, 1, 0, 0, 0x01010000)))));
    }

    byte[] fragment(Frame f, long ts) {
      return fragment(f, ts, new byte[0]);
    }

    private byte[] fragment(Frame f, long ts, byte[] prefix) {
      if (ts <= previous || f.duration <= 0 || (previous < 0 && !f.key))
        throw new IllegalArgumentException("MP4 timestamp or first IDR");
      for (Nal nal : f.units) {
        int type = f.avcc[nal.offset] & 31;
        if (type == 1 || type == 5) {
          Bits b = new Bits(f.avcc, nal.offset, nal.length);
          b.ue();
          if (b.ue() % 5 == 1) throw new IllegalArgumentException("B slices unsupported");
        }
        if (type == 7
                && !Arrays.equals(
                    f.avcc, nal.offset, nal.offset + nal.length, video.sps, 0, video.sps.length)
            || type == 8
                && !Arrays.equals(
                    f.avcc, nal.offset, nal.offset + nal.length, video.pps, 0, video.pps.length))
          throw new IllegalArgumentException("parameter sets changed");
      }
      byte[] mfhd = full("mfhd", 0, 0, u32(sequence++));
      byte[] tfhd = full("tfhd", 0, 0x020000, u32(1));
      byte[] tfdt = full("tfdt", 1, 0, ByteBuffer.allocate(8).putLong(ts).array());
      byte[] trun =
          full(
              "trun",
              0,
              0x701,
              u32(1, 0, f.duration, f.avcc.length, f.key ? 0x02000000 : 0x01010000));
      byte[] moof = box("moof", mfhd, box("traf", tfhd, tfdt, trun));
      ByteBuffer.wrap(moof)
          .putInt(8 + mfhd.length + 8 + tfhd.length + tfdt.length + 16, moof.length + 8);
      previous = ts;
      return join(prefix, moof, u32(f.avcc.length + 8), ascii("mdat"), f.avcc);
    }

    byte[] envelope() {
      byte[] xml =
          ("<metadata><mimetypecodec>video/mp4; codecs=\""
                  + video.codec()
                  + "\"</mimetypecodec></metadata>")
              .getBytes(StandardCharsets.UTF_16LE);
      byte[] h = new byte[20];
      h[0] = 1;
      h[1] = 8;
      ByteBuffer.wrap(h).order(ByteOrder.LITTLE_ENDIAN).putShort(18, (short) xml.length);
      return join(h, xml, init);
    }

    byte[] media(Frame f, long ts) {
      byte[] h = new byte[10];
      h[0] = 2;
      ByteBuffer.wrap(h)
          .order(ByteOrder.LITTLE_ENDIAN)
          .putInt(6, (int) Math.min(Integer.MAX_VALUE, ts / 90));
      return fragment(f, ts, h);
    }
  }
}
