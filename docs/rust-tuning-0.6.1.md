# Rust 전송·DB 튜닝 — v0.6.1

2026-10-15. v0.6.0(`638d69c`)의 Linux 바이너리를 보존하고 같은 합성 카메라로 비교했다.
Java, 카메라 설정, 서비스 인증, 원본 wire 형식은 변경하지 않는다.
이번 변경은 현재 Rust 구현의 비용 감소이며 원본 바이너리의 추가 복원 주장이 아니다.

## 채택한 변경

| 경로 | 변경 | 보존한 동작 |
|---|---|---|
| WebRTC 영상 | 시청자별 `Vec<Bytes>` 패킷 목록을 drain 후 재사용 | 공유 payload, 개인별 SPS/PPS 상태·순서·SSRC·암호화 |
| 음성 수신·통과 경로 | 수신/변환 결과를 불변 `Bytes`로 소유해 passthrough 복사 제거 | AAC→Opus/G.711 처리와 음성 시계, WebRTC 송신 payload 복사 |
| MSE | 10바이트 전송 헤더를 stack 배열로 생성 | 시청자별 MP4 시간·순번, pause/continue/switch, 원본 header |
| SQLite | 쓰기 1개·읽기 최대 4개 연결 재사용, 일부 반복 SELECT statement 캐시 | WAL·동기화 설정, 트랜잭션과 기존 저장 의도 |

영상 목록은 비정상적으로 큰 프레임 이후 작은 프레임으로 돌아오면 큰 할당을 버린다.
음성은 카메라 수신 단계의 최초 데이터 생성과 전송 암호화까지 없어진 것은 아니다.
시청자별 WebRTC 음성 payload는 기존처럼 복사한다. 공유 방식의 실측 이득을 확인하지 못했다.
MSE MP4 payload는 여전히 시청자별 출력 버퍼에 복사된다. 전체 무복사 전송으로 표현하지 않는다.

DB 슬롯은 비동기로 기다리고, 실제 SQLite 작업은 blocking pool에서 실행한다.
HTTP 호출자가 취소돼도 이미 실행 중인 SQL이 끝날 때까지 슬롯을 점유한다.
읽기 전용 연결과 쓰기 연결을 분리하며 SQL 실패나 미완료 트랜잭션이 남은 연결은 재사용하지 않는다.
Linux에서는 파일 inode를 대조해 제거·교체된 DB의 이전 연결을 재사용하지 않는다.
읽기 요청이 없는 DB를 새로 생성하지 않는다. 저장 동기화 수준을 낮추지 않았다.

## 제외한 실험

MSE/HLS/녹화가 영상의 NAL·slice 검사 결과를 공유하는 캐시도 구현해 측정했다.
32KiB 한 개 slice, 16개 mux, 6회 교대 순서의 부품 시험에서 시간 중앙값이
23,168.5µs → 24,061.5µs로 약 3.9% 늘었다. 이 입력에서는 기존 검사 자체가 가볍다.
효과를 입증하지 못한 영상 캐시와 그에 필요한 프레임 자료형 변경은 최종 코드에서 제외했다.
검증 생략이나 모든 시청자의 타임라인 강제 공유로 수치를 높이지 않았다.

## 재현

```sh
cargo test --features nvidia
cargo test --manifest-path ports/unreal-rtsp/Cargo.toml
cargo test --manifest-path ports/unreal-core/Cargo.toml
cargo clippy --all-targets --features nvidia -- -D warnings
cargo build --release --features nvidia
cargo test --release --features nvidia tuning_bench -- --ignored --nocapture --test-threads=1
```

부품 시험은 같은 실행 파일에서 기존 할당 경로와 재사용 경로를 6회 번갈아 실행한다.
서비스 전체 CPU나 최대 동시 접속 수를 나타내지 않는다. DB는 1,000회 순차 조회,
음성은 320바이트, RTP는 약 32KiB 프레임에서 만든 공유 payload 목록이다.

실제 브라우저 비교는 `scripts/compare-rust-tuning.ps1`을 사용한다.
보존한 기준 바이너리는 `test-results/tuning-061/baseline/uvms-gateway`,
후보 바이너리는 `/root/uvms-target/release/uvms-gateway`에 둔다.
Windows Edge 16개 독립 페이지, H.264/AAC 640×360 15fps 합성 카메라 한 스트림,
녹화 off, 준비 후 모드별 20초를 측정한다. 기준→개선→개선→기준 순서다.
테스트용 페이지는 자동 WebRTC 연결을 건너뛰고 지정한 모드로 직접 연결한다.
모드 사이에 이전 시청자 슬롯이 모두 반환된 것을 확인한다.

CPU는 Linux 프로세스의 user+system 시간이며 **논리 코어 하나=100%**다.
개발 PC에서 브라우저와 WSL을 함께 실행하므로 운영 서버 독립 부하 시험과 다르다.
측정 변동, 클라이언트 프레임 드롭, 입력 프레임 속도를 함께 보고 판단한다.

## 측정 결과

부품별 6회 중앙값이다. 아래 비율은 해당 반복 루프의 시간 감소다.

| 부품 | 기존 | 개선 | 감소 |
|---|---:|---:|---:|
| RTP 패킷 목록 | 24,140µs | 19,461.5µs | 19.4% |
| MSE 10바이트 헤더 | 15,024µs | 2,058µs | 86.3% |
| DB 1,000회 조회 | 210,558µs | 98,646µs | 53.2% |

아래는 **음성 송신 공유를 포함했던 초기 후보**와 v0.6.0의 16명 브라우저 시험이다.
각 버전 2회 평균이며 RSS는 각 실행의 최대값을 평균했다. 최종 코드의 수치로 사용하지 않는다.

| 모드 | 기존 CPU | 개선 CPU | 기존/개선 RSS | 기존/개선 FPS |
|---|---:|---:|---:|---:|
| WebRTC | 15.47% | 16.44% | 62.59 / 61.28MiB | 14.44 / 14.28 |
| MSE | 7.01% | 6.64% | 52.97 / 50.90MiB | 14.41 / 13.99 |

모든 실행에서 16명 재생과 카메라 입력 1개 유지를 확인했고 WebRTC RTP 손실은 0이었다.
그러나 WebRTC CPU는 상대적으로 6.3% 증가했다. 전체 WebRTC 성능 향상을 입증한 결과가 아니다.
MSE CPU는 5.4% 감소했지만 개선 버전에서 브라우저 프레임 드롭도 더 많았다.
따라서 이를 순수 처리 효율 향상이나 최대 수용 인원 증가로 환산하지 않는다.
개발 PC의 브라우저·WSL 공동 실행, 짧은 측정 및 입력 시점 차이를 포함한 관측값이다.
이 관측 때문에 음성 payload 복사/공유만 바꾼 별도 바이너리로 추가 확인했다.

### 음성 공유를 제외한 이유

320바이트 payload의 순차 부품 시험에서는 복사 28,607µs → 공유 20,990µs로 26.6% 줄었다.
그러나 16명 WebRTC, 준비 5초·측정 60초, 복사→공유→공유→복사 순서의 서비스 시험에서는
CPU 평균이 복사 15.05%, 공유 15.18%였다. 복사 범위 14.80–15.29%, 공유 15.16–15.20%로 겹친다.
평균 FPS는 각각 14.41 / 14.29였으며 모든 실행의 RTP 손실은 0, 입력 연결은 1개였다.
이는 경합을 입증한 결과도, 공유의 실질적 개선을 입증한 결과도 아니다.
최종 코드는 수신·passthrough의 버퍼 공유만 남기고 시청자별 RTC payload는 기존 복사를 유지했다.

`scripts/compare-rust-tuning.ps1 -AudioIsolation`으로 추가 시험을 실행한다.
`test-results/tuning-061/candidate-copy-audio`와 `candidate-shared-audio`를 각각 준비한다.
두 빌드는 `src/rtc.rs`의 음성 payload 생성 한 곳만 다르며,
복사는 `Bytes::copy_from_slice(&frame.data)`, 공유는 `frame.data.clone()`이다.
최종 전체 서비스가 v0.6.0보다 빨라졌다는 결론이나 최대 시청자 증가량은 제시하지 않는다.

원시 라운드와 모드별 수치는 `rust-tuning-0.6.1.json`에 보존한다.

## 정합성 검증

- Gateway 48개, RTSP 22개, core 9개 단위 테스트 통과. 자동 시험에서 제외한 수동
  벤치마크 및 별도 원본 캡처 의존 시험은 이 개수에 포함하지 않았다.
- `clippy --all-targets --features nvidia -- -D warnings` 통과.
- MSE wire 검사: pause 시 전송 중단, continue와 switch 시 IDR 재개 및 시간 연속성 통과.
- ONVIF 모의 장치: discovery, Media1/2, Digest/WS-Security, PTZ 실패·취소 뒤 Stop 등 통과.
  실제 카메라의 방향은 조작하지 않았다.
- 저장 한도 도달·확대 후 재개, 예약 만료, 수동 off, 재시작 후 설정 복원,
  기존 파일 보존 및 12개 완성 MP4의 영상·음성 검사 통과.
- 4개 카메라 녹화와 12개 재생 화면이 동작하는 동안 32개 동시 요청으로
  조회 API 1,000건 모두 HTTP 200. p50 26.1ms, p95 45.1ms, 최대 380.4ms.
  한 번의 유한 부하 검사이며 API 최대 처리량이나 응답 시간 보장은 아니다.

GPU가 없는 환경이므로 NVIDIA 기능은 빌드까지 확인했고 실제 GPU 변환 성능은 측정하지 않았다.

### 다중 카메라 장애·녹화 회귀

AAC, PCMU, PCMA, Opus 음성을 갖는 합성 H.264 카메라 4개를 WebRTC/MSE/HLS의
12개 화면에서 동시에 재생했다. 수동 녹화 2개·예약 녹화 2개를 함께 실행했다.
측정 483.39초 동안 입력 종료, 연결을 유지한 패킷 폐기, 전체 입력 재시작,
게이트웨이 재시작을 주입했고 검사 실패와 브라우저 media error는 0이었다.
이 장시간 시험은 초기 음성 공유 후보로 실행했다. 최종 코드에서는 RTC의 기존 복사만 복원했고,
해당 송신 경로는 위 60초 교차 시험과 별도의 네 종류 음성 디코딩 검사로 확인했다.
장애 구간에는 재생 중단이 발생한다. 복구 유예 75초 뒤의 지속적인 장애가 없는지 검사한 결과이며
무중단이나 24시간 안정성 보장이 아니다. 마지막에는 입력 연결 0개로 반환됐다.

완성 MP4 128개(309,261,323바이트) 전체에서 영상·음성, 타임스탬프와 시작 IDR을 검사했다.
SQLite 무결성은 `ok`, 파일·색인 일치, 미완성 파일 0개, 검색·Range 다운로드 검사 통과다.
2초를 넘는 녹화 공백 6개는 모두 주입한 장애 구간에 해당했다.
재시작 뒤 API 응답 복귀는 452ms였고 수동·예약 녹화 설정과 기존 색인이 유지됐다.

```sh
python3 scripts/soak-fixture.py setup --binary /root/uvms-target/release/uvms-gateway
# Windows PowerShell에서 UVMS_SOAK_SECONDS=480, UVMS_SOAK_OUTPUT 지정 후 실행
node scripts/soak-surveillance.mjs
python3 scripts/verify-soak-recordings.py test-results/tuning-061/soak/result.json
python3 scripts/soak-fixture.py stop
```

최종 RTC 복사 빌드에서 AAC→Opus, PCMU, PCMA, Opus 각각 40/40 음성 분석 구간이
무음이 아니었고 영상 시간도 진행했다. media error와 음성 변환 오류는 0이었다.

## 배포

Linux 실행 파일은 20,657,560바이트(약 19.70MiB), SHA-256은
`b32f90f3871b0e1f1763daff381344d455f9aa782706ba4e24a3eee9da38e76b`다.
0.6.0 대비 112,032바이트 늘었다. 최종 음성 비교·디코딩에 사용한 바이너리와 동일하다.
WSL Rust 서비스에 v0.6.1을 배포했고 기존 설정과 녹화 1,421개를 보존했다.
