package uvms;

import java.io.IOException;
import java.net.*;
import java.nio.ByteBuffer;
import java.nio.channels.DatagramChannel;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.concurrent.ConcurrentHashMap;

/**
 * One public port with a fixed service-owned socket pool. STUN selects a peer by local ufrag;
 * authenticated ICE establishes endpoint ownership for subsequent DTLS/SRTCP packets.
 */
final class SharedUdp implements AutoCloseable {
  interface Peer {
    String localUfrag();

    void receive(byte[] packet, InetSocketAddress sender) throws Exception;
  }

  final DatagramSocket socket;
  final List<DatagramSocket> sockets = new ArrayList<>();
  final ConcurrentHashMap<Peer, DatagramSocket> senders = new ConcurrentHashMap<>();
  final ConcurrentHashMap<String, Peer> peers = new ConcurrentHashMap<>();
  final ConcurrentHashMap<InetSocketAddress, Peer> endpoints = new ConcurrentHashMap<>();
  private boolean closed;
  private int nextSender;

  SharedUdp(int port) throws IOException {
    this(port, 1);
  }

  SharedUdp(int port, int workers) throws IOException {
    try {
      for (int i = 0; i < workers; i++) {
        DatagramChannel channel = DatagramChannel.open(StandardProtocolFamily.INET);
        DatagramSocket receiver = channel.socket();
        sockets.add(receiver);
        if (channel.supportedOptions().contains(StandardSocketOptions.SO_REUSEPORT)) {
          channel.setOption(StandardSocketOptions.SO_REUSEPORT, true);
        } else workers = 1;
        receiver.setReceiveBufferSize(1024 * 1024);
        receiver.setSendBufferSize(65536);
        receiver.bind(new InetSocketAddress(port));
        if (port == 0) port = receiver.getLocalPort();
      }
    } catch (IOException | RuntimeException e) {
      for (var receiver : sockets) receiver.close();
      throw e;
    }
    socket = sockets.getFirst();
    for (int i = 0; i < sockets.size(); i++) {
      DatagramSocket receiver = sockets.get(i);
      Thread.ofPlatform().name("uvms-shared-udp-" + i).daemon().start(() -> receive(receiver));
    }
  }

  synchronized void register(Peer peer) throws IOException {
    if (closed || peers.putIfAbsent(peer.localUfrag(), peer) != null)
      throw new IOException("ICE registration failed");
    senders.put(peer, sockets.get(Math.floorMod(nextSender++, sockets.size())));
  }

  // Called only after this peer has checked the complete ICE username and HMAC.
  synchronized boolean authenticated(Peer peer, InetSocketAddress address) {
    if (closed || peers.get(peer.localUfrag()) != peer) return false;
    Peer owner = endpoints.get(address);
    if (owner != null && owner != peer) return false;
    endpoints.entrySet().removeIf(e -> e.getValue() == peer && !e.getKey().equals(address));
    endpoints.put(address, peer);
    return true;
  }

  synchronized void unregister(Peer peer) {
    peers.remove(peer.localUfrag(), peer);
    senders.remove(peer);
    endpoints.entrySet().removeIf(e -> e.getValue() == peer);
  }

  static boolean isStun(byte[] b) {
    return b.length >= 20 && (b[0] & 0xc0) == 0 && ByteBuffer.wrap(b).getInt(4) == 0x2112a442;
  }

  static String localUsername(byte[] b) {
    if (!isStun(b)) return null;
    ByteBuffer data = ByteBuffer.wrap(b);
    if (data.getShort(0) != 1 || Short.toUnsignedInt(data.getShort(2)) + 20 != b.length)
      return null;
    String result = null;
    for (int at = 20; at + 4 <= b.length; ) {
      int type = Short.toUnsignedInt(data.getShort(at));
      int n = Short.toUnsignedInt(data.getShort(at + 2));
      if (n > b.length - at - 4) return null;
      if (type == 8) break; // Never use an unauthenticated attribute following integrity.
      if (type == 6) {
        if (result != null) return null;
        String name = new String(b, at + 4, n, StandardCharsets.US_ASCII);
        int colon = name.indexOf(':');
        if (colon <= 0) return null;
        result = name.substring(0, colon);
      }
      at += 4 + ((n + 3) & ~3);
    }
    return result;
  }

  void dispatch(byte[] b, InetSocketAddress sender) throws Exception {
    Peer peer;
    if (isStun(b)) {
      String name = localUsername(b);
      peer = name == null ? null : peers.get(name);
    } else {
      peer = endpoints.get(sender);
    }
    if (peer != null) peer.receive(b, sender);
  }

  private void receive(DatagramSocket receiver) {
    byte[] buffer = new byte[65535];
    DatagramPacket packet = new DatagramPacket(buffer, buffer.length);
    while (!receiver.isClosed()) {
      try {
        packet.setLength(buffer.length);
        receiver.receive(packet);
        dispatch(
            Arrays.copyOf(buffer, packet.getLength()),
            (InetSocketAddress) packet.getSocketAddress());
      } catch (Exception ignored) {
        // A malformed peer datagram must not terminate other sessions.
      }
    }
  }

  void send(Peer peer, byte[] b, InetSocketAddress address) throws IOException {
    DatagramSocket sender = senders.get(peer);
    if (sender == null) throw new IOException("ICE closed");
    sender.send(new DatagramPacket(b, b.length, address));
  }

  int port() {
    return socket.getLocalPort();
  }

  public synchronized void close() {
    closed = true;
    for (var receiver : sockets) receiver.close();
    peers.clear();
    senders.clear();
    endpoints.clear();
  }
}
