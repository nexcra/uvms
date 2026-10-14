> 2026-10-13 후속 구현: 아래 분석 당시의 미이식 항목 중 RTSP 수신, WebRTC/MSE 연결, MP4 녹화, RTSP 재송출은 현재 Rust 서비스에 연결됐다. 최신 구현 범위는 [README](../README.md), 실제 테스트는 [native-service-verification](native-service-verification.md)을 기준으로 한다. 아래 원본 분석의 근거·당시 범위는 보존한다.
# 원본 분석 기반 이식 조사

검토일: 2026-10-13. 기존 FFmpeg 시제품은 원본 이식 결과가 아니다.
이번 조사는 원본 EXE/DLL의 함수 분석, 원본 DLL 실행, 그 근거에 대응하는 첫 Rust
이식 부품을 구분해 기록한다. **전체 Linux 서비스 이식 완료를 뜻하지 않는다.**

## 질문과 필요한 근거

| 질문 | 필요한 근거 | 상태 |
| --- | --- | --- |
| 원본 RTSP 수신과 H.264 프레임 전달 경로 | URTSPClient.dll 함수, 호출/데이터 흐름, COM 경계 | TCP/H264 핵심 경로 분석 및 원본 DLL에서 40프레임 수신 확인. UDP/auth/reconnect 등은 미검증 |
| 원본 WebRTC 협상·패킷화·암호화·전송 | 서버와 UnrealWebRTC.dll 함수 및 인터페이스 | 압축 프레임 passthrough 연결과 SDP 경로 확인. 원본 DLL을 통한 실제 Edge 재생 확인. libwebrtc 전체 내부 복원은 아님 |
| 원본 WebSocket/MSE 초기화와 프래그먼트 구조 | 서버 함수와 공식 SDK 파서의 상호 일치 | header 생산자/소비자 대조, MP4 mux 핵심 함수 확인. 원본 MSE wire capture와 전체 mux 포트는 미완료 |
| Windows 종속 부분과 Linux 대체 범위 | API 호출 및 객체 경계 | COM/서비스/IOCP/allocator/thread 경계 식별. Linux 서비스 구현은 미완료 |
| 기존 플레이어와 실제 동작 동등성 | 원본 실행 캡처와 호환성 테스트 | 원본 DLL+별도 브라우저 probe만 확인. 원본 EXE/수정 없는 SDK 플레이어/신규 Linux 서비스의 3자 비교는 미완료 |
| Rust 이식 구현 | 위 근거에 연결된 코드·테스트 | `ports/unreal-core`: 프레임 normalizer 및 MSE payload header만 이식. Linux 8개 계약 테스트+실제 원본 캡처 입력 테스트1개 통과 |

## 조사 환경

- REA 6.4.0, Ghidra 12.1.4, 기존 Temurin JDK25 사용.
- Ghidra 공식 배포 ZIP SHA256: `ddac49f903da9d5bac833e5cc79395098b9c33cfd3279be5f31bd00387d2d4db`.
- MSI와 추출 바이너리는 이전 식별 결과를 재사용한다. EXE는 정적 분석만 했다.
  원본 RTSP/WebRTC DLL은 해시를 확인하고 제한시간이 있는 별도 프로세스에서 직접 실행했다.
  MSI 설치, COM 등록, Windows 서비스 설치는 수행하지 않았다.
- `scripts/rea-session.mjs`가 REA MCP 세션을 유지하고 요청·전체 응답을 `evidence/native/`에 저장한다.
- 정적 함수 분석은 실행 관찰이 아니다. 디컴파일 의사코드는 원본 소스가 아니다.
- 원본 파일 SHA256 불변 확인: `evidence/native/source-integrity-review.json`.

## 결과 문서

- `original-protocol.md`: 공식 SDK 플레이어의 정확한 통신 규약.
- `original-rtsp-native.md`: RTSP DLL 함수와 프레임 전달 경로.
- `original-webrtc-native.md`: WebRTC DLL 함수와 전송 경로.
- `original-server-native.md`: 서버의 RTSP→NAL 변환→RTC interface와 MSE 생성 함수.
- `original-runtime-plan.md`: 원본 전체 서버를 격리 Windows 환경에서 실행할 때의 구성·캡처 절차.
- `../ports/unreal-core/README.md`: 원본 주소별 첫 Rust 이식 코드와 명시적 차이/검증.

## 지금 근거로 말할 수 있는 결론

원본 DLL의 live 경로도 확인했다. 합성 RTSP → 원본 `URTSPClient.dll` → 서버 함수에서
복원한 NAL 정규화 → 원본 `UnrealWebRTC.dll` → 실제 Edge에서 **640×360 영상 31프레임**을
디코딩했다. 해당 관측 구간에서 packet loss/frame drop은0, keyframe3개, ICE/DTLS 연결 성공이다.
RTSP Init/Stop과 RTC media Init/InitClient/Terminate는 모두 S_OK였다.
근거: `evidence/runtime/original-live/{browser.json,result.json,stdout.txt,run.json}`.
FFmpeg는 외부 테스트 카메라 생성기에만 사용했으며 이 수신·송출 프로세스의 처리 엔진이 아니다.
브라우저는 SDK의 SDP 조건을 따른 최소 probe다. 원본 서버 EXE의 WebSocket 라우팅을 실행하거나
수정하지 않은 SDK 전체 플레이어를 실행한 검증으로 확대 해석하지 않는다.

원본의 확인된 H264 경로는 RTSP로 받은 압축 NAL을 모아 전달하며, 서버가 길이 접두사를
시작 코드로 바꾸고 NAL offset/length index를 붙여 WebRTC DLL의 callback으로 넘긴다.
이 경로를 만들기 위해 FFmpeg로 재인코딩할 필요는 없다. 원본 역시 이 조사에서 확인한
수신·정규화·송출 함수 경로에는 transcoder를 두지 않는다.

Windows DLL을 Linux에서 직접 재사용하는 포트가 아니라, 확인한 프레임 계약과 프로토콜을
유지하고 COM/IOCP 등 OS 경계를 대체하는 이식이 필요하다. 아직 복원하지 않은 libwebrtc
세부 동작이나 mux 오류 경로를 임의 구현한 뒤 원본과 같다고 보고하지 않는다.

## Linux 서비스 완료 전에 필요한 작업

1. RTSP control/auth/SDP/RTP depacketizer와 timestamp/queue 동작 이식 및 원본 캡처 대조.
2. 원본 MP4 init/fragment mux 필드, 타임스탬프 및 keyframe 경계 대조·이식.
3. codec-first signaling과 RTC 세션 수명, original MSE 명령 처리 구현.
4. WebRTC 전송 엔진 대체 지점의 RTP/RTCP·ICE·DTLS·SRTP 호환 검증.
5. 수정하지 않은 원본 SDK 플레이어로 새 Linux 서비스 재생·재연결·다중시청 검증.

기존 `dist/` 실행물이나 localhost:8080 화면은 앞선 FFmpeg 시제품이다. 이번 원본 DLL
실행 probe와 Rust 부분 이식 결과를 해당 실행물의 기능으로 혼동하지 않는다.

