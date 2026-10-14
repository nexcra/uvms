# Rust v0.4.3와 go2rtc v1.9.14 비교

## 비교 대상과 재현 조건

Rust는 배포 중인 v0.4.3과 같은 실행 파일, go2rtc는 측정 시 GitHub latest
API가 반환한 [공식 v1.9.14 릴리스](https://github.com/AlexxIT/go2rtc/releases/tag/v1.9.14)의
linux/amd64 바이너리다. 최신 개발 브랜치나 별도 튜닝 빌드를 사용하지 않았다.

| 대상 | 파일 크기 | SHA-256 |
|---|---:|---|
| Rust v0.4.3 | 18,082,872바이트 | `73492572ee633567f197df43aa29b4a879a6f52d36e0b63f4df22d29e77e0fb1` |
| go2rtc v1.9.14 | 5,726,972바이트 | `32d616af226bd731678ffde328b94cfb94e30339bfefc469cfb76323144615a6` |

go2rtc의 다운로드 SHA-256은 GitHub 릴리스 asset의 digest와 대조했다.
Rust는 NVIDIA API 기능을 포함한 release 빌드이고 동적 링크 ELF, go2rtc는
공식 정적 링크 ELF다. 파일 크기는 각 배포물의 디스크 크기이며 런타임 메모리나
언어 자체의 크기를 비교하는 값이 아니다. go2rtc 실행 파일은 저장소·배포 ZIP에
포함하지 않는다.

- 장비: Ryzen 7 PRO 5750GE, 8코어/16스레드, 호스트 RAM 약 64GB.
- 서버: 같은 Ubuntu 24.04 WSL. 둘 다 `uvms` 사용자, 파일 한도 16,384,
  CPU quota 없음. 운영 서비스와 Java 코드는 변경하지 않았다.
- 같은 합성 RTSP URL, H.264 640×360/설정 15fps, 약 1.25~1.28Mbps, 음성 없음.
  연속 생성 중인 입력을 번갈아 구독하므로 바이트 단위로 동일한 녹화 구간을
  재생한 것은 아니다. 실제 수신 비트레이트를 각 결과에 기록한다.
- 입력 RTSP는 TCP, 출력 WebRTC는 UDP. go2rtc 활성 API에서 `rtsp+tcp` 입력과
  `ws+udp` 소비자를 확인했다. 브라우저의 선택된 후보 통계도 UDP를 확인한다.
- 둘 다 트랜스코딩·녹화·HLS 캐시 비활성. 미디어 서비스에서는 FFmpeg 사용 안 함.
  기존 합성 입력 생성기만 FFmpeg를 사용한다.
- 서버를 동시에 부하 측정하지 않는다. 각 실행 전에 두 테스트 서비스를 중지하고
  해당 서버만 새 프로세스로 시작한다. 실행 순서는 **Rust → go2rtc → go2rtc → Rust**.
- 브라우저 32→64개, 전송 전용 128→512개. 각 단계 준비 5초 후 60초 측정.
  단계 사이 연결은 누적하며, 다음 반복 실행에서는 서버를 재시작한다.
- CPU는 미디어 서버 프로세스만, **한 코어=100%** 기준이다. 브라우저·수신기 CPU는
  별도 기록한다. RSS는 각 구간에서 관찰한 프로세스 최대값이다.

브라우저는 같은 Windows Edge 실행 파일·옵션, 페이지당 독립 WebRTC 연결 8개,
브라우저 프로세스당 32개, 실제 `<video>` 디코딩을 사용한다. 두 대상 모두
`--disable-accelerated-video-decode` 옵션을 지정한다. 시그널링 교환만 각 서버의
프로토콜에 맞췄고, offer 코덱 선택·ICE 수집·통계·판정은 같은 코드를 사용한다.

전송 수신기는 같은 Rust 실행 파일·4개 worker thread를 사용한다. 각 연결이
실제 ICE·DTLS·SRTP 협상을 수행하고 복호화한 RTP의 sequence·marker 프레임을
검사한다. **이 시험에는 브라우저 디코딩과 화면 출력이 없다.**

판정 기준과 이전 용량 시험의 한계는 [동시 접속 보고서](capacity-2026-10-14.md)에
있다. 이번 Rust 브라우저 시험은 HLS를 끈 조건이므로 이전 HLS 활성 수치와
직접 비교하지 않는다. 제품 내부의 RTP 분할 크기, ICE 소켓 관리, 암호화·GC
구현 등은 각 배포 버전의 구현을 유지한다.

## 측정 결과

### 브라우저 실제 재생

각 셀은 두 번 측정의 평균이다. 메모리는 각 측정의 최대 RSS를 평균한 값이다.

| 연결 | Rust CPU | go2rtc CPU | Rust RSS | go2rtc RSS | Rust / go2rtc 평균 fps |
|---|---:|---:|---:|---:|---:|
| 32 | 24.35% | 48.15% | 79.21MiB | 200.13MiB | 14.52 / 14.49 |
| 64 | 51.86% | 112.75% | 122.56MiB | 380.09MiB | 14.52 / 14.49 |

64개에서 Rust CPU는 약 54.0%, 메모리는 약 67.8% 낮게 관측됐다.
CPU 원시 범위는 Rust 49.08~54.65%, go2rtc 110.94~114.56%였다.
32개 CPU 범위는 Rust 21.62~27.09%, go2rtc 46.36~49.93%였다.

모든 반복·단계가 판정 기준을 통과했고 RTP 패킷 손실 증가량은 0이었다.
64개에서 Rust freezeCount 증가 합은 각 실행 0/0, go2rtc는 5/25였다.
이는 64개 수신기의 계수 합이며 해당 횟수만으로 체감 품질을 단정하지 않는다.
총 수신량은 Rust 평균 79.61Mbps, go2rtc 79.44Mbps로 유사했다.
두 서버 모두 입력 연결 최대 1개, 브라우저 선택 후보는 UDP였다.

### 전송 전용 비교

각 행은 두 번 측정의 평균이며 fps는 수신기 전체의 평균이다.

| 연결 | 서버 | CPU | 최대 RSS 평균 | 완전한 RTP 프레임 fps | 총 수신 Mbps | 판정 |
|---|---|---:|---:|---:|---:|---|
| 128 | Rust | 64.34% | 198.02MiB | 15.01 | 164.16 | 2회 통과 |
| 128 | go2rtc | 192.37% | 708.18MiB | 9.42 | 103.17 | 2회 실패 |
| 512 | Rust | 462.26% | 685.15MiB | 15.00 | 656.29 | 2회 통과 |
| 512 | go2rtc | — | — | — | — | 앞 단계 실패로 미측정 |

go2rtc 128개는 각 실행 평균 8.29/10.55fps, 가장 느린 연결 7.30/9.82fps로,
13fps 기준을 충족하지 못했다. RTP sequence 불연속 합은 64/37이었다.
이는 수신 순서의 불연속 계수이며 네트워크 패킷 손실률과 동일하지 않다.
두 제품 모두 128개 연결 자체는 성공했고 10초 동안 수신이 완전히 멈춘 연결은 없었다.
공통 수신기는 품질 기준 실패 시 다음 단계를 실행하지 않으므로 go2rtc 512개
성능은 이번 시험으로 알 수 없다.

Rust 512개는 두 실행 모두 sequence 불연속·10초 정지가 0이었고,
가장 느린 연결도 약 14.99fps였다. 서버 CPU는 436.91~487.62%,
최대 RSS는 684.27~686.03MiB였다. 이는 브라우저 512개 재생 검증이 아니다.

**128개에서는 두 서버가 실제 전달한 영상량이 다르므로 CPU 비율을 같은 일을
처리하는 효율 배수로 해석하지 않는다.** go2rtc 프레임률 저하의 내부 원인은
확정하지 못했다. 같은 수신기와 설정에서 반복됐다는 관찰만 기록하며,
공유 호스트 부하나 수신기 상호운용 영향을 분리한 결과는 아니다.
따라서 “go2rtc의 최대 시청 수는 64명” 또는 “Rust가 4배 많은 시청자를 처리한다”는
결론을 내릴 수 없다. 같은 출력량에서의 자원 비교 근거는 위 브라우저 32·64개 결과다.

## 한계와 증거

서버와 부하 발생기가 같은 물리 PC를 사용하며 다른 작업들도 실행 중이다.
전용 유휴 서버의 실험이 아니다. 반복 순서를 바꿔 영향 편중을 줄였지만 없애지
못하므로 작은 CPU 차이를 일반적인 우열로 해석하면 안 된다. 로컬/WSL 경로이므로
물리 NIC·인터넷 회선·TURN·WAN 손실·1080p/4K·음성·녹화·변환 성능은 검증하지 않는다.

수신 프레임률과 재생 정지는 측정하지만 카메라 촬영부터 화면 표시까지의 종단
지연은 측정하지 않았다. 이 결과를 “몇 배 더 저지연”으로 표현하지 않는다.
두 번의 짧은 반복에서 관찰한 CPU·메모리 차이가 다른 입력과 모든 기능의
성능 차이를 뜻하지도 않는다.

실행 전후 호스트 CPU 스냅샷은 전송 시험에서 최대 100%까지 관찰됐다.
구간 전체의 호스트 부하를 통제·기록한 것은 아니므로 특히 전송 시험 실패를
go2rtc 엔진만의 한계로 귀속할 수 없다. 전용 Linux 서버와 별도 부하 발생기에서
장시간 반복하기 전에는 운영 최대 인원이나 제품 전체의 성능 순위를 확정하지 않는다.

원시 데이터는 Git 제외 경로 `test-results/go2rtc-comparison/`에 보존한다.
`browser/<run>-<engine>/capacity.json`, `service-samples.json`,
`transport/<run>-<engine>/transport.json`, 각 실행의 `environment.json`과
`completion.json`, `release.json`, `binaries.json`, `go2-source-active.json`을
함께 확인한다. 집계는 `scripts/summarize-go2rtc.py`로 재생성한다.
실행별 수치와 시작·종료 호스트 부하를 포함한
[집계 스냅샷](go2rtc-comparison-2026-10-15.json)은 저장소에도 보존한다.
`all_runs_finished`는 8회 실행 종료를 나타내며, 품질 실패로 생략한 단계를
측정 완료로 표시하지 않는다.

재현 순서는 다음과 같다.

1. WSL root에서 `scripts/go2rtc-benchmark-fixture.py`를 실행해 공식 바이너리의
   해시를 확인하고 별도 서비스 설정을 만든다(HTTP8097, 미디어 UDP50810).
2. `UVMS_BENCH_MAX_SESSIONS=1024`로 `scripts/rust-optimization-fixture.py
   /opt/uvms/uvms-gateway`를 실행한다(HTTP8091, 미디어 UDP50800).
3. `cargo build --release --example rtc_capacity`로 공통 전송 수신기를 빌드한다.
4. Windows에서 `scripts/compare-go2rtc.ps1 -Mode browser`, 종료 후
   `scripts/compare-go2rtc.ps1 -Mode transport`를 실행한다. 두 모드를 병렬로
   실행하지 않는다. 스크립트는 마지막에 두 테스트 서비스만 중지한다.

검증: 공통 수신기 release 빌드와 Clippy `-D warnings`, Rust 포맷,
브라우저 스크립트 구문 검사, Python 컴파일 검사, PowerShell 파서 검사를 통과했다.
종료 후 두 비교 서비스가 중지됐으며 운영 `uvms`의 v0.4.3 health가 정상임을
확인했다. 이번 변경은 비교 도구와 보고서이며 운영 미디어 엔진 변경은 없다.

go2rtc 신호 교환은 해당 릴리스의
[`internal/webrtc/webrtc.go`](https://github.com/AlexxIT/go2rtc/blob/v1.9.14/internal/webrtc/webrtc.go),
네트워크 설정은
[`internal/webrtc/README.md`](https://github.com/AlexxIT/go2rtc/blob/v1.9.14/internal/webrtc/README.md)를
확인했다. 대상 소스는 `test-results/go2rtc-reference/`에 보존했다.
