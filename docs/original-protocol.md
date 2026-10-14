# 원본 SDK 프로토콜 조사 — 원본 서버 검증 전

후속 대조: `original-server-native.md`에서 원본 EXE의 codec-first 협상과 MSE 생산자
헤더 함수를 확인했다. 아래 문서는 SDK 자체의 근거만 기술하며, DLL 실행 검증은 별도
`original-rtsp-native.md`, `original-webrtc-native.md`에 기록한다.

## 범위와 근거의 등급

이 문서는 공식 SDK의 **수정하지 않은 플레이어 소스**를 읽은 결과다. 사용자 MSI 안의 서버를 실행하거나 서버 함수를 디컴파일한 결과가 아니다. 아래 규약은 기존 플레이어와 호환하기 위한 구체적인 계약이며, 네이티브 분석 및 원본 서버와의 통신 캡처로 대조해야 한다. 이전 독립 구현의 재생 성공은 이 계약의 검증 근거가 아니다.

- 대상 MSI: `Unreal Media Server x64 v15.0`, SHA-256 `513ffbda23167be0528dc6885a1420c5d3fde9ad1db4e094d4eeeecf0d6e1731`.
- [공식 SDK 페이지](https://umediaserver.net/umediaserver/source.html)는 SDK v15.0을 표시하며 [SDK ZIP](https://umediaserver.net/bin/UMediaSDK.zip)을 연결한다.
- 받은 ZIP: `evidence/protocol/UMediaSDK.zip`, 4,037,205 bytes, SHA-256 `63c97d27d8d537954e2caef754a70ce30fdedbac04f2b3b12ab94116b9e4a2ea`.
- 전체 출처와 파일별 해시: `evidence/protocol/provenance.json`. 원본 SDK는 `evidence/protocol/sdk/Unreal Streaming Technologies SDK v15.0/`에 보존했다. 이후 `SDK/`는 이 경로를 뜻한다.
- SDK 루트 `Readme.txt:5`의 v15.0 변경 내역은 ServerConfig COM과 SRT 설명도 변경이다. WebRTC 플레이어 헤더는 v2.5 / 2021-01-07이며, MSE 프로토콜 문서는 v12를 명시한다. **버전 이름이 15.0이라는 이유만으로 MSI와 JS가 동일 빌드라고 단정할 수 없다.**
- 아래 참조는 저장된 원본 파일의 1부터 시작하는 줄 번호다. 웹 페이지 내용을 재인용한 설명이 아니라 직접 조사한 SDK 파일의 행동을 기록한다.

## WebRTC 수신 플레이어 계약

주 근거 `W = SDK/WebRTC/WebRTC_Player/unrealwebrtcplayer.js` (SHA-256 `0dc1928bb618e3bd5753284316b6cbc09be3cd1483a53d1bb9f086f6999e24a2`). 샘플 설정은 같은 디렉터리 `play.html:28–40`.

| 순서 | 관찰된 동작 | 소스 위치 |
| --- | --- | --- |
| 1 | 클라이언트가 `ws[s]://host:port/webrtc_playnow/{singleport\|randomport}/{udp\|tcp}/{alias}` 연결. 토큰이 있으면 `/sid:{sid}`를 붙임. | W:123–128 |
| 2 | 서버가 먼저 텍스트 `{videoCodec}\|-\|-\|{audioCodec}`를 전송해야 한다. 실제 구분자 5문자는 `\|-\|-\|`에서 역슬래시를 제외한 `|-|-|`다. 영상/음성 없음은 각각 빈 문자열. | W:138–170 |
| 3 | 첫 응답에 구분자가 없으면 클라이언트는 오류 문자열로 취급하고 종료한다. | W:144–152 |
| 4 | 클라이언트는 `new RTCPeerConnection(null)`을 생성한다. 수신할 트랙 유무에 맞는 offer 옵션을 고른다. | W:152–171 |
| 5 | SDP의 코덱을 서버가 알린 값으로 제한하고 `sendrecv`를 `recvonly`로 바꾼다. `extmap-allow-mixed`를 제거한다. `JSON.stringify({type:"offer",sdp:...})`를 먼저 전송하고 로컬 SDP를 설정한다. | W:77–99,241–300 |
| 6 | 브라우저 ICE candidate 콜백은 아무것도 보내지 않는다. 즉 이 JS에는 클라이언트 candidate trickle 메시지가 없다. | W:102–104 |
| 7 | 서버는 `JSON(answer)` + `|-|-|` + `JSON(candidate)`를 한 텍스트 메시지로 보낸다. candidate 객체에서 `candidate`, `sdpMLineIndex`를 사용한다. | W:190–197 |
| 8 | 클라이언트는 원격 SDP/candidate를 적용한 직후 **WebSocket을 닫으며 WebRTC PeerConnection은 유지한다.** 종료 시점이 미디어 세션 종료가 아니다. | W:194–200 |
| 9 | 사용자가 Stop을 누르거나 PeerConnection이 failed가 되면 PeerConnection을 닫는다. 별도의 서버 stop 명령은 없다. | W:38–69,115–120 |

기호 혼동을 피한 영상 전용 첫 응답 예: `H264|-|-|`. 이 문자열 예는 클라이언트 파서를 만족하는 **합성 예시**이며 원본 서버 캡처가 아니다. 첫/두 번째 메시지 모두 구분자 없는 응답은 오류로 취급된다(W:147–150,178–188).

추가 호환 조건:

- H264의 SDP 선택은 `profile-level-id=42`가 있는 payload만 고른다(W:302–315). 주석은 서버가 다른 profile/level도 보낼 수 있다고 주장하지만 이 조사에서는 실제 디코딩 범위를 입증하지 않았다. 이를 서버가 baseline으로 재인코딩한다는 증거로 해석하면 안 된다.
- 오디오 clock은 opus 48,000, 그 외 8,000; 영상 clock 90,000(W:78–85). 샘플 설명은 영상 H264/VP8/VP9, 오디오 opus/PCMA/PCMU를 명시한다(`play.html:31`).
- `singleport`이며 연결 주소가 숫자 IPv4일 때, candidate IP가 다르면 클라이언트가 candidate의 IP를 접속한 서버 IP로 바꾼다. localhost와 도메인 주소는 예외(W:213–239). NAT 동작을 서버 단독 구현이라고 보면 안 된다.
- 샘플 기본 포트는 signaling 5119(WS)/443(WSS), single WebRTC port 5135이고 udp/tcp 둘 다 선택할 수 있다고 설명한다(`play.html:34–38`). 실제 포트 바인딩과 TCP ICE 구현은 네이티브/런타임 확인 대상이다.
- sid 해석, 쿠키 인증, 오류 목록, SDP answer 상세, candidate 우선순위/종류, DTLS 인증서 생성, SRTP profile, RTCP/NACK/PLI 처리, ICE 역할과 timeout은 이 JS만으로 확정할 수 없다.

### 원본 JS의 제어 흐름 실행 확인

`evidence/protocol/probe-original-webrtc-client.mjs`는 원본 JS 파일을 변경 없이 Node VM에서 실행한다. 브라우저 API와 서버 응답만 모의 객체로 주입했다. `node evidence/protocol/probe-original-webrtc-client.mjs`로 재현 가능하다.

결과 `evidence/protocol/webrtc-client-probe.json`: 9개 assertion 통과. 영상 전용 offer, codec 제한/recvonly/확장 제거, candidate 미전송, NAT 주소 치환, signaling 종료 뒤 PeerConnection 유지, Stop에서 PeerConnection 종료를 확인했다. **실제 브라우저 ICE나 원본 서버를 시험한 결과가 아니다.**

## MSE / WebSocket 수신 계약

주 근거 `M = SDK/Players/HTML5 MSE Player/unreal_html5_player_script_v2.js` (SHA-256 `a17c9d8ee2a8dfad7ed03c6cca307b137a7c73f977f6418b54ac5eb052124b27`). 보조 문서 `P = 같은 디렉터리/HTML5 MSE Server protocol.txt` (SHA-256 `f08e3b1bd56bc22b743c4b1d2d617e589a2220de46a1188898b4bae6ec3e2997`).

- URL `ws[s]://host:port/{alias}`; 필요시 `/sid:{token}`. 최초 play JSON을 보내지 않고 연결 즉시 서버 데이터를 기다린다(M:59–72,884–889; P:7–11).
- `binaryType="arraybuffer"`. 다음 헤더는 **각 WebSocket binary message의 payload**에 포함되는 언리얼 헤더이며 WebSocket 자체 프레임 헤더와 다르다.
- integer/문자열은 `Uint16Array`로 읽어 JS 실행 환경의 native endian을 사용한다. x86/x64의 실제 목표 환경에서는 little endian/UTF-16LE가 된다. 타임스탬프는 명시적으로 little endian `getInt32(6,true)`다. 플랫폼 독립적인 wire endian 선언은 문서에 없으므로 native 생산자 분석으로 대조할 것.

| message | offset | 길이 | 관찰된 의미 |
| --- | ---: | ---: | --- |
| error | 0 | 1 | 0 |
| error | 1 | 1 | P는 reserved로 정의; M은 읽지 않음 |
| error | 2 | 나머지 | 16-bit code unit 오류 문자열. 표시 후 재생 종료. P는 서버가 연결도 닫는다고 명시. |
| init | 0 | 1 | 1 |
| init | 1 | 1 | 모드 flag. M은 4/8/12이면 realtime. P의 예제는 3/12이면 audio+video. 단일 영상과 단일 음성 각각의 bit 배정은 이 코드만으로 확정하지 않음. |
| init | 2 | 2 | M의 `timeshiftSessionGlobalStartTime`; 0 아니면 timeshift 활성 |
| init | 4 | 2 | M에서 미사용 |
| init | 6 | 2 | M의 `timeshiftBufferLimit` |
| init | 8 | 2 | M의 `timeshiftPos`; 0은 실시간 위치 |
| init | 10 | 8 | M에서 미사용 |
| init | 18 | 2 | XML 길이 **bytes** (문자 수가 아님) |
| init | 20 | XML 길이 | 16-bit code unit XML. 최소 읽는 요소 `mimetypecodec`의 텍스트를 `MediaSource.addSourceBuffer()`에 사용 |
| init | 20 + XML 길이 | 나머지 | ISO-BMFF initialization segment |
| media | 0 | 1 | 2 |
| media | 1 | 1 | 1이면 server buffer filling 카운터, 그 외 empty 카운터 |
| media | 2 | 1 | timeshift 모드에서 0이면 `timeshiftInProgress=true` |
| media | 3 | 1 | timeshift current-time 값의 상위 8 bit |
| media | 4 | 2 | timeshift current-time 값의 하위 16 bit, native little endian |
| media | 6 | 4 | signed 32-bit little endian segment timestamp, milliseconds |
| media | 10 | 나머지 | ISO-BMFF media segment |

근거: error M:894–903; init M:905–943; media M:944–969. 특히 media timeshift의 3 bytes 값은 순차 u24LE가 아니다. x86/x64 기준 `(payload[3] << 16) | (payload[5] << 8) | payload[4]`로 조합된다(M:956–959). 시간 단위는 UI 연산상 seconds로 취급하지만 native 생산자에서 확인해야 한다(M:314–319).

**구 문서와 실제 포함 JS 차이:** P는 init offset 2–17, media offset 2–5를 reserved라고 적지만 M은 이미 timeshift로 소비한다. 단순히 reserved로 취급하는 구현은 timeshift 호환이라고 할 수 없다. live-only 최소 이식에서는 0으로 내보내는 방안을 원본 live 캡처와 대조해야 한다.

XML의 전체 루트/필드 목록, 원본 avcC 생성 규칙, ftyp brands, 트랙 ID, timescale, moof/trun/tfdt/mdat 배치, 프레임별 duration/CTS, SPS/PPS 갱신, keyframe 경계는 이 클라이언트가 파싱하지 않으므로 여기서 복원할 수 없다. MSE 엔진에 통째로 전달하기 때문이다. P:1–2는 H264/AAC와 realtime 30ms/buffered 500ms segment를 설명하지만 **v15 MSI 실제 값은 미확인**이다.

### 제어 메시지와 세션

| 방향 | 텍스트 메시지 | 관찰 |
| --- | --- | --- |
| client → server | `continue` | timeshift 흐름 재개(M:261–267) |
| client → server | `pause` | timeshift 사용자 pause 및 20초 이상 앞서 버퍼링될 때 흐름 중지(M:508,856–866) |
| client → server | `seek:{seconds}` | timeshift seek. 0은 live. client queue 비우고 새 init을 기다림(M:270–281,997) |
| client → server | `switch:{alias}` | adaptive bitrate alias 전환(M:804–853) |

MSE는 WebRTC와 달리 데이터가 WS로 흐르므로 WS close가 재생 종료다(M:1049–1052). init message는 시작뿐 아니라 live playlist의 콘텐츠 변경 시 다시 올 수 있다(P:36). M의 SourceBuffer 생성은 최초 init에서만 이뤄진다(M:929–940). 같은 MIME/codec에 대한 재초기화와 codec 변경은 별도 검증이 필요하다.

클라이언트 startup은 realtime이면 timestamp가 100ms 초과, buffered이면 2000ms 초과를 기다린다(M:1032–1039). timestamp를 항상 0으로 채운 서버는 유효한 MP4를 전송해도 이 플레이어를 시작시키지 못한다. adaptive 모드는 10초 주기이며 filling/empty 비가 0.05 초과면 낮은 alias로 이동한다(M:808–818,1041–1042). 원본 서버가 filling bit를 세우는 정확한 기준은 미확인이다.

## Rust 이식 전에 충족할 확인 항목

| 항목 | 현재 답변 상태 | 다음 근거 |
| --- | --- | --- |
| 외부 signaling URL/메시지/WS 생명주기 | 공식 JS로 확인 | 원본 v15 서버 캡처와 상호 비교 |
| 외부 MSE 헤더/클라이언트 제어/소비 방식 | 공식 JS로 확인 | 원본 binary message와 fields 대조 |
| RTSP DESCRIBE/SETUP/PLAY, auth, RTP/RTCP 수신 | 이 문서 범위에서 미확인 | URTSPClient.dll 함수 추적 + 합성 RTSP 서버 캡처 |
| H264 depacketize, SPS/PPS, 프레임 경계/타임스탬프 | 미확인 | native call graph, 단일 NAL/STAP-A/FU-A fixture |
| MSE 실제 muxing 구조 | 미확인 | 원본 송출 bytes의 MP4 box 구조 검사, 생성 함수 추적 |
| WebRTC payload 재포장/SSRC/seq/RTCP/ICE/DTLS/SRTP | 미확인 | UnrealWebRTC.dll 호출 경계/구현 분석 및 packet trace |
| 기존 공식 클라이언트 → Rust 서비스 재생 | 미검증 | 수정하지 않은 SDK 플레이어로 live-only 최초 검증 |
| 원본 내부 구현과의 동등성 | 미확인 | 네이티브 함수별 근거와 분기/오류/수명주기 대조 |

기존 FFmpeg 시제품에 URL과 헤더만 붙이는 것은 원본 내부 처리 이식이 아니다. 호환 검증용 계약은 여기서 확보했지만, RTSP부터 두 송출 경로까지 원본의 실제 처리 단계를 연결한 뒤 구현해야 한다. 원본 소스 자동 복원 또는 완전 동등 포팅을 주장하지 않는다.

SDK 원본과 ZIP은 조사 증거이며 새 서비스의 배포 파일에 자동 포함하지 않는다. 클라이언트 변경으로 서버 오류를 숨기지 않고 원본 클라이언트를 비교 기준으로 유지한다.
