# Java dependencies

Runtime dependencies are pinned in `pom.xml` and bundled in the runnable JAR:

| Component | Version | License | Purpose |
|---|---|---|---|
| Netty codec-http and its transitive modules | 4.1.137.Final | Apache-2.0 | HTTP, WebSocket, NIO transport |
| Bouncy Castle bctls | 1.86.2 | Bouncy Castle license (MIT style) | DTLS 1.2 |
| Bouncy Castle bcpkix, bcprov, bcutil | 1.86 | Bouncy Castle license (MIT style) | Certificates and TLS crypto |
| Gson | 2.14.0 | Apache-2.0 | JSON |
| Error Prone annotations (Gson dependency) | 2.48.0 | Apache-2.0 | Compile/runtime annotations |

JUnit is a test-only dependency. JDK 21 is installed separately, not bundled. The service does not bundle FFmpeg, MediaMTX, Unreal binaries, certificates, private keys or SDK scripts. Runtime dependency license and notice files are copied from the resolved dependency JARs and the exact upstream release tags into `licenses/`. Upstream source URLs and hashes are recorded in `sources.json`.

Protocol/implementation references: [RFC 3711](https://www.rfc-editor.org/rfc/rfc3711), [RFC 5764](https://www.rfc-editor.org/rfc/rfc5764), [Netty API](https://netty.io/4.1/api/), [Bouncy Castle TLS API](https://downloads.bouncycastle.org/java/docs/bctls-jdk18on-javadoc/).
