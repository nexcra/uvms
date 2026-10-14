package uvms;

import static org.junit.jupiter.api.Assertions.*;

import java.nio.ByteBuffer;
import java.util.HexFormat;
import org.junit.jupiter.api.Test;

/** Byte regression against the preserved baseline JAR, not an independent RFC oracle. */
class SrtpGoldenTest {
  // Baseline SHA-256: 171b2a3a25cf025e79654d8c0ec1aa48577701c561e14eb85c9770ae612595e2.
  // Reproducer: scripts/CryptoGolden.java; retained output: crypto-golden.json.
  static final String[] RTP = {
    "80e0ffff00015f9010203040f5e65e80cfce80b1823b603a8c50bf4b94e766da5b12998e69ea0b5d3710c36caab23a738bd0e2ed8ca6",
    "80e00000000182b810203040b0dd2c5ad690a7b8d3dd084a615ae8b48c1449d8567a393d6d45718a1b2f977c8e66f4ac1eb8b7e492ac",
    "80e000010001a5e010203040ae1e2eba2e0d9c3aac5c33ed4e3d1ba66d4583271ba7df5618cb757b42070eb8f10cc18edc6f64ac2a5d"
  };
  static final String[] RTCP = {
    "80c8000610203040e46dd335ad6e95c4d104efe6eb69ca428e76e68980000000faaf9330c0bd9b8e5130",
    "80c8000610203040ce67ea8196371863e4b4c45c2e92be37f3e23485800000018fedb2e529a046d68786",
    "80c80006102030405035fee785cfa6725cf5ba3b6486adb2dac55477800000023156c468b54fc1265a0f"
  };

  @Test
  void originalCiphertextAcrossRtpRolloverAndRtcpIndices() throws Exception {
    var hex = HexFormat.of();
    var sender =
        new Rtc.Srtp(
            hex.parseHex("e1f97a0d3e018be0d64fa32c06de4139"),
            hex.parseHex("0ec675ad498afeebb6960b3aabe6"));
    int[] sequences = {65535, 0, 1};
    for (int ordinal = 0; ordinal < sequences.length; ordinal++) {
      byte[] packet = new byte[44];
      ByteBuffer.wrap(packet)
          .put((byte) 0x80)
          .put((byte) 0xe0)
          .putShort((short) sequences[ordinal])
          .putInt(90000 + 9000 * ordinal)
          .putInt(0x10203040);
      for (int i = 12; i < packet.length; i++) packet[i] = (byte) (i * 7 + ordinal);
      byte[] original = packet.clone();
      assertArrayEquals(
          hex.parseHex(RTP[ordinal]), sender.rtp(packet), "RTP sequence " + sequences[ordinal]);
      assertArrayEquals(original, packet, "RTP source buffer must remain unchanged");

      byte[] report = hex.parseHex("80c80006102030400000000000000000000000000000000000000000");
      ByteBuffer.wrap(report).putInt(16, 90000 + 9000 * ordinal).putInt(20, ordinal + 1);
      original = report.clone();
      assertArrayEquals(hex.parseHex(RTCP[ordinal]), sender.rtcp(report), "SRTCP index " + ordinal);
      assertArrayEquals(original, report, "RTCP source buffer must remain unchanged");
    }
    assertEquals(1, sender.rollover);
    assertEquals(3, sender.rtcpIndex);
  }
}
