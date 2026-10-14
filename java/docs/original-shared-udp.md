# Original shared UDP relay: recovered implementation

Date: 2026-10-14. Static REA/Ghidra analysis of the supplied MSI's binaries. This report does not claim that the original server executable was run, nor that Java duplicates every native instruction.

## Target and investigation boundary

- `UMediaServer.exe` SHA-256 `19fdb1d0888b38a7b5886a57cce774d9ddf0abc7fe02a62ee6e3673ca8b2a1e8`.
- `UnrealWebRTC.dll` SHA-256 `3a283df31ced9fdca9cf02d367213b0be0bf01394744d56ff85f9ce627a75287`.
- REA 6.4.0, Ghidra 12.1.4; default auto-analysis complete before queries. Full tool responses, source-bound evidence and readable pseudocode are in `evidence/native/udp-engine/`. EXE focused queries reused the concurrently owned `buffer-engine` session; their 111–121 records were copied unchanged here.
- Prior dispatcher evidence: `evidence/native/server-v2/010-route-func.c`, `ev_35c437ebbfafb3a5cdc77c7fc3e801783981448398282bd6d8b11cec1b4c01eb`.
- Native addresses and field offsets below are recovered observations; descriptive field names are analyst labels. Windows API constants are decoded from the observed arguments. Decompiled call signatures have lost arguments in several places; do not treat them as original C++ signatures.

## Findings and checklist

| Question | Status | Observation / boundary |
|---|---|---|
| Does singleport mean one OS socket? | Answered statically | No. Initialization creates 16 workers and 16 UDP sockets bound to the same configured public address/port, sharing one IOCP. |
| Is the original WebRTC DLL itself the global public-port multiplexer? | Answered for traced path | EXE rewrites its candidate and relays packets to the DLL's original candidate endpoint. Each admitted peer gains a connected internal UDP relay socket. |
| How is the first packet assigned? | Answered statically | Pending local ufrag records; the relay scans for STUN cookie and `localUfrag:` byte sequence, chooses the associated internal sockaddr, then consumes the pending record. |
| What is the established-session fast path? | Answered statically | Map keyed by source IPv4 + source port, returning the connected internal relay object; raw WSABUF forwarding. |
| How are buffers/workers shared? | Answered for relay path | Fixed 16 IOCP workers; one reusable 64 KiB input buffer per socket context. No decoded-video work occurs in traced forwarding path. |
| How are stale entries removed? | Partially answered | Periodic sweep checks global tick differences: established directions >=11, pending >=6; closes stale sockets and removes mapping/pending records. Tick period was not established in this pass. |
| Does relay admission authenticate STUN? | Bounded negative observation | No MESSAGE-INTEGRITY verification or structured USERNAME-attribute parser is visible in the complete recovered helper `0x140021800`; downstream libwebrtc still receives raw STUN. This does not establish overall authentication absence. |
| Does Java match original throughput or cleanup exactly? | Unresolved by static analysis | Requires Java tests/measurement; original runtime throughput and packet scheduling were not measured in this pass. |

## Signaling and socket ownership

Dispatcher `0x1400673c0` accepts `singleport/` or `randomport/`, retaining the distinction in `local_310`. After the PlayingPeer InitClient returns answer/candidate, the singleport branch (when global `DAT_1403f5c20` exists):

1. Calls `0x140079970` to save the original host candidate IP/port and replace candidate endpoint with configured public IP/port. The helper explicitly searches `typ host`, parses the original port, and rebuilds the candidate string using `%s %d` (`ev_164ed77394748699990ede6e11132ba57fcb5ecc8f6c5c7a7f2baede414caee8`).
2. Calls `0x14007a5f0` to extract `ufrag ` from candidate, falling back to `a=ice-ufrag:` from JSON-escaped SDP (`ev_a914af7811a557c2cbb60b6c00482a6cc754cdd1e142908800ef7111f3f3b4fc`).
3. Builds a 0x30-byte pending record: local ufrag string +0, original sockaddr +8, stream label +0x18, original peer pointer +0x20, publish/play flag +0x28. `0x140021640` adds it to a locked pending list; publishing records also reject duplicate stream labels (`ev_aaf4be0d9f9487b38d481e688fd606a8422e39363bfe7dfe600cd659d84e68e0`).

`0x140020040` initializes one IOCP and loops **16** times, starting `0x140020d50` (wrapper for worker `0x140020d60`) and calling UDP socket helper `0x140020560` with the same input address and port. Worker handles occupy owner +8..+0x80; listener sockets occupy owner +0x208..+0x280. Evidence `ev_a6b39f3db73a8be64902c002cbc23300e14266f8ffb4b464618e808ddc623f01`.

Socket helper `0x140020560` calls `WSASocketW(AF_INET, SOCK_DGRAM, ..., WSA_FLAG_OVERLAPPED)`, sets `SO_REUSEADDR=1`, requests `SO_RCVBUF=1,048,576` bytes with 524,288 then 262,144 fallbacks, sets `SO_SNDBUF=65,536`, binds the configured endpoint, tags the context type 0xC, attaches IOCP, and posts its receive. Evidence `ev_5874f3a74322efa0900d7f0ca94e6ecc91a09aaeca13b5d28dad1897e0e43866`. These are requested socket buffers, not measured OS-effective capacities. The exact Windows distribution of packets among same-port sockets was not measured.

## First-packet and established-packet routing

`0x140020a30` holds owner +0x300 critical section and builds the key `%u.%u.%u.%u:%u` from source address and port. It looks up owner +0x288's map. A hit directly returns the established relay; a miss calls `0x1400208a0` and inserts its returned object under that key. Evidence `ev_bc4c2366ea5f3f0ed5429b37a849f022b9a533799fac4e0403c9fba1b8a27a95`. The ordered-node traversal indicates a tree map, not proven O(1) hashing. Its decimal port value is taken from the raw sockaddr field; consistency matters for lookup, not display byte order.

Admission helper `0x140021800` scans the supplied bounded packet for the four STUN cookie bytes `21 12 A4 42`, then iterates pending entries, searching remaining packet bytes for each entry's `ufrag + ':'`. On match it copies the saved internal sockaddr to output, handles publishing start where relevant, destroys that pending record, and removes its list node. Evidence `ev_23e5d3f17af1496211b77ed9d6e59761eb6d6bc8059b7f6e821ebbe35876e6b6`. It is a relay classifier; no DTLS or SRTP terminator was observed here.

`0x1400208a0` creates a new UDP socket, requests 64 KiB receive/send buffers, binds the internal local IP with port zero, connects to the saved original candidate endpoint, stores the public listener object as +0x2B8, stores the browser's sockaddr at +0x2A0, attaches IOCP and posts receive. Evidence `ev_0e521f9ff4a6c9446fea4004c0e8d74153910a3fa991440042fbb70641b659ce`.

The worker `0x140020d60` blocks on `GetQueuedCompletionStatus`. In the public→internal direction it calls the endpoint lookup and forwards the same input WSABUF with `WSASendTo` on the connected internal socket. In internal→public direction it selects the saved public socket and saved browser sockaddr, forwards that WSABUF, then reposts receive. Evidence `ev_8fa1feeddc31439c1e289a6fb56c9f84ab542b8bdc196fd4d28537675fbca116`. This observed path does not decode or re-encode video and does not parse RTP NAL units. Kernel network transfers still occur; this is not an end-to-end zero-copy claim.

`0x140020440` allocates/zeros a 64 KiB buffer once when attaching a context to IOCP, sets WSABUF length to 64 KiB and uses the context pointer as IOCP completion key (evidence `ev_3f86edffd305e3c65be196aefaf38e1a678443f8edfa744b9032dcc71c2aac96`). `0x1400204d0` reuses this buffer in `WSARecvFrom`, retaining source sockaddr on public sockets and omitting it for connected internal sockets (`ev_2266b824b5bb010a921aa7539dc3de4c9e8742cc11f1cf7a5375db14c725d64d`). Exact overlapped send/receive buffer ownership under all completion orders remains untested.

## Cleanup and engine sharing

- `0x140021190` sweeps established endpoint map and pending list under owner lock. It expires established entries when either of two activity tick differences reaches 11; pending entries at 6. It calls socket close/cleanup and erases map/list nodes. Evidence `ev_49795d2f7c17e430e0bdb0aafc34309fb2992285da1a1a3100a00d73cb34e309`. Tick duration was not proved, so these constants are not reported as seconds.
- `0x1400202d0` locks a per-context critical section, changes socket to INVALID_SOCKET, shutdowns/closes it once, updates activity tick and queues the context for deferred cleanup (`ev_39302bfac731ebf782451be4982f69f2481a1cabf6aff7540392056a204ebf9e`). Final deferred destruction timing was not traced here.
- `0x1400201f0` closes 16 listener sockets, posts 16 IOCP wakeups, waits for 16 worker handles up to 2,000 ms, closes thread handles, deletes the owner critical section (`ev_2eb050f762bef0616f303dfcc984ff2a4652471a6bbe592e29fe62ffff8783dd`). Process-wide IOCP handle destruction was not established by this function alone.
- Separately, DLL `0x18009e430` shares a PeerConnection factory group until its client count reaches **10**, then creates another group with three thread objects and a factory. `0x1800a0af0` closes the peer and decrements that group count. Prior evidence `ev_85774eb6b5a8ffccfc491b58639603f47a0e11d5e1ad13b562026d6a28e72e86` and `ev_ed80077e2809ad7816a351be94392da6c1decb2bcd4cace7194846fc08845345`. Object types/thread roles need their constructor/vtable corroboration before naming network/worker/signaling individually.

## Evidence-backed Java adaptation boundary

A suitable Java port preserves the public one-port contract, separates first-packet local-ufrag admission from established source-endpoint routing, reuses a service-owned receive engine and buffers, and removes per-session ownership of the public socket. Because Java terminates ICE/DTLS/SRTP itself, it can send directly to peers and omit the EXE→DLL internal relay socket/hop.

This is an architectural adaptation of recovered behavior, not a literal copy of the original Windows engine. A Java concurrent map instead of the observed locked tree, one or a few receiver threads instead of sixteen Windows IOCP workers, and authenticated STUN admission are implementation choices. In particular Java must validate STUN MESSAGE-INTEGRITY using the peer's credential before associating or changing a remote endpoint. The relay's raw substring classifier should not replace Java's existing ICE authentication. Receiving unauthenticated packets or closing one peer must not transfer another peer's address or close the service-owned listener.

## Session lifecycle

The initial standalone DLL search session completed and switched to EXE before the queue consolidation raced with the driver. The driver's ENOENT at request rename terminated it through its `finally { client.close(); }` cleanup; original binaries were not executed or modified. Redundant partial evidence is retained, and stale close/exit requests are marked `.cancelled.json`. The shared EXE session completed snapshot saving and `close_binary` successfully, and its driver exited normally after both investigations finished.

