# 직접 접속 및 성능 측정 — 2026-10-13

## 적용 상태

웹 API, WebSocket 영상, 녹화 목록·다운로드, RTSP 재송출의 서비스 토큰 인증을 제거했다. `/api/login` 처리기·인증 쿠키 발급·토큰 입력란·토큰 환경변수도 제거했다. 입력 카메라의 아이디·비밀번호를 사용하는 Basic/Digest 연결은 유지했다.

- 웹: http://127.0.0.1:8080/ — `camera_live` 자동 재생.
- 재송출: `rtsp://127.0.0.1:8555/camera_live` — 사용자·암호 없음.
- 새 프로세스 환경에 UVMS_TOKEN 없음. 무인증 카메라 목록·녹화 목록·MP4 다운로드200, Set-Cookie 없음.
- 서비스 테스트11개와 clippy 통과. 실제 재송출은 인증 없는 Rust 클라이언트16개 모두100frames 수신.

## 측정 조건

AMD Ryzen 7 PRO 5750GE, WSL2 Ubuntu24.04,16logical CPUs. 서비스는 release Rust 바이너리, 브라우저는 같은 Windows 호스트의 headless Edge다. 실제 LAN 카메라1대의1920×1080 H264 약10fps·약4.5Mbps 영상을 여러 클라이언트에 전달했다. 실카메라 녹화는 꺼져 있다. FFmpeg는 이 영상 수신·전달에 사용하지 않았다.

각 방식에서1→4→9→16개 브라우저를 순서대로 연결했다. 초기 연결과2초 준비 시간을 제외하고 단계마다20초 측정했다. 측정 중 카메라 입력 TCP 연결은 항상1개였다. 서버 CPU100%는 논리 코어 하나를 가득 사용한 값이다. CPU와 메모리는 uvms 프로세스만 측정했으며 Windows 브라우저 자원 사용은 포함하지 않는다. 메모리는 동일 프로세스에서 순차 측정한 RSS이므로 이전 단계의 할당 메모리가 남아 있을 수 있으며, 각 단계의 독립적인 메모리 소모량이 아니다.

WebRTC FPS는 framesDecoded, MSE FPS는 totalVideoFrames-droppedVideoFrames의 증가율이다. 표시 누락 열은 브라우저 renderer droppedVideoFrames의 합이다. 전송 Mbps는 클라이언트가 받은 영상 bytes의 합으로 물리 NIC의 전체 wire 대역폭과 다르다.

| 방식 | 동시 시청자 | 평균 / 최저 FPS | CPU 평균 / 최고 | RSS 최대 MiB | 수신 합계 Mbps | 표시 누락 frames |
|---|---:|---:|---:|---:|---:|---:|
| webrtc | 1 | 9.94 / 9.94 | 3.8% / 8.0% | 41.6 | 4.5 | 0 |
| webrtc | 4 | 9.94 / 9.94 | 10.1% / 13.9% | 98.2 | 18.1 | 17 |
| webrtc | 9 | 9.94 / 9.94 | 19.6% / 38.8% | 187.6 | 41.4 | 14 |
| webrtc | 16 | 9.94 / 9.94 | 29.8% / 39.8% | 268.7 | 72.3 | 0 |
| mse | 1 | 9.94 / 9.94 | 5.9% / 9.9% | 273.8 | 4.6 | 0 |
| mse | 4 | 9.78 / 9.64 | 5.9% / 7.0% | 270.7 | 18.1 | 17 |
| mse | 9 | 9.99 / 9.99 | 7.8% / 9.0% | 269.6 | 41.1 | 0 |
| mse | 16 | 9.93 / 9.49 | 12.8% / 15.9% | 272.9 | 73.6 | 20 |

WebRTC의 모든 단계에서 수신 패킷 손실은0이었다. MSE16클라이언트 단계에서 표시 누락 합계는20frames, 가장 낮은 클라이언트는 약9.49fps였다. 모든 클라이언트가20초 동안 계속 프레임을 처리했다. 데이터의 passed는 연결·프레임 진행 검사 통과이며 운영 SLA나 최대 처리량 판정이 아니다.

## RTSP 재송출

별도 단계에서 Rust RTSP 클라이언트16개가 사용자·암호 없는 URL에 동시에 연결했다. 각100frames, 총1600frames 수신·AVCC 경계·timestamp 검사를 통과했다. 시작부터 종료까지10.04초, 서버 CPU 평균6.9%/코어, RSS 최대270.8MiB, AVCC payload 합계73.6Mbps였다. 이 단계는 초기 연결 시간을 포함하며 디코더·렌더러 성능 시험은 아니다.

## 범위와 재현

카메라1대를16명이 보는 짧은 부하 시험이다. 서로 다른 실제 카메라16대 입력, 장시간 녹화 동시 부하, 공인망/NAT, 최대 접속자 수, 실제 영상의 종단 지연은 아직 측정하지 않았다. 카메라 영상이나 스크린샷은 성능 결과에 저장하지 않았다.

- Windows: `node scripts/benchmark-viewers.mjs` (Edge·Playwright 및 WSL의 설치된 uvms 서비스 필요).
- 측정 시간·접속 수: `UVMS_BENCH_SECONDS` 기본20, `UVMS_BENCH_COUNTS` 기본1,4,9,16.
- Linux: `python3 scripts/benchmark-relay.py` (기존 `/root/uvms-rtsp-target/debug/examples/capture` 클라이언트 필요).
- 원시 결과: `test-results/performance/viewers.json`, `service-samples.json`, `relay.json`.
- 인증 제거 API 확인: `test-results/direct-access/api.json`.
