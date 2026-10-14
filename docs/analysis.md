# UMediaServer MSI 분석과 Linux 구현 범위

> 이 문서는 최초의 불충분한 분석과 FFmpeg 시제품에 대한 기록이다. 이후 Ghidra 네이티브
> 분석 및 원본 DLL 직접 실행을 수행했다. 현재 조사 상태는 [원본 조사](original-investigation.md)를 따른다.

## 결론

요청 범위인 RTSP 카메라 → WebRTC → 웹 재생은 Linux Rust 서비스로 재구현할 수 있다.
현재 구현은 Rust가 HTTP/WebSocket, 인증, 세션, ICE/DTLS/SRTP/WebRTC와 fMP4 WebSocket 전송을 담당하고,
FFmpeg가 RTSP 수신, 필요시 H.264 인코딩과 fMP4 muxing을 담당한다. FFmpeg까지 Rust로 작성한
순수 Rust 구현은 아니다. 원본 실행 파일의 소스 코드를 복원하거나 자동 번역한
결과가 아니며, 원본 제품의 전체 기능이나 기존 플레이어 프로토콜 호환성을 주장하지 않는다.

## 관찰한 대상

- 사용자 파일: `D:\workspaces\UMediaServer(x64).msi`, 9,968,128 bytes.
- SHA-256: `513ffbda23167be0528dc6885a1420c5d3fde9ad1db4e094d4eeeecf0d6e1731`.
- MSI Property 테이블: Unreal Media Server x64 v15.0, UnrealStreaming, ProductVersion 15.0.
- 설치 프로그램과 대상 바이너리를 실행하지 않았다. Windows Installer 데이터베이스를
  읽기 전용으로 열고 내장 CAB을 추출한 뒤 `expand.exe`로 20개 파일을 풀었다.
- `scripts/extract-msi.ps1`과 `evidence/msi-*.json`에 재현 절차와 테이블이 있다.
- 원본은 `UMediaServer.exe`, `URTSPClient.dll`, `UnrealWebRTC.dll` 등으로 구성된다.
  원본 DLL, 실행 파일, 배포용 개인키를 새 서비스에 사용하거나 배포하지 않는다.

## REA 근거

REA 6.4.0 CLI에서 얻은 전체 결과를 `evidence/`에 보존했다.

| 파일 | 관찰과 근거 |
| --- | --- |
| `msi-rea.json` | MSI 직접 분석은 `unsupported binary format`으로 실패. MSI 추출은 REA 밖에서 수행. |
| `server-resources-rea.json` | UMediaServer.exe PE 리소스: `ev_1ef8d03ff682b41ae3a3a13c2d85a33205b0a89b888566f58386b5625030052e` |
| `webrtc-resources-rea.json` | UnrealWebRTC.dll PE 리소스: `ev_4e695b16d37d8f943e4d55986c96611e138576cd5d93c84ecc3d86355adb958d` |
| `webrtc-managed-rea.json` | `ev_6525040a615a47fafaef8232f2f0a6817e40dc38177d3fa0a83c147b14825848`: x64 PE, CLI 메타데이터 없음, `not-managed`. CIL/C# 복원 경로 사용 불가. |
| `webrtc-native-rea.json` | Ghidra/IDA 미설정으로 네이티브 디컴파일 실패. 함수 단위 복원·런타임 일치 여부는 미확인. |

추가 정적 관찰은 REA 근거와 구분한다. `scripts/inspect-pe.mjs`는 파일을 로드/실행하지 않고
PE import/export 테이블과 ASCII 문자열 위치를 기록한다. `server-pe.json`, `rtsp-pe.json`,
`webrtc-pe.json`에서 Win32/COM/WinSock 의존성과 COM DLL export를 관찰했다.
문자열은 기능의 단서일 뿐 실행 경로나 동작을 입증하지 않는다.

## 공개 규약과 설계 판단

[공식 SDK](https://umediaserver.net/umediaserver/source.html)의
[v15 SDK ZIP](https://umediaserver.net/bin/UMediaSDK.zip)에 읽을 수 있는 플레이어 JS와
HTML5 MSE Server protocol 문서가 있다. [공식 아키텍처](https://umediaserver.net/umediaserver/architecture.html)는
RTSP 입력, WebRTC 재생, MSE WebSocket 재생을 별도 경로로 설명한다.

- 원본 WebRTC 재생: `/webrtc_playnow/{singleport|randomport}/{udp|tcp}/{alias}`.
  최초 codec 문자열, JSON SDP offer, `answerJSON|-|-|candidateJSON` 교환.
- 원본 MSE 재생: WebSocket 바이너리로 초기화 XML/fMP4 및 미디어 fragment를 보낸다.
- 이 프로젝트: 자체 `/ws` JSON offer/answer 프로토콜과 함께 제공하는 플레이어를 사용한다.
  WebRTC 영상은 암호화된 SRTP로 전달한다. 별도 `/ws/mse`는 초기화 MP4와 완전한 fMP4 조각을
  WebSocket 바이너리 메시지로 전송한다. 원본 MSE의 독자 XML/프레임 헤더는 재현하지 않았다.

사용자는 입력·출력 범위를 “RTSP 카메라 입력 → WebRTC/웹 재생”으로 정했으며,
기존 프로토콜 호환성이나 원본 분석을 생략하도록 승인한 것은 아니다.
독립 구현 선택은 에이전트의 잘못된 범위 해석이었다. 현재 실행물은 별도 시제품이며
원본 분석 기반 이식 완료물로 취급하지 않는다. 2026-10-13 후속 작업에서는 원본 함수와
데이터 흐름을 추적하고 실제 근거에 따라 이식 의무를 다시 정한다.

## 지원과 제한

- H.264 영상 전용. 오디오, 녹화, 재송출, 관리 GUI, nDVR, 원본 SDK 호환은 지원하지 않는다.
- 기본 transcode 모드는 FFmpeg libx264로 H.264 baseline, 최대1280×720/30fps로 변환한다.
  입력 H.265 지원 여부는 설치한 FFmpeg 디코더에 달려 있다.
- copy 모드는 호환 H.264 입력에 한정한다. B-frame 없음, baseline 호환 프로필,
  SPS/PPS와 주기적인 IDR이 필요하다. 카메라에 RTCP PLI를 전달하지 않는다.
- WebRTC는 IPv4 UDP ICE. public_ip/포트 포워딩과 STUN 설정 가능. 인증된 TURN relay와 WebRTC TCP는 미구현.
  WebSocket MSE 영상은 HTTP/WSS 경로로 전달되므로 별도 UDP 경로를 요구하지 않는다.
- 카메라 장애 시 명시적인 오류와 세션 종료; 플레이어에서 다시 연결한다.
- 단일 공용 토큰 인증이며 사용자별 권한 관리 시스템은 아니다.
- 실제 카메라 검증 대신 사용자가 요청한 합성 RTSP 영상으로 검증한다.
  최종 실행 결과는 `docs/verification.md`에 기록한다.
