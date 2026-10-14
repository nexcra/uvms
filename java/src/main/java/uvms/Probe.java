package uvms;

/** Metadata-only camera input check; never saves private video. */
public final class Probe {
  public static void main(String[] args) throws Exception {
    try (var input = new Rtsp(System.getenv("REAL_CAMERA_RTSP_URL"))) {
      System.out.println("Video " + input.video.width() + "x" + input.video.height());
      for (int i = 0; i < 30; i++) {
        var f = input.next();
        System.out.println(
            "frame="
                + i
                + " timestamp="
                + f.timestamp()
                + " key="
                + f.key()
                + " bytes="
                + f.avcc().length);
      }
    }
  }
}
