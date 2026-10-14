# UnrealWebRTC.dll 원본 네이티브 분석

이 문서는 원본 DLL의 정적 분석과 별도로 표시한 원본 DLL 실행 관측을 기록한다. Rust 재구현의 동작을 원본 동작의 증거로 사용하지 않는다. Ghidra 의사코드는 복원된 원본 소스가 아니다.

## 대상과 분석 방법

- 입력: `evidence/extracted/UnrealWebRTC.dll`, MSI에서 추출한 x64 PE, 6,049,624바이트.
- SHA-256: `3a283df31ced9fdca9cf02d367213b0be0bf01394744d56ff85f9ce627a75287`.
- preferred image base: `0x180000000`. 아래 VA는 이 기준이며 ASLR 런타임 주소가 아니다.
- REA 6.4.0 + Ghidra 12.1.4. 전용 세션 `evidence/native/webrtc-v2/`; 각 요청·원시 응답·Evidence를 함께 보존한다.
- `webrtc/` 세션은 클라이언트 60초 제한으로 import 중단됨. 드라이버 timeout 인자를 수정한 뒤 `webrtc-v2/`에서 재시작했다.
- 정적 분석 단계에서는 DLL 실행·COM 등록·원본 COM 클래스 생성을 하지 않았다. 뒤의 통제된 런타임 검증에서는 선택한 두 원본 DLL의 클래스를 직접 생성하되 등록·서비스 설치는 하지 않았다.

## COM 경계: 리소스에서 확인한 사실

REA `inspect_pe_resources` Evidence: `ev_4e695b16d37d8f943e4d55986c96611e138576cd5d93c84ecc3d86355adb958d`.

TYPELIB 리소스는 파일 offset 5,902,312부터 2,600바이트이다. 추출한 독립 `.tlb`의 SHA-256은 `1af56eea5a08aad72f0d89e96215ffeb3a2b8e11a25f72bae6ef89446e1fbc3e`이다. `scripts/inspect-typelib.ps1`가 이 독립 파일을 `LoadTypeLibEx(REGKIND_NONE)`로 읽었다. DLL을 로드하거나 등록하지 않았다. 결과는 `evidence/native/webrtc-v2/typelib.json`이다. 이 추가 해석은 REA 자체의 파싱 결과와 구분한다.

| 항목 | 리소스에 들어 있는 식별자 / 시그니처 |
|---|---|
| PlayingPeerClient CLSID | `8c5a8556-d17d-4303-b66a-b5f94f3ed9cc` |
| IPlayingPeerClient IID | `e403a754-6df0-4c75-9e86-d33bf5c25008` |
| vtable +24 | `HRESULT InitClient([in] BSTR sSDP, [in] BSTR sProtocol, [out] BSTR* psSDP, [out] BSTR* psCandidate)` |
| vtable +32 | `HRESULT Terminate()` |
| PublishingPeerClient CLSID | `da964f25-1650-4538-8946-ddf1959a1c99` |
| IPublishingPeerClient IID | `4a9b54f9-ad1c-4ed0-8950-32d20b564f7f` |

Publishing 쪽의 InitClient에는 `sFlags` 입력이 더 있고 UninitClient, SetStreamFlags, IsPublishing가 추가된다. 리소스에는 두 COM 클래스 모두 `ThreadingModel = Free`라고 기록되어 있다.

## 정적 RTTI 및 vtable 후보

`scripts/inspect-msvc-rtti.mjs`는 PE 섹션, MSVC x64 TypeDescriptor, CompleteObjectLocator와 vtable 포인터의 구조를 검증해 연결했다. 이는 디컴파일을 대체하지 않는 코드 진입점 후보 수집이다. 결과: `evidence/native/webrtc-v2/rtti.json`.

| 타입 | vtable VA | 핵심 후보 |
|---|---|---|
| `PassThroughEncoder` | `0x18043d610` | `0x18009f720`, `0x18009f730`, `0x18009f700` |
| `PassThroughEncoderFactory` | `0x18043d678` | `0x18009f6f0` |
| `ATL::CComObject<CPlayingPeerClient>` 주 인터페이스 | `0x18043db80` | slot 3 `0x1800a01b0` InitClient, slot 4 `0x1800a0880` Terminate |
| 같은 객체 +8 인터페이스 | `0x18043dbb0` | `0x1800a08a0`, `0x1800a0af0`, `0x1800a0b40` |

바이너리에는 `FakeVideoCapturer`, `EncodedMediaFramesSink`, libwebrtc의 PeerConnection/RTC 타입들 및 `../../webrtc/...`, `../../third_party/boringssl/...` 경로가 함께 존재한다. 이 이름만으로 실제 패킷 전달이나 재인코딩 유무를 결론 내리지 않는다. 아래 함수의 호출·데이터 흐름으로 검증한다.

## 결론: 원본은 이미 인코딩된 프레임을 libwebrtc로 넘긴다

**관측:** `CPlayingPeerClient`는 `IMediaFlip` 인터페이스로 미디어 형식과 인코딩된 프레임을 받는다. `PassThroughEncoder`를 객체 안에 만들고, libwebrtc가 등록한 encoded-image 콜백으로 원래 프레임 버퍼를 전달한다. 아래 연결은 RTTI 이름만의 추측이 아니라 생성자·팩토리·콜백 등록·프레임 입력 코드와 객체 offset의 일치로 확인했다.

1. 생성자 `0x1800a23d0`은 객체 `+8`에 `IMediaFlip` vtable, `+0x10`에 PeerConnectionObserver, `+0x18`에 CreateSessionDescriptionObserver를 두고 `+0xf0`에 PassThroughEncoder를 생성한다. Evidence `ev_b31f97ec1800cbb2ecb5f9b1c484c1d1a08748d9f1d48d51af1442e920a3d9b0`.
2. PassThroughEncoder 생성자 `0x18009eb70`은 vtable과 null callback 필드 하나를 설정한다. Evidence `ev_4cc9e76692bad7505a97dde72e51ff3e5864fdacd013d6f84414a6d4a7b1e85a`.
3. 세션 설명 성공 콜백 `0x1800a13a0`은 observer의 `this+0xd8`, 즉 주 객체 `+0xf0`를 공유 팩토리 슬롯 `0x18057aba0`에 게시한다. critical section 안에서 이전 슬롯을 기다린 후 게시하며, 이후 local session description을 설정한다. Evidence `ev_a4e170f2d5414a7997645aa491109ae3f79a47a15a898334313328140c30ead4`.
4. 팩토리 함수 `0x18009f6f0`은 `factory+0x20`의 포인터를 반환하고 슬롯을 0으로 만든다. Evidence `ev_20f36a82bacb1bd002bfaca9b89ff92f368696826516f11a912bea9fd0b1fdef`.
5. encoder vtable slot 2의 `0x18009f720`은 전달받은 콜백을 `encoder+8`, 즉 주 객체 `+0xf8`에 저장한다. Evidence `ev_5613594a02ed8736edf12132a6498cd0ce4da52b6de1e8fa29d7c89bd16b20ce`. 여러 encoder vtable 메서드는 `0x1803edd20`의 `return 0` stub이다. Evidence `ev_c8d08a35920a8eaec7e0c33baa3a0929e7ad8ae2360f1cd32a0ac26755f131e7`.
6. 프레임 입력 `0x1800a0b40`의 this는 `IMediaFlip` 포인터인 주 객체 `+8`이다. 이 함수의 `this+0xf0`가 곧 주 객체 `+0xf8` 콜백이다. 원래 버퍼 포인터와 길이를 encoded-image 구조체에 넣어 콜백 vtable `+8`을 호출한다. Evidence `ev_2f4f3939c8825885f892694f99f4f2c39f7f26ee7326e21142cac5f0f44e32a9`; 함수 dossier `ev_e6c4f1dcf8b0d674d52a6c91731fff2320e19839578d37149de88a8262ffb32b`.

이 확인된 video playback 경로에는 FFmpeg 호출이나 픽셀 디코드·재인코드 단계가 없다. 따라서 앞서 만든 FFmpeg transcode 중심 시제품을 원본 이식이라고 설명할 근거가 없다. 단, 이것은 확인한 경로의 결론이며 제품 전체의 모든 코덱·녹화·변환 기능에 대한 부재 증명은 아니다.

## IMediaFlip 입력 계약

`0x1800a08a0`은 IMediaFlip vtable `+0x18`에 해당한다. 두 AM_MEDIA_TYPE형 구조체를 연속으로 받는다: video `+0`, audio `+0x58`. 각 구조체의 formattype GUID는 `+0x2c`, pbFormat은 `+0x50`이다. Evidence `ev_4f824c3d3c220e60be98c69e81ac3c867d4a0f83911b41af2180a76f61314aa6`.

- VideoInfo 형식은 width/height를 pbFormat `+0x34/+0x38`, compression을 `+0x40`에서 읽는다. VP80·VP90를 codec 문자열 `vp8`·`vp9`로 저장한다.
- VideoInfo2/MPEG2Video 형식은 width/height를 `+0x4c/+0x50`, compression을 `+0x58`에서 읽는다. `H264`, `X264`, `h264`, `x264`, `AVC1`, `avc1` 값은 `h264`로 저장한다.
- 오디오는 Opus subtype GUID, WAVE_FORMAT_MULAW(7), ALAW(6)에 따라 `opus`, `pcmu`, `pcma`를 저장한다.
- codec 문자열의 실제 바이트는 `0x18043eb00`에서 확인했다. Evidence `ev_97872c6f958138b1993fe7f7010795f7e80ae57ad8a4db961e2df640bb453aaf`.

IMediaFlip vtable `+0x28`, `0x1800a0b40`이 읽는 프레임 descriptor는 다음과 같다. 이름은 분석자가 붙였고 원래 C++ 선언명은 미확인이다.

| 바이트 offset | 읽기 | 관측된 사용 |
|---|---|---|
| `+0` | 32bit integer | 값 2이면 video 경로, 그 외 audio 경로 |
| `+8` | signed 64bit | video RTP timestamp = `(int32)(value / 10000) * 90`; 100ns 단위를 ms로 절삭한 뒤 90kHz로 환산하는 것으로 해석 |
| `+24` | byte | keyframe flag; 첫 keyframe 전에는 다른 프레임을 건너뜀 |
| `+32` | unsigned 32bit | 인코딩 payload 길이 |
| `+40` | pointer | 인코딩 payload 시작 주소; encoded-image에 그대로 전달 |

H.264 payload 뒤의 메타데이터는 `buffer+payload_length`에서 읽는다.

```text
u16_le nal_count
u16    reserved_or_padding       // 이 함수에서는 읽지 않음
repeat nal_count:
    u32_le offset_in_payload
    u32_le nal_length
```

이를 객체의 RTPFragmentationHeader형 배열에 확장해서 콜백에 함께 전달한다. assembly에서도 `0x1800a0eb1 MOV [RSP+0x20], R15`와 `0x1800a0ebe CALL [RAX+8]`로 다섯 번째 ABI 인자가 fragmentation pointer임을 확인했다. 디컴파일러가 호출 인자를 모두 표현하지 못하므로 assembly를 함께 보존했다. Evidence `ev_3047cc5333de7232c039a1941773260099f3e5dbd92c87512687acb7fb96916a`.

따라서 RTSP 쪽이 전달하는 프레임은 단순 임의 H.264 byte stream이 아니라, 인코딩 payload + 별도 NAL offset/length 메타데이터를 가진 내부 전달 형식이다. start code 포함 여부와 upstream의 trailer 생성은 RTSP/server 쪽 분석과 대조해야 한다.

## 원본 offer/answer와 candidate 처리

`IPlayingPeerClient.InitClient`, `0x1800a01b0`의 분석:

1. BSTR 입력을 좁은 문자열로 변환하고 SDP JSON 사본을 소문자로 만들어 codec/media 검사에 사용한다.
2. `0x1800a66f0`은 `m=video`, `m=audio`, 첫 `a=rtpmap:` 인근의 codec 이름을 미디어 설정과 대조한다. media 유무가 다르면 실패한다. audio payload type도 추출해 저장한다. Evidence `ev_743f90d66764fdaf3da7da50770adf44ba2e7785c8e7a2c310cc5a7ae3956260`.
3. PeerConnection/stream/track을 생성한다. `video_label`, `audio_label`, `stream_label`을 사용하고 video track의 소스는 `cricket::FakeVideoCapturer` 생성자 `0x1800a25d0`이다. Evidence `ev_37d0a7de4e82a500c8d13d805615a70a32decc3b61442868aa6facb86d11417e`, `ev_119555db98496cf751d501339a68493c5b28a92d210523147d076e7b11a752ff`.
4. JSON의 `type`과 `sdp`를 읽어 세션 설명을 만들고 PeerConnection vtable `+0xd8`을 호출한다. type이 `offer`이면 `OfferToReceiveVideo=false`, `OfferToReceiveAudio=false` 조건을 구성해 vtable `+0xc0`으로 answer 생성을 요청한다. 함수 전체 Evidence `ev_5c8b8616b432fa48fe433903296459adf3838064d01efb7faa36d140c6c6649d`.
5. 세션 설명 성공 콜백 `0x1800a13a0`이 `{type, sdp}`를 Json::StyledWriter로 직렬화한다. candidate callback `0x1800a1020`은 protocol 문자열이 맞는 첫 candidate를 `{type:"candidate", sdpMid, sdpMLineIndex, candidate}`로 직렬화한다. Evidence `ev_a4e170f2d5414a7997645aa491109ae3f79a47a15a898334313328140c30ead4`, `ev_aa672a58ea4dd135fad1dd076b74825198970238216660a39eb5ccc32ec406d6`.
6. InitClient는 `Sleep(5)` 루프 최대 1001회에서 결과/실패 플래그를 기다린다. SDP·candidate 문자열이 모두 있으면 각각 BSTR 출력 포인터로 돌려준다. 대략 5초의 nominal polling budget이며 실제 경과시간을 측정한 값은 아니다.

`sdp`, `type` 등의 문자열 주소는 별도 read_bytes로 확인했다. Evidence `ev_5a5714fdf443c25f230a05f64bf6d0894f75b5f367b529d51a79186e28b4fae9`. protocol 비교 helper `0x1800a17e0`는 문자열 동등 비교이다. Evidence `ev_eba33dcceb1061ab6d6081cb09c148b13c5b4ef8615e6a320a18151cbdcfacf5`.

UMediaServer.exe 담당 분석에서 PlayingPeerClient의 CLSID/IID와 InitClient 호출을 별도로 확인했다. 서버는 두 출력 BSTR을 `|-|-|`로 연결한다. DLL은 JSON 두 개를 만들고, 서버가 WebSocket 응답 형식을 구성하는 경계이다. 서버 근거는 `evidence/native/server-v2/010-route-func.c`, `017-guid-read`, `030-sink-iid`를 참조한다.

## H.264 packetizer와 transport

원본 PassThroughEncoderFactory 생성자 `0x18009eb90`에는 H.264 profile-level-id `42001f`, `42e01f`, `4d0032`, `640032` 및 `level-asymmetry-allowed`를 codec 목록에 넣는 코드가 있다. Evidence `ev_1fe4d35a58b87d93f50515d7dedbf1d3e9a58993f874e51e60fb0511a691a47c`. 이 목록이 모든 브라우저와 협상되는지는 런타임 확인이 필요하다.

libwebrtc의 `RtpPacketizerH264` RTTI에서 vtable `0x1804ec5a8`을 찾고 다음 실제 코드로 연결했다.

| 함수 | 확인한 동작 | Evidence |
|---|---|---|
| `0x1803dd024` | 전달된 fragmentation offset/length로 NAL을 순회한다. SPS(type 7)는 검사·VUI 수정 helper를 거쳐 패킷 계획에 넣을 수 있다. | `ev_57ab17b33823eef7f84281b690efcc67e1b667b7c217bfaf1cd3dceebeb34067` |
| `0x1803dad84` | non-interleaved 모드에서 큰 NAL 분할 또는 작은 NAL 집합화를 선택한다. | `ev_d75326dd0baf6080221a7fcfb8704d7ace72fa12bc29f26af537a195ef4ce083` |
| `0x1803db418` | 원 NAL header를 제외하고 최대 payload-2를 기준으로 FU 분할 크기·first/last를 계산한다. | `ev_3d7542caa0bd978b4da9cd836fe191c5d10da29b3b361048ac8ead535a997365` |
| `0x1803db8b8` | payload 한도 내에서 NAL들을 집합화한다. | `ev_9f9e4c3f9a54bb84b4468aab0c40b67323ac72dfe060f84ae776a461d7fb1153` |
| `0x1803db214` | 단일 NAL/FU/STAP 중 하나를 출력하고 마지막 packet marker를 설정한다. | `ev_6f71ede16c0b0d93c3d5bb60a89a550afa47820fb44b5e03847bfc90d7b0fbab` |
| `0x1803db0e8` | FU-A indicator(type 28), start/end 비트와 원래 NAL type을 써서 payload를 복사한다. | `ev_31789a5a8be06bd405ee7f59495e3d0ea05e50732fc601743bcc7bc938c0ac9d` |
| `0x1803dae60` | STAP-A(type 24), 각 NAL의 16bit 길이와 payload를 출력한다. | `ev_16b0010dc04ddca77e1b697c33e3be132d0490e88592708c3d18db00fee2d457` |

DTLS handshake 함수 `0x18020fba8`은 ICE writable 조건을 검사하고 SSL adapter의 vtable 메서드를 호출하며 DTLS handshake 성공/실패를 기록한다. Evidence `ev_cb9e3c4f8fcc78e7cbb00452a00ff828126e24dfccf359adff44e18c0de57211`. SRTP protect 함수 `0x18034f260`은 RTP 최소길이·SSRC·sequence 및 protection 처리를 수행한다. Evidence `ev_a8fabdd5c01551bade4165557ad4185ecd1a3a2ff0eed9b15e331a96323e31e3`.

**해석:** 이 DLL은 libwebrtc/rtc/cricket, BoringSSL, SRTP 구현을 포함하고 원본 전용 COM·pass-through 연결층으로 서버의 인코딩 프레임을 WebRTC로 연결한다. 확인한 packetizer/transport 코드의 존재와 wrapper 구조는 관측했지만, 모든 간접 vtable call을 끝까지 해결한 단일 전체 call graph 또는 실제 네트워크 실행 추적을 확보한 것은 아니다.

## 원본 DLL 런타임 검증: 실제 영상 수신 성공

정적 계약 복원 후 두 원본 DLL을 선택적으로 로드하는 **45초 제한 자식 프로세스**를 만들어 검증했다. COM은 MTA로 초기화하고 원본 `DllGetClassObject` → class factory로 객체를 생성한다. 레지스트리 등록이나 UMediaServer 서비스 설치는 하지 않는다. 해시가 다른 DLL은 실행하지 않는다. 테스트 종료 시 원본 Terminate/Stop을 호출하고 프로세스를 끝내며, 원본 DLL 내부 스레드와 FreeLibrary 경쟁을 피하려 DLL 언로드는 프로세스 종료에 맡긴다.

실행 도구는 `scripts/probe-original-webrtc.ps1`, `scripts/original-webrtc-probe.cs`, `scripts/browser-original-webrtc.mjs`이다. **운영용 서버가 아닌 분석용 oracle**이다.

### 먼저 기록 프레임 재생

`evidence/runtime/webrtc-original-v3/`에서 원본 URTSPClient로 앞서 기록한 프레임을 복원한 server normalizer에 통과시켜 원본 UnrealWebRTC에 전달했다. 46프레임이 수용됐고 Edge가 640×360 영상 32프레임을 디코드했다. 3개 keyframe, packet loss 0, ICE와 DTLS connected를 확인했다.

앞선 v1/v2의 넓은 최신 브라우저 offer는 원본 InitClient에서 E_FAIL이었다. 공식 SDK의 실제 `unrealwebrtcplayer.js` 83–93, 241–310행은 codec 하나 선택, H264 profile42 확인, `a=extmap-allow-mixed` 제거를 수행한다. 최소 브라우저 probe에 같은 조건을 적용한 v3는 성공했다. 두 조건을 함께 바꿨으므로 실패의 단일 원인을 하나로 특정하지 않는다.

### 이어서 live RTSP → 원본 RTC → 브라우저

`evidence/runtime/original-live/`에서 다음 경로 전체를 실행했다.

```text
로컬 합성 영상 RTSP 카메라
 → 원본 URTSPClient.dll (RTSP TCP, 실제 수신)
 → 원본 UMediaServer 0x140025dc0에서 복원한 AVCC normalizer
 → 원본 UnrealWebRTC.dll / IMediaFlip
 → 원본 libwebrtc ICE·DTLS·SRTP
 → 실제 Edge 브라우저 영상 디코드
```

normalizer는 AVCC 길이 prefix를 Annex-B start code로 바꾸고 원본과 동일한 NAL offset/length trailer를 붙인다. 각 NAL payload가 입력과 byte-for-byte 같은지 실행 중 검사한다. 이 fixture는 원본 RTSP DLL이 keyframe에 SPS/PPS를 이미 넣으므로 원본 서버의 선택적 cached prefix 분기는 필요하지 않았다.

| 결과 | 값 / 파일 |
|---|---|
| original RTSP Init / Stop | 둘 다 `S_OK (0)`; `stdout.txt` |
| original IMediaFlip Init | `S_OK (0)` |
| original PlayingPeer InitClient / Terminate | 둘 다 `S_OK (0)` |
| 원본 RTC가 수용한 live sample | 32, notReady 0; `result.json`, `submitted.jsonl` |
| 브라우저 | 640×360, 31 decoded frames, 3 keyframes, framesDropped 0, packetsLost 0; `browser.json` |
| ICE / DTLS | connected / connected |
| DTLS cipher | `TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256` |
| SRTP cipher | `SRTP_AES128_CM_HMAC_SHA1_80` |
| H.264 협상 | payload102, 90kHz, profile-level-id42001f, packetization-mode1 |
| 이미지 | `original-player.png` |

FFmpeg는 로컬 테스트 카메라의 영상 생성기에만 사용된다. **검증한 RTSP 수신→WebRTC 전송 경로에는 FFmpeg 프로세스나 재인코딩이 없다.** 브라우저 probe는 공식 SDK와 동일한 codec/SDP 필터를 적용한 최소 코드이며, 공식 SDK 플레이어 파일 전체나 UMediaServer.exe의 WebSocket 라우팅을 실행했다고 주장하지 않는다. 이 런타임 기록은 별도 통제된 실험 근거이며 REA 정적 Evidence ID가 아니다.

## 미해결 경계와 Rust 이식에 필요한 다음 근거

- 원본 DLL의 live RTSP/브라우저 핸드셰이크는 위 범위에서 성공했다. UMediaServer.exe의 전체 WebSocket 라우팅, 공식 SDK 전체, wire packet capture 및 Rust와의 차등 비교는 별도 미완료 항목이다.
- IMediaFlip frame descriptor의 사용한 필드는 복원했으나 전체 구조체 선언, buffer 소유권·수명 계약, 모든 상태/동시성 조건은 아직 확인하지 못했다.
- H.264 NAL metadata가 upstream에서 만들어지는 위치, SPS/PPS 반복 정책, keyframe 판정은 RTSP/server 쪽 코드와 이어서 대조해야 한다.
- codec별 RTX/NACK/PLI/FEC, pacing, RTCP 정책을 전부 분석하지 않았다. 단순 RTP 송신으로 같은 동작을 주장할 수 없다.
- libwebrtc/BoringSSL의 정확한 커밋·버전은 미식별이다. 현재 Rust WebRTC 라이브러리의 동작과 자동으로 동일하다고 가정하지 않는다.
- Windows COM·critical section·thread API는 Linux 이식 시 교체해야 한다. 미디어 passthrough 계약은 유지 가능한 경계로 보이나 실제 호환성은 기존 SDK 플레이어 및 원본의 통제된 테스트로 검증해야 한다.
