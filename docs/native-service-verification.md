# Rust native 서비스 검증 — 2026-10-13

최신 변경: 웹·WebSocket·녹화 다운로드·RTSP 재송출의 서비스 토큰 인증과 세션 쿠키를 제거했다. 입력 카메라의 Basic/Digest 인증은 유지한다. 아래 인증·local_access 검증은 변경 전 역사적 기록이며 현재 접속 절차가 아니다. 현행 검증과 성능 수치는 `docs/performance.md` 참조.

검증 대상은 현재 uvms-gateway0.2.0이며 이전 FFmpeg gateway0.1.0 결과와 구분한다. 실제 서비스는 WSL Ubuntu-24.04 systemd, 영상 클라이언트는 Windows Edge/Playwright다. 아래 기본 검증은 합성 카메라640×360/15fps/H264 constrained baseline을 사용했다. 실제 LAN 카메라 후속 검증은 마지막 절에 기록했다. 공인망 NAT·장시간 부하 시험은 하지 않았다.

## 원본 근거

- MSI·native pseudocode·SDK 메시지·원본 DLL live replay 근거는 original-investigation.md와 각 native 문서에 연결되어 있다.
- 원본 URTSPClient.dll 출력40frames를 Rust normalizer에 넣어204NAL,420361payloadbytes 및 timestamp/keyframe 보존을 검증했다.
- 원본 AP4/libwebrtc 전체 소스 복원이나 전체 프로그램의 동등성을 주장하지 않는다. 압축 media flow와 관측된 wire 계약을 구현했다.

## 완료한 검증

| 항목 | 결과 | 파일 |
|---|---|---|
| Rust 검사 | 서비스11, RTSP12, core8, MP4 4 테스트 통과. 별도 원본 캡처1개 통과. fmt/clippy 경고0 | evidence/port/native-service-tests.txt |
| 원본 WebRTC SDK | 수정하지 않은 SDK, candidate 없는 offer, 원본 delimiter응답. signaling WS 종료 후5.2초 계속 재생.640×360,35→107frames,drop0 | evidence/runtime/rust-sdk-final/browser.json |
| 원본 MSE SDK | 수정하지 않은 SDK, 원본 binaryheader로 재생.35→105 totalframes. renderer dropped14→29 관측, JS오류·decodererror없음 | evidence/runtime/rust-sdk-final/mse.json |
| 자체 UI | WebRTC와 MSE 재생/연결해제/재연결,640×360 실제 재생, 토큰URL/localStorage 노출없음 | test-results/native-webrtc, native-mse |
| RTSP 재송출 | 직접수신80frames와 재송출80frames AVCC SHA256 전부동일. SPS/PPS동일. ffprobe46frames디코드. 인증401/200, UDP461 | evidence/port/relay-verification.json |
| 실제 녹화 다운로드 | 웹 버튼으로 받은 MP4:10초,150frames,640×360 H264, decodererror없음 | test-results/native-systemd/downloaded.mp4 |
| 저장용량 제한 |1MiB 예산에서1,041,859bytes 기록 후storage_full.6초간 파일수·용량불변, 빈MP4없음.4files96frames전부디코드 | evidence/port/recording-verification.json |
| 인증 다운로드 | 인증없이401, 인증후200, 내려받은bytes와원본파일일치 | 같은 파일 |
| 정상 종료 | 활성.partial SIGTERM후 유효MP4로 마무리, 소유테스트프로세스exit0 | 같은 파일 |
| 카메라 장애 | source중단→onlinefalse/retrying→source복구→online/recording.서비스PID유지 | evidence/port/service-lifecycle.json |
| systemd 재시작 | PID변경, 녹화중파일완료처리, 재연결및녹화재개 | 같은 파일 |
| 외부 미디어 의존성 | 서비스자식프로세스0. 서비스mountnamespace에서/usr/bin/ffmpeg 실행불가 상태로 재생·녹화 | 같은 파일 |
| MSE 명령 |pause중전송정지, continue/switch각각이전sampleend에서IDR재개,52media전체tfdt와envelope일치 | evidence/runtime/mse-commands/result.json |

원본 MSE SDK의 headless 실행에서는 renderer droppedFrames가 증가했다. 따라서 MSE 무손실 화면 표시나 일정 지연을 보장한다고 표현하지 않는다. RTSP 압축 payload 보존 검증과 브라우저 renderer drop은 별개의 결과다.

## 배포

- /opt/uvms/uvms-gateway, uvms.service 활성화. 계정uvms, 설정/etc/uvms, 녹화/var/lib/uvms/recordings.
- systemd unit이 FFmpeg/ffprobe 파일 접근을 차단한다. FFmpeg는 별도 uvms-demo-source에서 테스트 영상을 만드는 용도로만 실행된다. MediaMTX는 테스트 RTSP 발행 endpoint다.
- 테스트 서버는 [고정버전 설정](https://raw.githubusercontent.com/bluenviron/mediamtx/v1.21.2/mediamtx.yml)을 확인하여 MoQ등 부가서비스를 끄고127.0.0.1:8554만 listen하도록 검증했다.
- 서비스 HTTP8080/RTSP8555는 현재loopback. 외부카메라/NAT 환경은 배포별IP·방화벽·TLS설정 후 검증해야 한다.
- Dockerfile은 FFmpeg없이 제공하며 Docker실행 자체는 이 환경에서 검증하지 않았다.
- WSL을 종료하면 서비스도 종료된다. WSL시작 시 enabled서비스가 시작되며, 현재 Codex의 journalctl 세션이 WSL실행을 유지한다.

## 의도적으로 제외한 범위

오디오, H265, B-frame MP4 CTS, RTSP/UDP·RTSPS, WebRTC/TCP, 원본 단일포트공유엔진, 원본sid인증DB, VOD seek·time-shift, RTMP/MPEG-TS/SRT 입력은 구현완료로 주장하지 않는다. 원본 SDK와 영상코덱 호환을 우선하려면 H264 baseline 및 B-frame0 카메라 설정을 사용한다.

추가 동시접속 검증: 최종 서비스에서 MSE 클라이언트2개를25초 유지했다. 각각263/273 decodedframes가 증가했고, 각 버퍼 범위는10.73초로 제한됐다. 한 클라이언트를 끊은 후 나머지 클라이언트는33frames를 더 디코드했다. test-results/native-multiview/multiview.json 참조.

## 다중 카메라 화면 후속 검증

2026-10-13, 서로 다른 두 RTSP 발행 source(camera15fps, camera_two10fps)로 수행했다. 한 source를 두 번 보여 주는 시험과 구분한다.

- WebRTC·MSE 각각 두 source640×360 동시 재생.
- 두 번째 source만 systemctl stop/start로 중단: 첫 번째 카메라는 기존 WebSocket/PeerConnection을 유지하면서 디코드 프레임이 증가했다. 두 번째만 자동 복구했다.
-4→9→16→4 배치 변경과14초 확대 보기에서 재연결이 발생하지 않았다.
- 개별 재연결, 전체 해제, 재생 중 선택 추가/제거, 단일↔분할 전환 모두 통과했다.
-1440/1100/900/390폭에서 가로 넘침 없음. umbra 구조·접근성·대비·반응형 검사 통과, 실제 screenshot도 확인했다. 값 감사의 유일 예외931px는 기존930px 분기 다음 픽셀이라는 주석이 있는 기존 값이다.
- camera_two의 분할 녹화도 ffprobe100frames640×360 H264 디코드 확인.
- 기존 단일 WebRTC 재생·해제·재연결 회귀 검사 통과.

결과: test-results/camera-wall/result.json, layout.json, final.png. 재현: scripts/browser-camera-wall.mjs, scripts/browser-wall-layout.mjs. 이 시험은 합성 source2개를 검증했으며16대 처리량 검증은 별도다.

## 실제 LAN 카메라 후속 검증

사용자가 제공한 카메라를 `camera_live`로 등록했다. URL은 서버의 `/etc/uvms/uvms.env`에만 저장하고 파일 권한을0600으로 설정했다. 자격증명은 소스·보고서·배포 압축파일에 포함하지 않는다. 자동 녹화는 현재 꺼져 있다.

- Rust RTSP 직접 수신: H2641920×1080, 약10fps.40frames·IDR4개 수신, AVCC와 timestamp 검사 통과.
- WebRTC 분할 화면: 실제 카메라35→95 decodedframes, 동시에 합성 카메라42→125frames. 실제 카메라 개별 재연결 통과.
- WebSocket(MSE) 분할 화면: 실제 카메라40→99 decodedframes, 동시에 합성 카메라46→115frames. 실제 카메라 개별 재연결 통과.
- Rust RTSP 재송출: 실제 카메라20frames·IDR2개,1920×1080 수신 통과.
- Rust MP4 임시 녹화:2.01초20frames,1920×1080 H264,1,094,253bytes. 서비스 외부에서 읽기 전용 ffprobe 디코드 검사 통과. 녹화 프로세스의 자식 프로세스0개, 정상 종료exit0.
- 임시 녹화 영상은 검사 후 삭제했고 실제 카메라 화면의 스크린샷은 저장하지 않았다. 서비스의 FFmpeg 호출이나 카메라 인코딩 설정 변경은 없었다.

결과: `test-results/real-camera/playback.json`, `test-results/real-camera/recording.json`. 실제 카메라1대와 합성 카메라1대의 동시 재생 결과이며, 여러 실제 카메라의 장시간 부하를 검증한 것은 아니다.

## Local access / automatic playback

The deployed loopback HTTP service now uses `local_access=true` and `default_stream="camera_live"`. Opening or refreshing the page starts the configured real camera without token entry or a manual list request. WebRTC, MSE, multi-camera playback and recording-list access passed with a fresh browser context and an empty token field (`test-results/local-access/result.json`). The token-authenticated mode remains available when local access is disabled. Local access rejects non-loopback binds, untrusted Host, cross-origin and cross-site requests. Root service tests:13 passed; clippy clean. RTSP relay credentials remain separate from the browser flow.
