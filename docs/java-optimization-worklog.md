# Original engine investigation and Java implementation

Scope: shared WebRTC UDP port and compressed-frame hot path. Preserve camera authentication only, native Java runtime, and existing Linux deployment.

## Checklist

- [x] Preserve previous Java JAR and source under `test-results/java-optimization/before`.
- [x] Retain before-change 16-viewer performance samples on the user-selected synthetic fixture; exclude interrupted physical-camera run.
- [x] Trace original singleport socket ownership, session demultiplexing and cleanup.
- [x] Trace original encoded frame / NAL index ownership and copies.
- [x] Implement confirmed principles with explicit Java-specific adaptations.
- [x] Verify shared-port isolation, peer cleanup, reconnect, two camera aliases, and media integrity.
- [x] Run identical after-change measurements, deploy and package verified build. Runtime/source archives and manifest are in `dist/`; deployed SHA matches the tested JAR.

## Finding ledger

| Question | Evidence / coverage | Status |
|---|---|---|
| Baseline Java UDP ownership | `Rtc` constructor bound one socket per viewer | Replaced with service-owned fixed socket pool on one public port |
| Baseline Java payload copies | NAL, FU, RTP and SRTP intermediate arrays | Shared NAL index, direct RTP/SRTP output, final-size MSE allocation |
| Original singleport implementation | `original-shared-udp.md` | Static observations: 16 same-port sockets, IOCP, ufrag admission, source endpoint fast path and internal relay |
| Original frame ownership and packetization | `original-buffer-optimizations.md` | Static observations: refcounted sample queue; normalize once per consumer traversal; explicit copy boundaries |
| Single socket Java experiment | `fixture-single-socket/` | CPU increased in the short sample. Final Linux fixed pool was measured separately; contention explanation remains a hypothesis |
| Final performance | `summary.json`, `java-optimization.md` | Fixture WebRTC CPU12.31→9.07%, MSE9.58→7.27%; not an original-server throughput comparison |

Static observations do not establish runtime performance. Java changes and measured effects are reported separately from claims about original code. Original binaries and camera credentials are excluded from deliverable packages.
