# 실행 검증 기록

검증일: 2026-10-13 (Asia/Seoul).

## 환경

- Linux: WSL2 Ubuntu 24.04, x86_64, glibc 2.39.
- Rust/Cargo: 1.99.0. `Cargo.lock` 고정.
- FFmpeg: Ubuntu 6.1.1, libx264.
- 합성 카메라: FFmpeg testsrc2, 640×360/15fps/H.264 → MediaMTX 1.21.2 RTSP/TCP.
- 브라우저: Windows Microsoft Edge, Playwright 1.58.2로 실제 브라우저 실행.
- 실제 카메라 대신 테스트 영상으로 먼저 완성하라는 사용자 선택에 따라 수행.

## 결과

| 검사 | 결과 | 근거 |
| --- | --- | --- |
| Rust 단위·HTTP·WebSocket 통합 테스트 | 9개 통과 | `test-results/rust-tests.txt` |
| Clippy 모든 타깃, 경고 오류 처리 | 통과 | `test-results/clippy.txt` |
| cargo fmt --check | 통과 | `test-results/format.txt` |
| Linux release 빌드 | 통과 | 패키지의 `uvms-gateway` ELF |
| WebRTC transcode | 실제640×360, 38프레임 확인, 중단·재연결 통과 | `test-results/webrtc-final/browser.json` |
| WebRTC copy | 실제 영상, 중단·재연결 통과 | `test-results/copy/browser.json` |
| WebSocket MSE transcode | 실제640×360, 42프레임 확인, 중단·재연결 통과 | `test-results/mse/browser.json` |
| WebSocket MSE copy | 실제640×360, 43프레임 확인, 중단·재연결 통과 | `test-results/mse-copy/browser.json` |
| WebRTC 동시2명 | 양쪽 디코딩, 1명 종료 후 다른 시청자 계속 재생 | `test-results/multiview.json` |
| MSE 동시2명 및25초 지속재생 | 프레임 증가, 버퍼 정리, 1명 종료 후 다른 시청자 계속 재생 | `test-results/mse-multiview/multiview.json` |

브라우저 성공 판정은 HTTP200이나 SDP 연결 여부만으로 하지 않았다. `videoWidth`, `videoHeight`,
실제로 디코딩된 영상 프레임 수, 재연결 후 추가 프레임 수를 확인했다.
토큰은 URL/localStorage/sessionStorage에 기록되지 않았다. 시험 종료 뒤 Linux 프로세스 목록에서
카메라 수신용 FFmpeg는 사라졌고 합성 영상 송출용 FFmpeg만 남은 것을 확인했다.

## 발견하고 수정한 문제

- FFmpeg6.1 RTSP demuxer는 `-rw_timeout`을 받지 않았다. demuxer가 제공하는 `-timeout`으로 고쳤고,
  실제 RTSP 수신·인코딩과 네 가지 브라우저 경로로 재검증했다.
- 브라우저 자동 favicon 요청의404를 제거했다. 실제 미디어 지속재생 검증은 이 수정 전에도 동작했으며,
  수정 후 최종 검사 기록을 보존했다.

## 배포 조건과 미검증 범위

- 제공 ELF는 Ubuntu24.04에서 빌드해 **glibc2.39 이상**이 필요하다. Ubuntu22.04/Debian12 등에서는
  대상 배포판에서 소스로 다시 빌드한다. Dockerfile은 빌드·실행 기반을 Debian bookworm으로 맞췄다.
- Docker 정의와 systemd unit을 제공하지만 실제 Docker 컨테이너 실행이나 systemd 설치는 검증하지 않았다.
  호스트 Docker Desktop은 기존 소켓 오류로 시작하지 못했고, 소켓 정리가 자동 승인 검토에서 차단돼
  별도 WSL Ubuntu 환경에서 Linux 실행을 검증했다.
- 실제 IP 카메라의 인증/펌웨어 차이, H.265 입력, 외부 NAT/방화벽/HTTPS, TURN, 여러 코덱,
  장시간 운영 및 대규모 부하는 검증하지 않았다.
- 원본 Windows 서버 실행이나 Ghidra/IDA 함수 디컴파일을 하지 않았으므로 원본과의 동작 동등성은 미확인이다.
- 오디오 및 원본 Unreal 전용 클라이언트 호환은 구현 범위 밖이다.

## 재현

`README.md`의 합성 RTSP 데모를 실행한 뒤 아래 명령을 별도 터미널에서 실행한다.

```bash
npm ci
npx playwright install chromium
UVMS_TEST_MODE=webrtc UVMS_TEST_STREAM=camera UVMS_TEST_OUTPUT=test-results/webrtc-final npm run test:browser
UVMS_TEST_MODE=webrtc UVMS_TEST_STREAM=camera_copy UVMS_TEST_OUTPUT=test-results/copy npm run test:browser
UVMS_TEST_MODE=mse UVMS_TEST_STREAM=camera UVMS_TEST_OUTPUT=test-results/mse npm run test:browser
UVMS_TEST_MODE=mse UVMS_TEST_STREAM=camera_copy UVMS_TEST_OUTPUT=test-results/mse-copy npm run test:browser
UVMS_TEST_MODE=mse UVMS_TEST_OUTPUT=test-results/mse-multiview node scripts/browser-multiview.mjs
```

기본 테스트 토큰은 loopback 합성 데모용 값이다. 실제 서비스 토큰은 `.env.example`을 참고해 새로 생성한다.
