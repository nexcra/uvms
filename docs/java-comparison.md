# Rust / Java 미디어 서비스 비교 — 2026-10-13

이 문서는 공유 UDP·버퍼 최적화 **이전** 빌드의 실제 카메라 비교 기록이다. 2026-10-14 Java 변경과 테스트 영상 기준 전후 결과는 `java-optimization.md`를 참조한다. 두 시험은 입력이 다르므로 숫자를 직접 합쳐 비교하지 않는다.

Java 21에서 RTSP 입력 → H.264 분배 → WebRTC, 원본 WebSocket/fMP4, MP4 녹화, RTSP 재송출을 구현했다. 코드·빌드 방법·제한은 `java/README.md`를 참조한다. 접속 토큰을 추가하지 않았으며 카메라 자체 인증만 사용한다. Java와 Rust 서비스 모두 FFmpeg를 미디어 처리에 호출하지 않는다.

## 측정 조건

- 동일 AMD Ryzen 7 PRO 5750GE, Windows/WSL2 Ubuntu 24.04, 논리 CPU 16개.
- Rust: 기존 release 바이너리와 별도 `uvms-rust-bench` 인스턴스, HTTP8089/RTSP8559. 실제 사용 중인 8080 서비스는 유지.
- Java: OpenJDK21, `-Djava.net.preferIPv4Stack=true -Xms64m -Xmx512m`, `uvms-java`, HTTP8088/RTSP8558. 최종 빌드 JVM 예열은 16명 WebRTC와 MSE 각각 30초 + 5초 표본이며 정식 결과에서 제외.
- 같은 실제 카메라 `camera_live`, H.264 Main 1920×1080 약 10fps. 실제 수신량은 시청자당 약2.2~2.5Mbps였다. 장면과 측정 시점 때문에 순차 측정의 전송량이 완전히 같지는 않다.
- 같은 웹 코드와 Windows headless Edge. 1→4→9→16개 독립 브라우저 페이지를 연결하고 프레임 도착 및 3초 준비 후 단계별20초 측정. 프로토콜 순서는 WebRTC→MSE. 서버끼리 동시 부하를 걸지 않았다.
- 이 입력의 녹화는 끔. 서버 CPU/RSS만 측정하며 Windows 브라우저 비용은 제외한다. CPU100%는 논리 코어 하나의 사용량이다. RSS는 JVM·직접 버퍼·네이티브 메모리를 포함한 전체 프로세스의 상주 메모리다. 단계별 재시작을 하지 않으므로 이전 할당이 남을 수 있다.
- WebRTC FPS는 `framesDecoded`, MSE FPS는 `totalVideoFrames-droppedVideoFrames` 변화량. 표시 누락은 브라우저 렌더링 카운터이며 WebRTC RTP 손실과 별도다. Mbps는 클라이언트 수신 payload 합계이며 NIC wire 대역폭이 아니다.

## 최종 결과

복원된 H.264 입력으로 Java 전체 측정 후 Rust 전체 측정을 실행했다. 아래는 각 서버16명 동시 접속의 실제 수치다. CPU100%는 논리 코어 하나를 가득 사용한 값이다.

| 방식 | Rust CPU 평균 | Java CPU 평균 | Rust RSS 최대 MiB | Java RSS 최대 MiB | Rust / Java 평균 FPS |
|---|---:|---:|---:|---:|---:|
| webrtc | 16.2% | 14.1% | 184.0 | 367.4 | 9.95 / 9.99 |
| mse | 4.8% | 12.9% | 199.6 | 367.9 | 9.92 / 9.98 |
| RTSP/TCP | 2.0% | 6.7% | 199.6 | 368.2 | 각100frames ×16 수신 |

이 시험의 WebRTC CPU는 Java가 조금 낮았다. MSE와 RTSP CPU 및 세 방식의 메모리는 Rust가 낮았다. 두 서비스 모두 브라우저 영상이 약10fps로 진행됐다. 서버별 카메라 입력 TCP 연결은 모든 브라우저 단계에서1개였다.

### 1·4·9·16명 상세

각 쌍의 값은 Rust / Java 순서다.

| 방식 | 시청자 | CPU 평균 % | RSS 최대 MiB | 평균 FPS | 최저 FPS | 수신 합계 Mbps | 표시 누락 frames |
|---|---:|---:|---:|---:|---:|---:|---:|
| webrtc | 1 | 1.1 / 2.7 | 22.6 / 367.3 | 9.94 / 9.95 | 9.94 / 9.95 | 2.2 / 2.3 | 0 / 5 |
| webrtc | 4 | 3.5 / 4.1 | 50.9 / 367.6 | 9.99 / 9.95 | 9.99 / 9.95 | 9.0 / 9.0 | 0 / 2 |
| webrtc | 9 | 8.5 / 8.3 | 112.3 / 374.0 | 9.98 / 9.97 | 9.94 / 9.94 | 20.2 / 20.0 | 8 / 10 |
| webrtc | 16 | 16.2 / 14.1 | 184.0 / 367.4 | 9.95 / 9.99 | 9.94 / 9.95 | 34.5 / 37.8 | 0 / 10 |
| mse | 1 | 2.1 / 3.0 | 198.9 / 367.5 | 9.94 / 9.99 | 9.94 / 9.99 | 2.2 / 2.4 | 0 / 0 |
| mse | 4 | 1.4 / 3.9 | 199.1 / 367.6 | 9.99 / 9.94 | 9.99 / 9.94 | 9.3 / 9.3 | 0 / 0 |
| mse | 9 | 2.7 / 6.9 | 199.3 / 367.6 | 9.81 / 9.97 | 9.79 / 9.79 | 20.2 / 20.1 | 0 / 4 |
| mse | 16 | 4.8 / 12.9 | 199.6 / 367.9 | 9.92 / 9.98 | 9.49 / 9.94 | 36.9 / 36.6 | 25 / 3 |

WebRTC의 RTP 수신 패킷 손실은 두 서버의 모든 단계에서0이었다. 표의 표시 누락은 브라우저 렌더러 카운터로, 모든 프레임이 화면에 표시됐다는 의미는 아니다.

### RTSP 재송출

16개 Rust capture 클라이언트를 각각의 재송출 서버에 붙여 클라이언트당100frames, 총1600frames 수신·AVCC 경계·timestamp 검사를 통과했다. 저장하거나 화면 렌더링하지 않았다. 이 구간은 초기 연결 및 첫 키프레임 대기를 포함하므로20초 브라우저 구간과 측정 방식이 다르다.

Rust: 10.04초, CPU 2.0%/코어, RSS 199.6MiB, AVCC payload 합계 35.8Mbps. Java: 9.29초, CPU 6.7%/코어, RSS 368.2MiB, payload 38.6Mbps.

### 측정 빌드

- Rust SHA256: `0665f8cac1ccd4eb0262b5fa8254a5e3bed3de53654540c644c36d5992cc2ff3`.
- Java SHA256: `171b2a3a25cf025e79654d8c0ec1aa48577701c561e14eb85c9770ae612595e2`.
- Java 브라우저 측정: 2026-10-13T14:42:46.699Z ~ 2026-10-13T14:46:49.614Z.
- Rust 브라우저 측정: 2026-10-13T14:47:00.860Z ~ 2026-10-13T14:51:05.614Z.

## 기능 검증

- Java JUnit5 테스트5개: RFC3711 키 파생 벡터, SRTCP 변조/재전송 거부, SPS/AVCC 경계, 원본 MSE 프레이밍과 MP4 타임라인, RTP FU-A 재조립.
- 실제 카메라 WebRTC와 MSE 1920×1080 재생 및 재접속.
- 토큰 입력 없는 자동 재생/새로고침, 실제 카메라와 별도 테스트 카메라의 동시 화면, 녹화 목록 접근.
- 원본 MSE의 `pause`/`continue`/`switch` 명령: pause 중1초 무전송, 재개·전환 후 IDR 시작 및 연속 MP4 타임라인 검사 통과.
- WebRTC offer를 보내기 전에 연결을 끊는 경우80회 반복 통과. 세션 제한32개가 누적 소진되지 않음을 확인했다.
- Java RTSP 재송출의 무인증 연결과 H.264 프레임 수신.
- Java MP4: 실제 카메라 2.01초/20frames, H.264 1920×1080 읽기 검증, 다운로드200 및 파일 일치. 정상 종료 후 partial0. 검증 파일은 제거했으며 서비스 자식 프로세스는 없었다. ffprobe는 서비스 외부에서 검사에만 사용.
- 변경하지 않은 원본 SDK: Baseline 640×360 테스트 영상에서 candidate 없는 offer의 WebRTC와 MSE 모두5초 이상 프레임 진행. 실제 Main 카메라는 원본 SDK가 Baseline만 제안하여 WebRTC 협상 실패. 자체 웹 화면은 실제 카메라 정상 재생. 이 제한을 성공으로 합산하지 않는다.

## 해석 범위

첫 Java 측정 도중 카메라가 H.264에서 H.265로 변경됐다. 같은 시각(14:24:44 UTC)에 기존 Rust 서비스도 입력 종료를 기록했으며, 외부 메타데이터 검사에서 HEVC 1920×1080을 확인했다. 중단된 MSE9 데이터의 음수 카운터는 플레이어 초기화로 인한 무효 값이며 성능 결과에서 제외했다. 해당 실행은 `test-results/java-comparison/java-interrupted`에 보존했다. 사용자가 H.264 복원을 선택했으며, H.264 Main 1920×1080 복원을 확인했다. 전송량이 이전 약4.5Mbps에서 약2.2~2.5Mbps로 달라져 두 서버를 모두 재측정했다. 변경 전 Rust 결과도 `rust-before-source-change`에 보존했으며 위 최종 표에는 섞지 않았다.

이 비교는 현재 두 구현의 짧은 전달 부하 비교다. Rust는 webrtc-rs, Java는 자체 ICE-lite/SRTP와 Bouncy Castle DTLS를 사용하므로 동일 알고리즘을 언어만 바꾼 실험이 아니다. Java에는 전체 ICE/TURN·혼잡 제어·오디오 등이 없으며 둘 다 영상 재인코딩 없이 전달한다. 결과로 Java/Rust 언어 전체나 go2rtc의 우열을 단정할 수 없다.

서로 다른 실제 카메라16대, 장시간 녹화 동시 부하, 최대 접속 수, 인터넷 NAT, 심한 패킷 손실, 실제 영상의 종단 지연은 이번 표본으로 검증하지 않았다. `passed`는 연결과 프레임 진행 검사 통과를 뜻한다.

## 재현

Windows PowerShell 예시:

```powershell
$env:UVMS_BENCH_URL='http://127.0.0.1:8088'
$env:UVMS_BENCH_SERVICE='uvms-java'
$env:UVMS_BENCH_OUTPUT='test-results/java-comparison/java'
$env:UVMS_BENCH_COUNTS='1,4,9,16'
$env:UVMS_BENCH_SECONDS='20'
$env:UVMS_BENCH_WARMUP='3'
node scripts/benchmark-viewers.mjs
```

Rust 비교 서비스에서는 URL8089, service `uvms-rust-bench`로 변경한다. 임시 비교 서비스는 작업 후 중지했으므로 재현 시 먼저 시작한다. sampler는 WSL에서 해당 systemd PID의 CPU/RSS/카메라 TCP 입력 개수를 매초 읽는다. RTSP 비교는 Linux에서 `UVMS_BENCH_SERVICE`, `UVMS_BENCH_RTSP`, `UVMS_BENCH_OUTPUT`을 지정하여 `python3 scripts/benchmark-relay.py`를 실행한다. 기존 Rust capture 예제 클라이언트가 필요하다.

원시 수치는 `test-results/java-comparison/{rust,java}/viewers.json`, `service-samples.json`에 있다. RTSP 수치는 각 폴더의 `relay.json`에 있다. 카메라 URL·비밀번호·영상은 성능 결과에 저장하지 않는다.
