package uvms;

import static org.junit.jupiter.api.Assertions.*;
import static uvms.Media.*;

import java.net.*;
import java.nio.ByteBuffer;
import java.util.*;
import org.junit.jupiter.api.Test;

class SharedUdpTest {
  @Test
  void fixedPoolUsesOnePortForEverySender() throws Exception {
    try (SharedUdp udp = new SharedUdp(0, 4);
        DatagramSocket destination = new DatagramSocket(0, InetAddress.getLoopbackAddress())) {
      destination.setSoTimeout(2000);
      int sockets = udp.sockets.size();
      assertTrue(sockets == 4 || sockets == 1); // platforms without SO_REUSEPORT fall back
      for (int i = 0; i < 32; i++) {
        final String name = "peer" + i;
        var peer =
            new SharedUdp.Peer() {
              public String localUfrag() {
                return name;
              }

              public void receive(byte[] b, InetSocketAddress sender) {}
            };
        udp.register(peer);
        udp.send(
            peer, new byte[] {(byte) i}, (InetSocketAddress) destination.getLocalSocketAddress());
        var packet = new DatagramPacket(new byte[100], 100);
        destination.receive(packet);
        assertEquals(udp.port(), packet.getPort());
        assertEquals(i, packet.getData()[0]);
        assertEquals(sockets, udp.sockets.size());
      }
      assertEquals(32, udp.peers.size());
    }
  }

  static byte[] request(String username, String password) throws Exception {
    byte[] value = ascii(username);
    byte[] attr = join(u16(6, value.length), Arrays.copyOf(value, (value.length + 3) & ~3));
    byte[] header = join(u16(1, attr.length + 24), u32(0x2112a442), new byte[12]);
    byte[] prefix = join(header, attr);
    return join(prefix, u16(8, 20), Rtc.mac("HmacSHA1", ascii(password), prefix));
  }

  @Test
  void authenticatedRoutingRebindingAndPeerRemoval() throws Exception {
    try (SharedUdp udp = new SharedUdp(0)) {
      class Peer implements SharedUdp.Peer {
        final String name;
        int received;

        Peer(String name) {
          this.name = name;
        }

        public String localUfrag() {
          return name;
        }

        public void receive(byte[] b, InetSocketAddress sender) throws Exception {
          if (SharedUdp.isStun(b)) {
            if (Rtc.validStun(b, name + ":browser", "password-" + name))
              udp.authenticated(this, sender);
          } else received++;
        }
      }
      Peer a = new Peer("a"), b = new Peer("b");
      udp.register(a);
      udp.register(b);
      var addrA = new InetSocketAddress("127.0.0.1", 10000);
      var addrB = new InetSocketAddress("127.0.0.1", 10001);
      var newA = new InetSocketAddress("127.0.0.1", 10002);
      byte[] dtls = {22, -2, -3};
      udp.dispatch(dtls, addrA);
      assertEquals(0, a.received);
      udp.dispatch(request("a:browser", "wrong"), addrA);
      assertTrue(udp.endpoints.isEmpty());
      udp.dispatch(request("a:browser", "password-a"), addrA);
      udp.dispatch(request("b:browser", "password-b"), addrB);
      udp.dispatch(dtls, addrA);
      udp.dispatch(dtls, addrB);
      assertEquals(1, a.received);
      assertEquals(1, b.received);
      // Another valid peer must not steal a live endpoint.
      udp.dispatch(request("b:browser", "password-b"), addrA);
      assertSame(a, udp.endpoints.get(addrA));
      udp.dispatch(request("a:browser", "password-a"), newA);
      udp.dispatch(dtls, addrA);
      udp.dispatch(dtls, newA);
      assertEquals(2, a.received);
      udp.unregister(a);
      udp.dispatch(dtls, newA);
      udp.dispatch(dtls, addrB);
      assertEquals(2, a.received);
      assertEquals(2, b.received);
      assertFalse(udp.socket.isClosed());
      assertEquals(1, udp.peers.size());
      assertFalse(udp.authenticated(a, newA));
    }
  }

  @Test
  void malformedStunCannotChooseAnEndpoint() throws Exception {
    byte[] good = request("local:remote", "secret");
    assertEquals("local", SharedUdp.localUsername(good));
    assertTrue(Rtc.validStun(good, "local:remote", "secret"));
    assertFalse(Rtc.validStun(good, "local:other", "secret"));
    for (int n = 0; n < good.length; n++) {
      byte[] truncated = Arrays.copyOf(good, n);
      assertNull(SharedUdp.localUsername(truncated));
      assertFalse(Rtc.validStun(truncated, "local:remote", "secret"));
    }
    byte[] altered = good.clone();
    altered[altered.length - 1] ^= 1;
    assertFalse(Rtc.validStun(altered, "local:remote", "secret"));
    ByteBuffer.wrap(altered).putShort(22, (short) 65535);
    assertNull(SharedUdp.localUsername(altered));
  }
}
