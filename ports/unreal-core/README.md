# Unreal v15에서 근거를 추적한 Rust 부분 이식

이 crate는 기존 FFmpeg gateway와 분리된 **첫 이식 부품**이다. 원본 네이티브 함수의 Ghidra pseudocode 및 공식 SDK 소비 코드를 근거로 frame normalization과 MSE WebSocket application payload header만 옮겼다. 실행 서비스나 플레이어가 아니며, 원본의 전체 미디어 경로를 이식했다고 주장하지 않는다. 런타임 dependency와 FFmpeg 사용이 없다. 테스트만 `serde_json`을 사용한다.

## 대응 함수와 근거

사용자 MSI에서 추출한 `UMediaServer.exe` SHA-256: `19fdb1d0888b38a7b5886a57cce774d9ddf0abc7fe02a62ee6e3673ca8b2a1e8`. 아래 경로는 저장소 루트 기준이다.

| Rust API | 원본 함수 | 보존한 근거 |
| --- | --- | --- |
| `frame::normalize` | `FUN_140025dc0` | `evidence/native/server-v2/040-rtc-normalize.c`; REA `ev_0e04335317c183d431da5c8c1b1ffc55822656229939918d34507ed233d373a8` |
| AVCC4 → 4-byte Annex B | `FUN_140027e10` | `041-avcc-annexb.c` 및 `.evidence.json`의 `raw_result.value`; REA `ev_a820351ac2a86dcb67b81c879fd98ab80241f39193493eb21f524fc927c3d88c` |
| `mse::initialization`, `mse::error` | `FUN_14009ba60` | `028-mse-frame.c`; REA `ev_42bc933a7827d5fbd6804f8afd06ca01ef2a75c59a8d7a517d973a50cc80fa79` |
| `mse::media` | `FUN_14009abf0` | `029-mse-setup.c`; REA `ev_d885c7e98de82df037906478a0edccd4e0cd16108903b3e95a5179ba1a8dbe7d` |

## 구현된 범위

`normalize`는 native와 같이 첫4bytes가 `00 00 00 01`이면 Annex B로 해석하고 그 외는 4-byte big-endian NAL length 형식으로 해석한다. NAL의 내용은 바꾸지 않고 length prefix를 4-byte start code로 바꾼다. 이어서 `u32LE NAL count`, 각 NAL의 `u32LE offset`과 `u32LE length`를 덧붙인다. offset은 전체 payload 시작부터 NAL header까지이며 start code를 제외한다. `payload()`는 trailer를 제외하고 `storage_with_index()`는 trailer까지 반환한다. 원본 frame의 size field도 trailer를 포함하지 않는 구조로 관찰했다.

kind/start100ns/end100ns/keyframe 네 개의 전달된 metadata 값은 그대로 보존한다. 이 구조체는 원본 48-byte COM frame ABI 전체의 바이트 복사가 아니다. 아직 해석하지 않은 ABI 필드나 포인터를 노출하지 않는다.

호출자가 검증된 SPS/PPS cache를 제공하면 keyframe 앞에 `start+SPS+start+PPS`를 넣고 index를 조정한다. 원본처럼 기존 frame에 SPS/PPS가 있어도 중복 제거하지 않는다. cache 탐색·갱신·소유권·오류 복구는 이식하지 않았다. cache 미전달은 original state의 cached-size=0 경로에 해당한다.

`mse`는 외부 WebSocket framing을 제외한 언리얼 application payload를 만든다.

- init: type1, audio1/video2/both3 (realtime이면 왼쪽2bit 이동), timeshift 4개 u16, XML UTF-16LE 바이트 길이 offset18, XML offset20, 그 뒤 이미 생성된 initialization segment.
- media: type2, caller의 filling byte, 선택 timeshift 상태와 특수 u24 시간 배치, signed timestamp-ms LE offset6, media segment offset10.
- error: type0, 원본 생성기가 사용한 mode flag, UTF-16LE 오류 문자열. SDK 문서의 error byte1 reserved와 달리 원본 생성 코드가 mode flag를 기록하므로 이를 보존한다.

XML 생성, ISO-BMFF muxing, timestamp 기준값 산출, filling 판단과 timeshift state machine은 호출자가 책임진다. bytes를 감싸는 기능을 muxer 구현으로 표현하지 않는다.

## 의도적으로 제한한 입력 / 원본과 차이

- source NAL 최대60개. 원본은60개에서 index 분석을 멈추고 원본 payload를 남길 수 있지만 이 포트는61개부터 오류로 거부한다. cache의 두 NAL은 별도여서 최대62개 index를 만들 수 있다.
- 원본 AVCC helper는 length=1을 만나면 변환을 중단한다. 이 포트는 AVCC 분기에서1-byte NAL을 거부한다. 첫 prefix가00000001이면 원본과 같은 Annex B 판정을 유지하므로 AVCC/AnnexB의 처음 prefix 모호성은 자동으로 해결할 수 없다. 다른 length-size, 3-byte start code 지원을 추가하지 않았다.
- 잘린 length, 길이가 남은 payload를 초과하는 NAL, zero-length NAL, 빈 AnnexB NAL은 거부한다. 잘못된 입력에서 원본의 partial output/메모리 접근을 재현하지 않는다.
- cache는 NAL type7(SPS), type8(PPS)와 최대256-byte native cache 범위를 검사한다. native의 cache 초기화 함수를 이식한 것은 아니다.
- init XML의 byte length는 native signed-short memcpy 경로를 넘지 않게32766bytes 이하로 제한한다. init 전체 payload는 native16bit outer header가 표현 가능한65535bytes 이하로 제한한다. 정수 overflow와 크기 초과는 오류로 반환한다.
- media timeshift clock은 caller가 최종 seconds를 넘기며1..0xFFFFFF만 허용한다. native clamp/wrap에 기대지 않는다. 첫 clock 전송 여부나 seconds derivation은 아직 포트 범위 밖이다.
- 기본 Rust `Vec` allocation 정책을 사용한다. 네이티브 COM allocator, refcount, threading, 수명주기를 복제하지 않는다.

## 검증

Linux WSL Ubuntu24.04 / Rust1.99.0에서 `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, unit/contract 테스트8개, release build를 통과했다. 로그 `evidence/port/linux-build-tests.txt`.

원본 `URTSPClient.dll`을 실제 실행하여 합성 RTSP 입력에서 얻은 외부 fixture40개로 별도 테스트1개도 통과했다. `evidence/runtime/rtsp-original-mta/frames.jsonl`의 각 frame과 `.bin`을 직접 읽었다. **204개 NAL / 420,361 payload bytes**, keyframe index11·26의 SPS/PPS/IDR(type7,8,5×5), 나머지38개의 slice(type1×5), 원본 timestamp/keyframe metadata가 보존됨을 확인했다. 로그 `evidence/port/original-capture-test.txt`.

이 테스트 입력은 원본 RTSP DLL의 실제 출력이다. 하지만 **원본 `FUN_140025dc0`의 실행 출력을 캡처해 Rust 출력과 비교한 것은 아니다.** normalization과 MSE header는 정적 pseudocode/SDK 기반 부분 이식이며 아직 original function runtime equivalence가 검증되지 않았다. fixture는 synthetic 영상이며 crate에 원본 DLL/EXE 또는 원본 SDK 코드를 포함하지 않았다.

```sh
cd ports/unreal-core
export CARGO_TARGET_DIR=/root/uvms-core-target
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
# 저장소에 실제 capture evidence가 있을 때만 실행하는 외부 fixture 테스트:
UNREAL_CAPTURE_DIR=../../evidence/runtime/rtsp-original-mta \
  cargo test --locked --test original_capture -- --ignored --nocapture
```

## 아직 이식하지 않은 핵심 경로

RTSP control/auth/SDP/RTP depacketization, 원본 frame callback ABI 및 공유 수명주기, SPS/PPS cache discovery, ISO-BMFF init/fragment muxer, WebRTC signaling/ICE/DTLS/SRTP/RTCP/RTP packetization, sockets/service 관리 모두 미포함이다. 원본 서버와 동일한 재생이 되는 Linux service를 제공하는 단계가 아니다. 다음 검증은 원본 normalization/header producer의 실제 출력을 확보해 대응 비교하고, 원본 RTSP 수신 및 muxer를 각 근거 함수에 맞춰 연결하는 것이다.
