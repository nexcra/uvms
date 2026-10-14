# 원본 v15 런타임 캡처 준비 — 정적 조사만 수행

## 결론

원본이 **서비스·COM 등록 없이 portable하게 실행되는지는 아직 확인하지 못했다.** 설치 MSI, 함께 제공된 도움말, SDK 모두 Windows service와 COM을 사용하는 정식 실행 경로를 보여준다. 따라서 원본 실행 비교는 일회용 Windows x64 VM 안에서 원본 MSI로 설치해 수행하는 것이 재현 가능한 첫 경로다. 개발 PC의 서비스나 레지스트리를 수정하거나 `UMediaServer.exe`, `UServerConfig.exe`, SDK COM DLL을 실행하지 않았다.

실제로 수행한 작업은 MSI 데이터베이스를 읽기 전용으로 조회하고, `hh.exe -decompile`로 CHM 문서만 추출하고, SDK HTML/C# 및 PE의 문자열을 읽은 것이다. 아래 명령 및 설정 예는 **향후 VM 내부에서만 실행할 계획**이며 이번 조사에서 실행되지 않았다.

## 확보한 근거

| 출처 | 내용 / 위치 | 증거의 한계 |
| --- | --- | --- |
| MSI 테이블 | `evidence/runtime-plan/msi-runtime-tables.json`; 재현 `read-msi-runtime-tables.ps1` | WindowsInstaller `OpenDatabase(...,0)` read-only. 설치 수행 아님. |
| MSI 포함 CHM | `evidence/extracted/UServerConfig.chm` → `evidence/runtime-plan/help/*.htm` | 사용자 MSI와 함께 배포된 도움말. 설명이 실제 실행 결과를 대체하지 않음. |
| 도움말 텍스트 | `evidence/runtime-plan/Overview.txt:8–12`, `Networking.txt:18–25` | 원본 HTML에서 태그를 제거한 파생 텍스트. 원본 HTML도 보존. |
| SDK COM 계약 | `evidence/protocol/sdk/Unreal Streaming Technologies SDK v15.0/Media Server/ServerConfig/Readme.htm:739–772,1144–1194` | 공식 SDK 계약. 특정 MSI의 COM 구현과 함수 단위로 대조 전. |
| SDK C# 사용 예 | 같은 SDK의 `Media Server/ServerConfigUsageExample/Form1.cs:80–118` | 설정 값을 구성하는 실제 예제 소스. 실행하지 않음. |
| PE 문자열 | `evidence/runtime-plan/runtime-strings.json`; 재현 `extract-runtime-strings.mjs` | 문자열과 파일 offset만 관찰. 호출 경로/실제 레지스트리 접근을 입증하지 않음. |

SDK 원본 출처/해시는 `evidence/protocol/provenance.json`과 `docs/original-protocol.md`를 참조한다.

## 설치·시작과 저장 위치

### 서비스

CHM `Overview.htm`은 서버가 Configurator와 분리된 Windows service로 실행되며 Configurator에서 시작/중지할 수 있다고 설명한다. Configurator 창을 계속 열어둘 필요는 없다. 파생 `Overview.txt:5–12`.

MSI의 `ServiceInstall`과 `ServiceControl` 테이블은 모두 빈 배열이다. 따라서 이 두 테이블만 보고 서비스가 없다고 판단하면 안 된다. `CustomAction`에 다음 항목이 있다.

| Action suffix / File | Target | InstallExecuteSequence |
| --- | --- | --- |
| `_C6E1DFBE_0339_4B63_8CDC_5C6A01AF6E4F` / `_3E3097BDC81343C38B61AD28FAA8CB21` | `-service` | 5993, condition `$C__3E3097BDC81343C38B61AD28FAA8CB21>2` |
| `_8551E5A5_0196_4E4C_B8C5_FE1766B7218C` / 동일 File | `/unregserver` | 1698, condition 해당 component `=2` |
| `_9EA04856_25D2_4AC8_B694_091D2311C7BE` / 동일 File | `/unregserver` | 5999, condition 해당 component `>2` |

`evidence/msi-File.json`에서 이 File 키는 `UMediaServer.exe`, 버전 `15.0.0.150`에 매핑된다. 그 외 설치 custom action용 embedded binary도 있으며 아직 의미를 분석하지 않았다. 따라서 `UMediaServer.exe -service` 한 줄로 MSI의 모든 작업을 대신할 수 있다고 주장하지 않는다. DLL self-registration과 helper의 역할이 남아 있다.

PE 정적 단서는 서비스 이름 `UMediaServer`, `SYSTEM\CurrentControlSet\Services\`, `RegServer`, `UnregServer`, `Service`, `LocalService`, `ServiceParameters`다. 특히 UMediaServer.exe의 UTF-16 문자열 file offset은 각각 `0x14a080`, `0x14a020`, `0x155c78`, `0x155c60`, `0x155c90`, `0x155a70`, `0x155aa8`. `LocalService` 문자열을 서비스 실행 계정이 LocalService라는 증거로 해석하면 안 된다. COM AppID 값 이름일 수도 있으며 호출 경로 확인이 필요하다.

### DLL/COM 등록

MSI `SelfReg`는 다음 13개 파일을 등록 대상으로 포함한다: UUserLog.dll, URTSPClient.dll, UDelegateGrabber.dll, UFileSource.dll, UTCPGrabber.dll, UMpeg2tsClient.dll, USplitter.ax, ULiveFileSource.dll, URTMPClient.dll, UPassGrabber.ax, UnrealWebRTC.dll, MediaFlipFlopPS.dll, USampleProvider.ax. `SelfRegModules` sequence 5600도 있다.

SDK의 별도 `ServerConfig64.dll`은 MSI에 포함된 `UServerConfig.exe` GUI와 다른 파일이다. SDK 문서는 x64 환경에서 `regsvr32`로 이 DLL을 등록하도록 명시하며 다음 COM 클래스를 공개한다(Readme.htm:743–772).

- CLSID `{C4DA49AC-71FD-4F53-B689-D230C0278A88}`
- ProgID `ServerConfig.Manager`, interface `IManager`

이 등록은 **자동 설정 스크립트를 선택할 때 필요한 SDK 의존성**이다. MSI 설치 후 GUI로 alias를 설정하는 경로라면 별도 SDK DLL 등록을 추가하지 않아도 된다. 그 GUI 경로의 실제 동작은 아직 시험하지 않았다.

### 파일과 레지스트리

- SDK 문서는 설정 변경이 `config.ums`를 읽고 쓰며 성공 후 서버에 변경을 알린다고 명시한다(Readme.htm:739–742,1185–1193; 파생 ServerConfig-readme.txt:8–11,226–232).
- 이 조사에서는 `config.ums`의 절대 경로·형식·원자적 갱신 방식은 확인하지 않았다. 임의 XML/INI를 만들어 넣지 않는다. SDK DLL의 `SYSTEM\CurrentControlSet\Services\` / `UMediaServer` / `config.ums` 문자열(file offsets `0x43260`, `0x43348`, `0x434c0`)은 service ImagePath를 통해 설치 경로를 찾을 수 있다는 가설의 seed일 뿐이다.
- MSI Registry 테이블은 `Root=2`(HKLM), `Software\Unreal\Server` 아래 `SSLPrivateKey=[TARGETDIR]unreal-key.pem`, `SSLCertificate=[TARGETDIR]unreal-cert.pem` 두 항목을 기록한다. private key 내용은 조사 보고서에 포함하지 않았다.
- 서버 PE에는 `SOFTWARE\UNREAL\SERVER`(`0x1572e8`), `SOFTWARE\WOW6432NODE\UNREAL\SERVER`(`0x157320`), `Software\Unreal\Server\TimeShift\Log`(`0x155ef0`) 등이 있다. 전체 schema/default 및 어느 hive/view를 쓰는지는 미확인이다.
- MSI의 `DIRCA_TARGETDIR` 기본식은 `[ProgramFiles64Folder][Manufacturer]\UMediaServer`. 기본 설치 디렉터리를 직접 추측하지 않고 VM 설치 로그/서비스 ImagePath로 확정한다.

## 합성 RTSP 영상 하나의 설정 계약

### 압축 영상 유지와 트랜스코딩을 구분할 근거

[공식 architecture 설명](https://umediaserver.net/umediaserver/architecture.html)의 media processing engine 절은 입력을 elementary stream으로 분리한 뒤 인코딩 상태를 유지하며 출력 프로토콜에 맞게 다시 포장한다고 설명한다. 서버가 decode/transcode하지 않는다는 **제조사의 명시적 설명**이다. 웹 문서는 `evidence/runtime-plan/official-architecture.html`로 보존했고 관련 파일 해시는 `source-hashes.json`에 있다. 현재 웹 설명이므로 사용자 MSI의 모든 분기를 입증하는 자료는 아니다.

별도 소스 근거도 같은 방향을 가리킨다. SDK `AddRTSPReBroadcast` 계약은 원본 stream 선택, transport, RTCP synchronization, realtime delivery를 받지만 **출력 해상도/fps/encoder/bitrate 설정을 받지 않는다**. 원본 WebRTC JS는 서버가 알린 codec을 받아 브라우저 offer payload를 고르며 H264 baseline SDP를 선택할 뿐 인코더 제어 명령을 보내지 않는다(`unrealwebrtcplayer.js:77–99,302–315`). 이 두 관찰은 원본 media path가 입력 코덱을 보존하는 구조라는 설명과 일치한다. 단, API에 인자가 없다는 사실만으로 내부 재인코딩이 절대 없다고 증명할 수는 없다.

사용자 MSI의 CHM `help/Overview.htm:27–31`은 Live Server/카메라 등 별도 encoder가 이미 인코딩한 데이터를 Media Server에 보낸다고 설명한다. CHM 전체의 `transcod`, `re-encod`, `reencod` 검색에서는 RTSP 입력에 대한 자동 변환 설명을 찾지 못했다. 이 bounded negative search를 전체 프로그램의 부재 증명으로 쓰지 않는다.

따라서 현재 정당한 구현 가설은 **H264 입력 → 원본의 압축 payload 유지 → RTP 또는 ISO-BMFF 재포장**이다. 기존 시제품의 libx264 강제 baseline/1280×720/30fps 변환은 원본에서 확인한 요구가 아니며 원본 이식 기준에서 제외해야 한다. 최종 입증은 아래 캡처 절차 7번의 NAL payload 비교와 native 함수 추적을 함께 사용한다. 원본이 지원하지 않는 입력 코덱을 자동 변환해 재생시키는 동작을 임의로 더하지 않는다.

SDK `IManager.AddRTSPReBroadcast`의 서명(Readme.htm:1144–1148):

```text
AddRTSPReBroadcast(
  BSTR sAlias, BSTR sRTSPURL, VARIANT_BOOL bRTCPSync,
  LONG nRTSPTransport, LONG nGetStreams, BSTR sUsername, BSTR sPwd,
  VARIANT_BOOL bRealTimeDelivery, VARIANT_BOOL bAllowSnapshots,
  UMediaServerResource * pNewResource)
```

첫 비교에 사용할 구체적 설정안:

| 값 | 선택 | 근거 / 의미 |
| --- | --- | --- |
| alias | `rea_camera` | 격리 실험에서 새 이름을 선택하는 설계값 |
| sRTSPURL | `127.0.0.1:18554/fixture` | **`rtsp://`를 포함하지 않는다.** SDK 문서:1154–1157, Form1.cs:112–114 |
| bRTCPSync | true | RTCP A/V synchronization. 영상만이어도 문서/예제 기본 경로 유지 |
| nRTSPTransport | 1 | RTSP over TCP; 2 UDP, 3 HTTP. 초기 캡처를 단일 TCP 흐름으로 단순화 |
| nGetStreams | 1 | video-only; 0 audio+video, 2 audio-only |
| username/password | empty | 로컬 테스트 source는 인증 없음; 실카메라 자격증명 불필요 |
| bRealTimeDelivery | true | realtime 모드를 첫 기준으로 선택 |
| bAllowSnapshots | false | Unreal Player용 옵션. WebRTC/MSE 실험에는 불필요 |

`UMediaServerResource`에는 아래 값을 명시한다. SDK의 VARIANT_BOOL 구조체 필드는 C# interop에서 short이므로 true는 `-1`, false는 `0`이다(Readme.htm의 구조체 규칙; 파생 ServerConfig-readme.txt:110–135).

```text
type = RTSPLiveBroadcast
bUnicast = -1
bMulticast = 0
bAllowNonUMSProtocols = -1
bAllowDelegates = 0
bNotifyArchivalServer = 0
bPlaylist = 0
nPlaylistType = 0
nTimeLimit = -1
nAuthType = 2                // anonymous
nUserGroup = 0
nTotalUsersLimit = 0
nAuthUsersLimit = 0
nThrLimit = 0
sDescription = "REA local synthetic RTSP fixture"
remaining strings = empty
```

원본 C# 예제는 `Form1.cs:80–118`에서 같은 타입을 생성해 위 메서드를 호출한다. 예제의 multicast true와 audio+video(0)를 이 계획에서는 multicast false, video-only(1)로 좁혔다. 이는 문서상 허용되는 **실험 설계**이며 이미 관찰한 원본 설정값이 아니다. 오류 `E_INVALIDARG`는 인자 검증 실패, `E_FAIL`은 config.ums I/O 또는 서버 통지 실패로 문서화되어 있다.

## VM 내부 최소 캡처 순서

1. 일회용 Windows x64 VM에 원본 MSI, SDK 플레이어, 고정 H264 fixture를 복사한다. 설치 전 snapshot을 만든다. 테스트 동안 외부/LAN에 게시하지 않고 VM 내부 loopback에서 source·server·browser를 연결한다. 개발 PC의 기존 WSL 데모와 포트를 공유하지 않는다.
2. 원본 MSI를 VM에서 설치하며 verbose MSI log를 저장한다. 설치 후 서비스 `UMediaServer`의 실제 이름, 실행 계정, ImagePath, start mode, COM 등록, 열린 포트, 생성된 파일을 기록한다. CHM/SDK가 설명한 경로와 일치하는지 비교한다. portable 실행은 별도 가설로 남기고 이 첫 캡처에서는 정식 설치 경로를 사용한다.
3. 고정 H264 elementary stream을 재생하는 **시험용 RTSP source**를 `127.0.0.1:18554/fixture`에 띄운다. 원본 서버 경로에는 외부 변환기를 넣지 않는다. source의 DESCRIBE SDP와 전달한 RTP packet/RTCP를 저장한다. 영상은 알려진 SPS/PPS, 주기적인 IDR, B-frame 없음으로 시작한다.
4. 원본 Configurator GUI에서 RTSP rebroadcast alias 하나를 위 계약으로 등록한다. 자동화가 필요하면 VM 안에서만 SDK ServerConfig64.dll을 등록하고 같은 계약의 x64 C# helper를 사용한다. 기존 sample exe를 그대로 누르지 않고 인자와 결과 HRESULT를 기록한다. 등록 전후 config.ums를 복사/해시 비교하여 위치와 변경 영역을 확정한다.
5. 서비스가 실행 중인지 확인하고 **원본 SDK 플레이어 코드는 수정하지 않은 채** sample 호출의 host/alias만 실험값으로 지정한다. MSE `ws://127.0.0.1:5119/rea_camera`; WebRTC `ws://127.0.0.1:5119/webrtc_playnow/singleport/udp/rea_camera`. WS 평문 loopback을 사용하므로 배포 인증서/private key는 불필요하다.
6. MSE는 WS binary message를 바이트 그대로 저장하고, WebRTC는 codec/SDP/candidate signaling 및 UDP 5135 흐름을 수집한다. 각각 첫 frame, 10초 지속, client 종료, 두 번째 시청자 join/leave를 반복한다. capture timing은 source time과 monotonic time을 함께 기록한다.
7. source RTP와 MSE `mdat`의 NAL payload를 byte-compare하여 패스스루/재포장 여부를 확인한다. WebRTC SRTP는 packet capture만으로 NAL을 읽을 수 없으므로 원본 DLL 호출 경계의 RTP/SRTP 전 데이터 또는 브라우저 수신 trace 등 추가 근거가 필요하다. 암호화된 wire packet을 plaintext와 단순 비교하지 않는다.
8. 최초 결과 후 TCP/UDP RTSP, malformed/중단 후 reconnect, WebRTC TCP 선택, source SPS/PPS 변경, real-time/buffered 전환을 독립 fixture로 확장한다. 처음부터 timeshift/오디오/다중 코덱 전체 복원 완료를 주장하지 않는다.
9. pcap/WS bytes, 로그, 설정 전후 해시, 설치 build identity, 브라우저 버전, fixture hash를 외부 결과 폴더로 반출한 뒤 VM snapshot을 되돌린다. 원본 DLL과 인증서·키는 새 Rust 서비스 패키지에 포함하지 않는다.

포트 근거는 CHM `Networking.htm`/파생 `Networking.txt:21–25`: unsecure signaling/MSE TCP 5119, TLS 443, WebRTC media 5135, RTSP **출력 클라이언트**용 554. RTSP source 입력 포트18554는 이 테스트가 선택한 값이며 554 출력 listener와 구분한다. UDP/TCP 5135 선택은 SDK `WebRTC/WebRTC_Player/play.html:34–38`에서 확인된다.

## 아직 풀리지 않은 시작 조건

- 정식 설치 helper들의 정확한 부작용, 최초 config.ums 생성 주체/위치.
- `-service`, `/RegServer`, 일반 인자 없는 실행 분기의 실제 의미와 console fallback 존재 여부.
- 등록 없이 DLL을 직접 생성할 수 있는지, RTSP/WebRTC 경로에 필요한 최소 COM 등록 집합.
- service ImagePath에서 config 경로를 찾는지, 사용자/ProgramData 폴더를 추가로 쓰는지.
- 해당 host에 사용할 수 있는 격리 Windows VM과 packet capture 수단의 실제 준비 여부.

이 항목들을 확인하기 전에는 현재 개발 PC에서 original exe를 실행하거나 서비스/레지스트리를 수작업으로 구성하는 계획으로 축약하지 않는다.
