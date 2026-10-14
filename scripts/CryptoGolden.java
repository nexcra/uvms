package uvms;

import com.google.gson.GsonBuilder;
import java.nio.ByteBuffer;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.MessageDigest;
import java.util.ArrayList;
import java.util.HexFormat;
import java.util.LinkedHashMap;

/** Runs against the preserved baseline JAR, never against current target/classes. */
public final class CryptoGolden {
  public static void main(String[] args) throws Exception {
    var hex = HexFormat.of();
    byte[] key = hex.parseHex("e1f97a0d3e018be0d64fa32c06de4139");
    byte[] salt = hex.parseHex("0ec675ad498afeebb6960b3aabe6");
    var sender = new Rtc.Srtp(key, salt);
    var cases = new ArrayList<Object>();
    int ordinal = 0;
    for (int sequence : new int[] {65535, 0, 1}) {
      byte[] packet = new byte[44];
      ByteBuffer.wrap(packet)
          .put((byte) 0x80)
          .put((byte) 0xe0)
          .putShort((short) sequence)
          .putInt(90000 + 9000 * ordinal)
          .putInt(0x10203040);
      for (int i = 12; i < packet.length; i++) packet[i] = (byte) (i * 7 + ordinal);
      byte[] rtcp = hex.parseHex("80c80006102030400000000000000000000000000000000000000000");
      ByteBuffer.wrap(rtcp).putInt(16, 90000 + 9000 * ordinal).putInt(20, ordinal + 1);
      var one = new LinkedHashMap<String, Object>();
      one.put("sequence", sequence);
      one.put("rtp_plain", hex.formatHex(packet));
      one.put("rtp_protected", hex.formatHex(sender.rtp(packet)));
      one.put("rtcp_plain", hex.formatHex(rtcp));
      one.put("rtcp_protected", hex.formatHex(sender.rtcp(rtcp)));
      cases.add(one);
      ordinal++;
    }
    var result = new LinkedHashMap<String, Object>();
    result.put(
        "provenance",
        "Preserved Java baseline JAR; regression oracle, not an independent RFC vector");
    result.put(
        "baseline_sha256",
        hex.formatHex(
            MessageDigest.getInstance("SHA-256").digest(Files.readAllBytes(Path.of(args[0])))));
    result.put("key", hex.formatHex(key));
    result.put("salt", hex.formatHex(salt));
    result.put("cases", cases);
    Files.writeString(
        Path.of(args[1]), new GsonBuilder().setPrettyPrinting().create().toJson(result) + "\n");
  }
}
