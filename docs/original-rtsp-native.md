# Original URTSPClient.dll: native trace and bounded execution

Analyzed on 2026-10-13. This report describes the supplied original DLL, not the independently written Rust/FFmpeg prototype. The target is `evidence/extracted/URTSPClient.dll`, SHA-256 `056726f883b96982bc303ae84ce9b3cb8d4c7c3640e82d42ef96bb08976ad148` (509,168 bytes, x64 PE). Ghidra 12.1.4 through REA 6.4.0 completed native auto-analysis. Full REA records, anonymous-function pseudocode and an Evidence-ID index are retained in [rtsp-v2](../evidence/native/rtsp-v2/index.json). `.c` files there are Ghidra pseudocode, not recovered original source.

## Established result

The original client receives RTSP media through an embedded LIVE555-derived stack, depacketizes H.264 RTP into NAL units, assembles same-timestamp NALs into AVCC access units, and exposes them to the server through a COM frame-provider interface. This path does not require decoding/re-encoding the H.264 picture. The actual DLL was exercised on a synthetic local RTSP stream: 40 video samples were retrieved and all 40 parsed completely as AVCC length-prefixed NAL units. This is considerably stronger evidence than the earlier imports/strings inventory.

The string `LIVE555 Streaming Media v` followed by `2020.03.06` is present at `0x180055ac8` and the surrounding bytes. C++ RTTI identifies `RTSPClient`, `MediaSubsession`, `H264VideoRTPSource`, `H264BufferedPacket`, and the proprietary wrappers `CRTSPClient`, `CURTSPClient`, `CUMediaSink`. Recovered request constructors and H.264 parsing corroborate the stack attribution. The version is an embedded version string; it does not establish that all upstream code is unmodified. Evidence: `062-live-version`, `026-names`, `015-pseudo`, `032-pseudo`.

## Native call/data chain

All addresses below belong to this exact DLL. Descriptive function names are analyst labels; no original function symbols are claimed.

| Stage | Native address | Observed behavior | Evidence file |
|---|---|---|---|
| Frame-provider init | `0x1800049d0` | Parses wide string options, invokes client init, returns two 88-byte media types and four DWORD status values | `035-pseudo` |
| Client init | `0x1800064d0` | Creates scheduler/environment/authenticator, starts OPTIONS/DESCRIBE sequence, runs event loop; has a media-type detector fallback if SDP lacks usable video parameters | `039-pseudo` |
| OPTIONS | `0x180006c70` → `0x180016600` | Schedules 15,000,000-unit delayed timeout and creates `OPTIONS` RequestRecord | `048-pseudo`, `052-pseudo` |
| OPTIONS response | `0x180006f70` | Inspects `GET_PARAMETER` support then issues DESCRIBE | `053-pseudo` |
| DESCRIBE | `0x180006cf0` → `0x1800164e0` | Creates `DESCRIBE` RequestRecord with completion callback `0x180007030` | `022-pseudo`, `015-pseudo` |
| SDP/subsessions | `0x180007030` → vtable callback `0x1800073b0` | Builds session, iterates subsessions, initiates sources and dispatches codec setup | `025-describe-response`, `030-vtable`, `034-pseudo` |
| Video selection | `0x180007880` | H264/H265/MP4V-ES branches; audio AAC/LATM/Opus/PCMU/PCMA also exists | `021-pseudo` |
| H.264 format | `0x180007a80` | Reads `sprop-parameter-sets`, base64-decoded NALs; requires SPS(type7) and PPS(type8), parses SPS dimensions/profile/level, creates AVC1 MPEG2VIDEOINFO-like format | `013-pseudo` |
| Pull begins streaming | `0x180004e80` → `0x180006840` → `0x1800074d0` | First pull starts a worker thread, waits for initialization event; subsequent pulls drain the sample queue | `037-pseudo`, `040-pseudo`, `046-pseudo` |
| SETUP then PLAY | `0x180007640` → `0x180006d70` / `0x180006e20` | Iterates selected subsessions, sends SETUP via `0x1800166b0`, constructs sinks, starts reading, sends PLAY via `0x180016830` | `059-pseudo`, `078-flow`–`086-flow` |
| Sink setup | `0x180008da0` | Chooses H264=1/H265=2/MP4V=3 sink mode, creates `CUMediaSink`, sets socket receive buffer | `080-flow` |
| H.264 RTP header | `0x18001e810` | Handles single NAL, aggregation and fragmented NAL payload headers, reconstructs FU start NAL header and marks frame start/end flags | `032-pseudo` |
| Aggregation elements | `0x18001ea40` | Reads big-endian 16-bit NAL lengths; advances by STAP/MTAP element headers | `033-pseudo` |
| Asynchronous read | `0x18000e1d0` | Installs callback `0x18000df40` into the `FramedSource`, requests next frame | `042-pseudo` |
| Timestamp/keyframe | `0x18000df40` → `0x18000df60` | Extends RTP timestamp across wrap, scales by 10,000,000 / clock rate, combines presentation timeval to 100ns, detects keyframe, submits payload | `045-pseudo`, `050-pseudo` |
| H264 keyframe detector | `0x18000cd20` | IDR and additional slice/SEI parsing; not merely `NAL type == 5` | `056-pseudo` |
| Sample/queue | `0x18000a000` → `0x18000ff30` → `0x18000eb80` | Synchronization/initial-keyframe gates, timestamp-based access-unit accumulation, queue insertion | `054-pseudo`, `058-pseudo`, `060-pseudo` |
| AVCC NAL append | `0x1800100b0` | Writes 4-byte big-endian NAL length then NAL bytes for H264 | `066-abi` |
| Keyframe config | `0x180010250` | Adds SPS/PPS from the media format when both were not already observed in the current accumulator | `067-abi` |
| Frame delivery | `0x18000ef60` | Locks queue, removes next sample, returns its owned pointer to caller | `047-pseudo` |
| Stop | `0x180004e50` → `0x1800068e0` | Stops event loop, joins/closes thread, releases media/queue storage | `036-pseudo`, `077-abi` |

The source-to-sink callback is internal to the DLL. The server-facing interface pulls queued samples; it is not a direct frame callback to the server.

## Recovered COM boundary, corroborated by execution

CLSID `{022F2113-B1BD-4326-9CC7-DDF80ABD245C}` comes from the original DLL's registry resource. Frame-provider IID `{7C5E98AF-E07E-4A9C-B508-C29039CF8BA1}` is observed at server address `0x1401544c0` and successfully used with the original DLL class factory. The server creates `Unreal.URTSPClient` in `FUN_140023a60`; its `+0x18` invocation passes `server+0x120` for two media types and `server+0x1d0` for information. See the separate server investigation.

First interface vtable at `0x1800549a8`:

| x64 vtable offset | ABI inferred from pseudocode and verified by harness |
|---|---|
| `+0x00/+0x08/+0x10` | IUnknown QueryInterface/AddRef/Release |
| `+0x18` | `HRESULT Init(this, wchar_t *options, AM_MEDIA_TYPE media[2], uint32_t info[4])` |
| `+0x20` | `HRESULT Stop(this, void *nonNullArgument)`; observed wrapper only tests argument for null |
| `+0x28` | `HRESULT GetSample(this, uintptr_t unusedArgument, Sample **out)`; S_OK yields sample, S_FALSE means no sample presently |
| `+0x30` | C++ deleting destructor, **not an API to call**; use IUnknown Release |

The exact declared COM/IDL method names and parameter names remain unknown. Above names are explanatory labels. `AM_MEDIA_TYPE` here is 88 bytes on x64, with `cbFormat` at72, format pointer at80; init returns video then audio. Both format blocks are copied with CoTaskMemAlloc in `0x180009370` and belong to caller. With H264 the format is `FORMAT_MPEG2Video` (`e06d80e3-db46-11cf-b4d1-00805f6cbbea`), subtype AVC1 (`31435641-0000-0010-8000-00aa00389b71`). A pair of 16-bit big-endian lengths and SPS/PPS bytes begins at format offset132, with NAL length size4 at128.

The sample object is allocated with CoTaskMemAlloc(48). Its payload is independently CoTaskMemAlloc-owned and copied. Ownership is returned by the queue pop; the harness frees payload then sample after reading. Evidence: `054-pseudo`, `058-pseudo`, `047-pseudo`.

| Sample byte offset | Field observed |
|---|---|
| 0 | uint32 media kind (1=audio, 2=video) |
| 8 | int64 start time in100ns |
| 16 | int64 next/end time in100ns; video filled when the next timestamp arrives |
| 24 | byte keyframe flag; bytes25–27 cleared by producer |
| 32 | uint32 payload length |
| 40 | pointer to payload |

`0x180009660` accepts a direct `rtsp://` URL or pipe-delimited options. Token table `0x180054520` and parser `0x180009ab0` identify `Transport`, `URL`, `UserName`, `Password`, `BufferLength`, `Flags`, `RTCPSynch`, `AudioStream`, `VideoStream`. Transport parser maps HTTP=1, TCP=2, UDP=4. Only localhost TCP/H264 was exercised. Successful explicit options:

```
URL=rtsp://127.0.0.1:8554/camera|Transport=TCP|VideoStream=1|AudioStream=0|RTCPSynch=0
```

## Actual original-DLL run

[probe-original-rtsp.ps1](../scripts/probe-original-rtsp.ps1) loads the exact hash-checked DLL using LoadLibraryEx, gets its class factory with DllGetClassObject and creates the provider directly. It does not call DllRegisterServer, install the MSI, register COM, or create a Windows service. A separate MTA PowerShell child is forcibly bounded to15seconds; its internal capture is limited to40 samples/8.5seconds. Only fixed `127.0.0.1:8554/camera` is used. Media/sample lengths are bounded before copying.

The existing local MediaMTX plus synthetic H264 test-source was the input. FFmpeg generated that test-source independently; **the original DLL itself received and assembled the frames**. This is not a run of the Rust/FFmpeg replacement.

Results in [rtsp-original-mta](../evidence/runtime/rtsp-original-mta/verification.json):

- Init/Stop/ClassFactory/CreateInstance all returned S_OK; child exited0 without external timeout.
- 40 video samples in3066ms, 399 empty polls.
- 640×360 AVC1, profile66, level22, 165-byte video format; absent audio type is zeroed.
- All payloads parse fully as four-byte big-endian length-prefixed NALs, no trailing bytes.
- Keyframes11 and26 contain SPS(25bytes), PPS(4bytes), then five IDR NALs. Nonkey frames contain five type1 NALs.
- Timestamps start0 then666666/1333333/2000000 (100ns). The recovered float scaling produces small rounding differences, e.g17333332 instead of17333333; preserve this as an observed compatibility detail, not an idealized formula.
- Original frame bytes, type-format blobs, JSON frame descriptors, worker stdout/stderr and process result are retained. These are controlled-run files, **not REA Evidence IDs**.

The first run used PowerShell's default apartment and stopped at CoInitializeEx with RPC_E_CHANGED_MODE before loading the target. It is retained separately in `evidence/runtime/rtsp-original`; the corrected `-MTA` run above is authoritative for the successful result.

## Rust/Linux port obligations derived from original

1. Implement/reuse an RTSP/RTP receiver while retaining the original media behavior: OPTIONS/DESCRIBE, selected-track SETUP, PLAY, RTP/RTCP timing, auth/session/keepalive handling. FFmpeg is not required by this original path.
2. Preserve H264 payload processing, including aggregation/FU NAL reconstruction, timestamp grouping, four-byte AVCC lengths, SPS/PPS metadata and keyframe insertion. Match original emitted sample layout semantically; COM itself need not survive on Linux.
3. Preserve codec configuration and encoded picture bytes. There is no evidence here that decoding/re-encoding is part of this DLL's H264 receive path.
4. Replace Windows COM/CoTaskMem/critical sections/events/threads/Winsock boundaries with explicit Rust ownership, queues/tasks and sockets. Do not claim binary ABI compatibility on Linux.
5. Keep a sample corpus from original execution and compare NAL bytes, lengths, timestamp grouping, keyframe classification and parameter-set insertion. Current40 samples are a positive baseline, not a complete protocol conformance corpus.

Still unverified: UDP loss/reordering, FU loss and all aggregation variants; actual camera interoperability; auth challenges, reconnect/timeouts, RTCP synchronization, rtsps/TLS; H265/audio; malformed streams; precise original queue thresholds/discard behavior under backpressure. Some were located in native code but not exercised. Original WebSocket/WebRTC output must be validated in their respective server/DLL paths; this RTSP capture alone does not establish browser compatibility.

## Principal REA Evidence IDs

| Claim | Evidence ID |
|---|---|
| LIVE555 version bytes | `ev_c0fc797c222ad375b8880bee912db6cd16e736c0c6c3293cf315cb7501c6d15a` |
| H264 sprop/media format | `ev_0c8b444cd650fdee77a78fe630cea54d65d8ce33630669c1177873fa4e5e8123` |
| RTP H264 header parsing | `ev_a911b85a00d3c65d898573477ccd63434a779311885257dded90d767b7b78eaf` |
| RTP aggregation elements | `ev_bc32eda1730750d11831842aa9acfb2991d0bd20a2b14ba58543a1affd546c72` |
| Init ABI | `ev_bae2fcf94fc8cf4b0130bc88031429651099dfcb2c5bae13aa95bebced93ba52` |
| Sink timestamps and submission | `ev_54fed709dccb9af7ff75ef0022abd0b692a80f4076e5f54675bb3798c7ac8b47` |
| Sample allocation and gates | `ev_9decc8cecdfc1f3fd85e764142b49053d0213fbc7b143468de4c71c337901335` |
| Timestamp access-unit assembly | `ev_cf6f9fa6d0f5942ae26ff9baf1665c343d19a21ebbd487f4b3517ed9659ebd6a` |
| AVCC NAL append | `ev_43711a302bfbffb4157126958034f24a64edd86ed7a4d6fb3396af8c9b3f4d9d` |
| SPS/PPS insertion | `ev_5194ab0265e62b2d93e9a137d452827f66261505344f58e1a2557d2734f84a1d` |
| Queue delivery | `ev_576b2664b8d144211af9199b033e0bc74142cd7fe0553312a6b2524e82f6699c` |

All IDs have full subject digest, provider/profile, limitations and exact parameters in the linked evidence directory. The initial timed-out `rtsp` session is diagnostic history; `rtsp-v2` supplies the completed native analysis.
