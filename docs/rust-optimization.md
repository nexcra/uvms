# Rust 공유 UDP·프레임 처리 개선 — 2026-10-14

추가 동시 접속 부하와 부하 발생기 한계는
[동시 접속 측정 보고서](capacity-2026-10-14.md)에 기록했다.
공식 go2rtc와 같은 입력·수신기를 사용한 비교는
[go2rtc 비교 보고서](go2rtc-comparison-2026-10-15.md)에 기록했다.

## v0.4.3: WebRTC RTP 분할 결과 공유

v0.4.2까지는 프레임 정규화만 공유하고, 각 시청자가 `H264Payloader`로 같은
Annex B를 다시 읽어 FU-A 조각을 할당·복사했다. v0.4.3은 프레임의 `OnceLock`에
불변 RTP payload와 SPS/PPS 처리 순서를 보관한다. 최초 WebRTC 소비자만 이를
만들고 이후 소비자는 `Bytes` 참조를 공유한다. WebRTC 소비자가 없으면 이
비용은 발생하지 않는다.

`src/rtp_video.rs`는 실제 FU-A·single-NAL 분할에 기존 `rtp 0.14.0` 라이브러리를
그대로 사용한다. 시청자별 SPS/PPS 보류·교체·STAP-A 출력 상태는 별도로 유지해
중간 접속, 큐 지연 후 복귀, 분리된 parameter-set 입력도 기존 동작을 보존한다.
SSRC·RTP 시퀀스·marker·타임스탬프·SRTP 암호화·NACK 처리는 기존 세션 경로에
남는다. 암호화된 패킷을 다른 시청자에게 재사용하는 구조가 아니며 전체 전송의
zero-copy를 주장하지 않는다. 기존 원본 정규화 이식을 유지한 Rust 전송 최적화로,
원본 uVMS의 미분석 내부 구현까지 복원했다는 의미는 아니다.

정합성 검사는 기존 payloader와 byte-for-byte 비교, MTU 경계, 큰 IDR 복원,
SPS/PPS가 서로 다른 프레임에 오는 경우, 초과 크기 parameter set, AUD/filler,
새 시청자 상태 분리 및 여러 스레드의 실제 payload 주소 공유를 포함한다.

### 분할 처리 자체의 측정

`cargo test --release --features nvidia benchmark_shared_packetization -- --ignored --nocapture`
로 실행한다. 이미 정규화된 32KiB NAL 2,000개를 16명에게 분할해 보내는 준비
작업을 6회 측정하며 변경 전·후 실행 순서를 교대한다. 출력은 두 방식 모두
992,000개 패킷이다. 처리 시간 중앙값은 1,049,543µs → 114,229µs로 약 9.19배
짧았다. 이 측정에는 RTSP 수신, 정규화, 네트워크 전송, SRTP, 브라우저 디코딩이
포함되지 않는다. 전체 서버 처리량이 9.19배 늘었다는 뜻이 아니다.
원시 수치: `test-results/rtp-sharing/microbenchmark.json`.

### 브라우저 동시 재생 측정

별도 WSL Rust 서비스에 동일한 합성 H.264 640×360/설정 15fps 입력을 연결했다.
Windows Edge headless 16개, 준비 5초 후 25초 측정, 녹화 off, 웹의 HLS 되감기
캐시 활성 조건이다. 변경 전 v0.4.2와 변경 후 v0.4.3을 교대로 두 번씩 측정했고,
각 실행 전 서비스 프로세스를 새로 시작했다. CPU는 서버 프로세스만 측정한
**한 코어 100% 기준**이며 입력 생성기·브라우저 CPU는 제외한다.

| 측정 | 변경 전 CPU | 변경 후 CPU | 변경 전 최대 RSS | 변경 후 최대 RSS |
|---|---:|---:|---:|---:|
| 1회 | 13.44% | 10.16% | 132.31MiB | 57.89MiB |
| 2회 | 11.51% | 10.52% | 129.22MiB | 57.75MiB |
| 평균 | 12.47% | 10.34% | 130.77MiB | 57.82MiB |

이 조건에서 CPU는 약 17.1%, 측정별 최대 RSS의 평균은 약 55.8% 감소했다.
평균 재생률은 전 14.47fps/후 14.44fps였고, 모든 실행의 RTP 패킷 손실은 0,
같은 alias의 입력 RTSP 연결은 최대 1개였다. 두 버전 모두 미디어 UDP50800
한 개와 시청자별 mDNS5353 소켓을 사용했다. 브라우저 dropped-frame 계수는
일부 실행에서 증가했으므로 전 시청자 무손실 렌더링으로 표현하지 않는다.
짧은 로컬 반복 측정이며 4K, WAN, 최대 동시 접속 수나 장기 안정성 결과가 아니다.

원시 자료는 `test-results/rtp-sharing/{before-1,before-2,after-1,after-2}/`의
`viewers.json`, `service-samples.json`이다. 변경 전 바이너리는 같은 디렉터리의
`before/uvms-gateway`에 보존했다. 서비스 시험은 `rust-optimization-fixture.py`에
각 바이너리를 명시하고 `benchmark-viewers.mjs`의 `UVMS_BENCH_*` 설정으로
재현한다(HTTP8091, service=`uvms-rust-opt-bench`, source port=8554,
counts=16, modes=webrtc, width=640, height=360, warmup=5, seconds=25).

### 회귀 확인

16명 측정 후 1명으로 줄이는 동일 순서의 추가 대조에서 CPU는 전 2.05%/후
1.90%, 재생률은 각각 14.55/14.56fps였다. 단일 시청에서 CPU 증가가 관찰되지
않았다. 이 단계의 RSS에는 앞선 16명 접속의 할당이 남으므로 단일 시청 메모리
수치로 사용하지 않는다.

`cargo test --all-targets --features nvidia`는 31개 통과, 별도 실행하는 벤치마크
1개 제외이며 fmt와 clippy도 통과했다. 합성 AAC 입력의 WebRTC·MSE·HLS
브라우저 음성 검사는 각각 100/100, 99/100, 100/100 구간에서 음성을 검출했다.

`scripts/browser-rtc-sharing.mjs`는 두 alias에 16명 접속, 8명 종료 후 기존
8명의 재생 지속, 새 8명 접속, 전체 16명의 추가 프레임 진행을 확인했다.
모든 연결은 UDP50800, RTP 손실 0이며 재시도 없이 통과했다. 이전 검사의
단일 통계 스냅샷에서는 재생 중인 한 연결의 포트를 찾지 못해 실패했다.
새 검사는 `transport.selectedCandidatePairId`로 실제 선택된 연결을 따라가며,
누락 시 재시도 이력을 남긴다. 이전 실패 기록도 삭제하지 않았다.
자료: `test-results/rtp-sharing/{audio,peer-isolation,peer-isolation-selected}/`.

WSL 운영 서비스(HTTP8080)에 v0.4.3을 배포한 뒤 합성 `camera`의 영상 수신,
연결 해제·재접속 검사를 통과했다. 입력 설정과 기존 MP4 225개를 보존했다.
실행 파일은 18,082,872바이트이며 서비스의 자식 프로세스와 FFmpeg 실행은
없다. 배포 증거는 `test-results/rtp-sharing/deployment.json`, 재생 증거는
`test-results/rtp-sharing/deployed-webrtc/browser.json`에 있다.

## 이전 공유 UDP·정규화 최적화 기록

현재 변경은 RTSP H.264 영상, WebRTC, WebSocket MSE, MP4 녹화, RTSP/TCP 재송출의 범위를 유지한다. HLS 등 원본 제품의 추가 기능은 [기능 대조표](feature-gap-2026-10-14.md)에 정리했으며 이번 성능 변경에서 구현하지 않았다. 카메라 자체 인증만 사용하고 서비스 접속 토큰은 추가하지 않았다.

## 분석 근거와 구현 경계

| 원본에서 확인한 내용 | 이번 Rust 반영 |
|---|---|
| 서버 `0x1400265f0`가 한 프레임을 한 번 정규화하고 RTC 소비자에게 재사용 | `Arc<SharedFrame>`에 `OnceLock`으로 정규화한 Annex B를 공유. MSE·녹화만 있으면 정규화하지 않음 |
| `0x140025dc0`의 압축 payload와 NAL 인덱스 | 기존 정규화의 바이트·검증 규칙 유지. payload 소유권을 넘겨 두 번째 전체 복사 제거 |
| 공용 UDP 포트에서 ICE 세션을 분배 | 프로세스가 소유하는 `webrtc-ice 0.14` UDP mux. `ice_udp_min`에 한 미디어 소켓을 열고 시청자가 공유 |
| MSE `0x14009abf0`의 크기 우선 출력 버퍼 처리 | 최종 moof+mdat 크기를 한 번 할당하고 AVCC를 직접 기록. MSE 10바이트 헤더 자리도 같은 버퍼에 확보 |

원본 Evidence ID와 분석 한계는 [프레임 분석](original-buffer-optimizations.md), [공유 UDP 분석](original-shared-udp.md)에 있다. 기존에 보존한 원본 분석을 재사용했으며 이번 작업에서 원본 바이너리를 다시 실행한 것은 아니다.

원본은 같은 공개 포트에 16개 소켓과 IOCP를 사용한다. Rust는 Tokio 비동기 소켓과 기존 ICE 라이브러리의 mux를 사용하므로 원본 IOCP 엔진의 직역이나 모든 내부 최적화의 복원이라고 부르지 않는다. RTP packetizer의 SPS/PPS 상태, 시퀀스, SSRC, SRTP 암호화는 시청자별로 유지한다. 정규화 공유가 전체 전송 경로의 zero-copy를 뜻하지 않는다.

추가로 MP4의 B-slice 검사는 NAL 전체를 복사해 RBSP로 바꾸는 대신 필요한 두 Exp-Golomb 필드만 읽는다. RTSP 재송출의 FU-A 분할도 조각별 임시 Vec 없이 최종 RTP 전송 버퍼에 헤더와 원본 조각을 기록한다. 녹화 시간축·파일 공개·용량 정책은 유지한다.

## 운영 설정

기존 설정의 `ice_udp_min`이 공용 WebRTC 포트다. `ice_udp_max`는 설정 호환을 위해 유지하며 예전 범위 검증도 유지하지만 시청자별 포트 선택에는 사용하지 않는다. 서비스 시작 시 포트를 확보하므로 충돌은 시작 실패로 드러난다. 다른 서비스와 포트를 중복 사용하지 않는다. `singleport`·`randomport` URL은 모두 이 공유 전송 경로로 받는다.

시청자 종료는 해당 ICE 연결만 제거하며 전역 소켓은 서비스 종료 시 닫는다. STUN 서버·public_ip 설정, 카메라 인증 및 기존 API는 유지한다. 외부 NAT·손실 환경 전체를 이번 로컬 시험으로 검증한 것은 아니다.

## 검증과 측정

측정 입력은 실제 카메라가 아닌 동일한 합성 RTSP fixture다. 640×360, H.264 baseline, 생성 설정15fps, 실제 수신 약13.5fps, 약1.15Mbps. FFmpeg/MediaMTX는 이 시험 입력을 만들기 위해서만 실행되며 서비스는 영상 재인코딩이나 외부 미디어 프로세스를 실행하지 않는다.

WSL Ubuntu24.04, 논리 CPU16개, Windows Edge headless16개 시청자. WebRTC/MSE 각각 준비5초 후25초를 측정하고, RTSP는16개 클라이언트가 각각100프레임을 받는다. CPU는 Linux 서비스 프로세스만 측정하며 **한 코어100% 기준**이다. 브라우저 디코더와 테스트 영상 생성기의 CPU는 포함하지 않는다. MSE 측정은 WebRTC 측정에 이어 같은 서비스 프로세스에서 실행한다.

원시 자료는 `test-results/rust-optimization/fixture-before/`와 `fixture-after/`의 `viewers.json`, `service-samples.json`, `relay.json`이다. 변경 전 배포 바이너리를 `before/uvms-gateway`에 보존했다. 배포용 압축 파일에 이 과거 바이너리나 시험 영상은 포함하지 않는다.

| 16명 동시 접속 | 변경 전 CPU | 변경 후 CPU | 변경 전 RSS 최고 | 변경 후 RSS 최고 |
|---|---:|---:|---:|---:|
| WebRTC | 10.06% | 9.87% | 164.84MiB | 105.03MiB |
| WebSocket MSE | 3.70% | 3.19% | 185.67MiB | 114.00MiB |
| RTSP/TCP | 1.85% | 1.78% | 185.43MiB | 114.00MiB |

이 짧은 한 쌍의 측정에서는 WebRTC RSS가36.3%, MSE RSS가38.6% 낮았다. MSE CPU 감소는 약13.7%이며 WebRTC·RTSP CPU 차이는 작아서 뚜렷한 개선으로 단정하지 않는다. 최대 처리량·장시간 안정성이나 Java보다 빠르다는 결론을 내리는 시험은 아니다. RTSP RSS는 앞선 브라우저 부하가 사용했던 프로세스의 잔류 메모리를 포함하므로 FU-A 변경만의 효과로 해석하지 않는다.

WebRTC는 전후 평균13.48/13.51fps, 패킷 손실0이었다. MSE는16개 headless 브라우저에서 전후 평균9.98/10.05fps였고 브라우저 dropped-frame 계수가 증가했다. 서버 CPU 개선을 클라이언트 전원 무손실·15fps 재생으로 표현하지 않는다. RTSP는 전후 각각1600프레임을 모두 받았다. 두 브라우저 모드 모두 같은 alias의 RTSP 입력 연결은 최대1개였다. 서로 다른 alias에 같은 URL을 등록하면 별도 입력이 생기는 기존 동작은 유지한다.

변경 전 WebRTC 측정에서 프로세스 UDP 소켓은 최대48개였고 변경 후17개였다. 변경 후 **미디어 UDP는50800의 한 소켓**이며 나머지는 라이브러리의 mDNS5353 소켓이다. mDNS 기능을 끄거나 전체 UDP 소켓이 한 개라고 주장하지 않는다. MSE 전용 구간에도 서비스 소유의 미디어 소켓 하나는 열린 상태다.

## 회귀 검증

- Rust 애플리케이션12개, MP4 crate8개, core contract8개 통과. 추가로 보존된 원본 RTSP DLL의40개 프레임으로 core 보존 검증1개 통과. 원본 normalizer 출력을 직접 캡처한 byte-equivalence 시험과 구분한다.
- MP4 개선 전 고정 box fixture,64-bit DTS, sample flag, offset, malformed/B-slice 거부와 상태 유지, prefix 길이와 payload 보존 검증.
- 16개 동시 소비자의 Annex B 버퍼 주소 공유와 기존 RTP packetizer 출력 일치, 큰 IDR FU-A 복원 검증.
- 실제 Edge16개·alias2개에서 동일한 원격 미디어 포트50800 선택.8개 종료 후 나머지8개 프레임 증가, 새8개도 같은 포트로 연결. `shared-port/result.json`.
- signaling offer 전 종료80회 후 세션 슬롯 회수. `sessions/result.json`.
- MSE pause 동안 송신 중단, continue/switch의 IDR 시작과 시간축 연속성. `mse-commands/`.
- 추출된 **수정하지 않은 원본 SDK**의 WebRTC·MSE 플레이어 모두 재생. `original-sdk/`.
- 별도 임시 녹화 서비스에서1MiB 한도 정지·6초간 파일 불변, MP4 다운로드 바이트 일치와 ffprobe 읽기 검증, SIGTERM 시 partial 정상 확정, 서버 자식 프로세스0. `recording.json`. ffprobe는 시험 검사 도구일 뿐 런타임 의존성이 아니다.
- root/MP4 clippy `-D warnings`, format, release build 통과. MP4 테스트 헬퍼의 새 Clippy 권고를 반영했으며 런타임 동작 변경은 없다.

## 배포

`uvms.service`(HTTP8080)에 적용했다. 기존 카메라 설정·비밀번호 파일의 해시가 유지됐고 완료 녹화225개의 크기·해시가 변하지 않았다. Java 바이너리와 서비스도 유지했다. 서비스 자식 프로세스가 없고 서비스 안에서 FFmpeg 실행 파일에 접근할 수 없음을 확인했다. 배포 기록은 `test-results/rust-optimization/deployment.json`이다.

Linux 바이너리13,504,800바이트, SHA-256 `98e4cd08818f76e6f7499228fa21fe037cd2d0d8f85310e0e2cb07e07cc3b264`. 배포 archive와 source ZIP의 최종 크기·해시는 `dist/native-manifest.json`에 기록한다. 원본 MSI·DLL·SDK·시험 영상·실제 카메라 비밀번호는 배포하지 않는다.

현장 카메라의 H.264 입력을 다시 확인한 결과가 아니라 사용자가 선택한 테스트 영상 검증이다. 실제 카메라 설정은 변경하지 않았다. Docker 실행, 외부 NAT·모바일 전기종·장시간 최대 동시 접속은 이번 시험 범위가 아니다.
