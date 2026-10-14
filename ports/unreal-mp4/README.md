# 원본 media flow를 따른 H264 fMP4 muxer

이 crate는 `RTSP compressed AVCC frame → ISO-BMFF fragment → WebSocket/MP4 file` 부분을 Rust로 구현한다. 외부 process, FFmpeg, codec decode/encode를 사용하지 않는다. NAL payload와 AVCC length prefix를 그대로 mdat에 복사한다. 원본 MP4 구현과 **바이트 단위 동일성**을 주장하지 않는다.

## 공개 API

```rust
pub struct Muxer;
impl Muxer {
    pub fn new(sps: &[u8], pps: &[u8], width: u32, height: u32)
        -> anyhow::Result<Muxer>;
    pub fn init_segment(&self) -> &[u8];
    pub fn codec(&self) -> String;
    pub fn fragment(&mut self, avcc: &[u8], timestamp_90k: u64,
        duration_90k: u32, keyframe: bool) -> anyhow::Result<Vec<u8>>;
    pub fn fragment_with_prefix(&mut self, avcc: &[u8], timestamp_90k: u64,
        duration_90k: u32, keyframe: bool, prefix_len: usize)
        -> anyhow::Result<Vec<u8>>;
}
```

- SPS/PPS는 NAL header를 포함하고 시작 코드/length prefix는 없는 bytes다. width/height는 호출자가 SPS에서 해석한 display dimension을 전달한다. 이 모듈은 SPS dimension과 caller 값을 대조하지 않는다.
- 단일 H264 video track, track ID1, media timescale90000. movie timescale1000.
- `timestamp_90k`는 호출자가 이미 세션 시작을0으로 맞춘 **DTS**다. 내부 hidden rebasing은 없다. `duration_90k`는 양수여야 한다. DTS는 이전 sample 시작보다 커야 하며 gap은 허용한다.
- PTS=DTS 경로만 지원한다. 모든 VCL NAL의 slice_type을 읽어 B-slice를 거부한다. decode 재정렬을 지원하려면 별도 CTS API와 signed composition offset이 필요하다.
- 첫 fragment는 IDR/keyframe이어야 한다. 중간 GOP에서 시작한 카메라에는 다음 IDR까지 기다린 뒤 muxer를 시작한다. `keyframe`과 IDR 유무가 다르면 오류다.
- 각 호출은 1sample의 완전한 `moof+mdat`를 반환한다. 기록 파일에 `init_segment()`를 한 번 쓰고 fragments를 순서대로 추가하면 fragmented `.mp4`가 된다. recording 파일 열기/회전/종료, reconnect, 공유 ingest 수명은 상위 service가 관리한다.
- `fragment_with_prefix`는 같은 할당의 앞에 0으로 초기화한 0~64바이트를 예약한다. MSE 전용 헤더를 직접 채워 중간 fragment 복사를 피할 수 있다. MP4 내부 data offset은 moof 기준 108이며 prefix 길이에 영향받지 않는다. MP4 파일에 쓸 때는 prefix를 제외해야 한다. 기존 `fragment`는 prefix 0으로 위임한다.
- SPS/PPS가 바뀌면 오류다. 새 muxer와 init, 새 recording 파일을 시작해야 한다. 허용 profile은66/77/88/100/110/122/144이며 high-profile avcC extension도 작성한다. 실제 영상 검증은 constrained baseline fixture에서 수행했다.

## 원본 근거와 의도적인 변화

근거 대상 `UMediaServer.exe` SHA256 `19fdb1d0888b38a7b5886a57cce774d9ddf0abc7fe02a62ee6e3673ca8b2a1e8`. 자세한 native call flow는 `docs/original-server-native.md`, pseudocode는 `evidence/native/server-v2/`에 있다.

| 원본 함수/근거 | 확인한 역할 | 이 crate와 관계 |
| --- | --- | --- |
| `0x14002bf00`, `032-mux-initsegment.c` | ftyp와 moov를 직렬화하여 초기 segment 반환 | 같은 init segment 구조 |
| `0x14002c280`, `042-ftyp.c`, REA `ev_c6a36e7a9e704c2996997c8bcc38d0443e32c3ec520420ac735c25dd009a10f6` | isom major brand, minor0x200, compatible brands | isom/0x200 유지, H264 전용이므로 hvc1 brand 제외 |
| `0x14002c320`, `043-moov.c` | movie/mvex, 각 track과 trex 구성 | single video로 제한. 원본의 audio/video track 번호·전체 defaults를 복제하지 않음 |
| `0x14002d3f0`, `031-mux-fragment.c`, REA `ev_ba8c8757a2d9ecba5ba02c1a83fee47171ef9ceec1d6aacd47ac5f0467261892` | timestamp 차이와 keyframe/fragment duration에 따른 fragment 생성 | caller가 duration 전달, 한 access unit마다 fragment 생성. 원본 batching 정책 미이식 |
| `0x14002da50`, `036-mux-serialize.c` | moof/traf/tfhd/tfdt/trun 생성, data offset=moof size+8, 원본 compressed sample data를 mdat에 기록 | 같은 fragment-relative 구조와 압축 bytes 보존. 표준 field를 명시적 Rust writer로 작성 |
| `0x14009a6c0`, `014-mse-codec-func.c` | 원본 MSE MIME `avc1.42E01E` 상수 | 이 crate는 실제 SPS profile/compatibility/level에서 codec을 생성. 고정 baseline 표기를 반복하지 않는 의도적 변경 |

box 세부 필드를 전부 원본 pseudocode에서 복원한 것은 아니다. 다음 구조는 원본의 컨테이너 처리 흐름을 보존하면서 [W3C ISO-BMFF MSE byte-stream 요구](https://www.w3.org/TR/mse-byte-stream-format-isobmff/)에 맞춰 좁은 입력 범위에 구현한 것이다: `ftyp+moov`, empty sample tables, mvex/trex, fragment마다 version1 tfdt, default-base-is-moof tfhd, trun data_offset/sample_duration/sample_size/sample_flags, moof 뒤 mdat. 실제 원본 init/fragment bytes와 비교하지 않았으므로 원본 AP4/Bento4 직렬화 전체의 이식이라고 표현하지 않는다.

2026-10-14 최적화는 `docs/original-buffer-optimizations.md`의 MSE 어댑터
`0x14009abf0` 크기 선계산 근거(REA
`ev_d885c7e98de82df037906478a0edccd4e0cd16108903b3e95a5179ba1a8dbe7d`)를
Rust API에 맞게 적용했다. 고정 단일 sample 구조의 `moof` 100바이트와 `mdat`
헤더 8바이트 및 선택적 transport prefix를 포함한 최종 용량을 한 번 할당하고 AVCC payload를 한 번만
복사한다. 기존 `AVCC → 임시 Vec → mdat Vec → 최종 Vec` 복사가 없어졌다.
원본 MP4 serializer와 동일한 구현이거나 소비자 간 fragment를 공유한다는 의미는
아니다. 반환한 Vec는 호출자가 소유하므로 비동기 전송 중 덮어쓰는 buffer 재사용은 없다.

slice 검증도 NAL 전체를 RBSP로 복사하지 않고 emulation-prevention 바이트를
건너뛰는 reader로 앞의 두 Exp-Golomb 필드만 읽는다. 필드마다 leading zero를
31개로 제한하며 기존 B-slice·잘린 입력 거부를 유지한다. 이는 Rust의 불필요한
복사를 줄인 별도 구현 선택이며 원본이 같은 reader를 사용한다는 주장은 아니다.

에러나 unsupported 입력에는 부분/손상 MP4를 반환하지 않고 상태를 유지한다. input access unit 최대64MiB,0-length/truncated NAL, forbidden bit, 지원하지 않는 NAL type, timestamp overflow와 sequence exhaustion을 거부한다. 압축 frame의 모든 H264 syntax를 검증하는 decoder는 아니다. caller는 complete access unit/frame 경계를 제공해야 한다.

## 검증 결과

- Linux Rust:4개 box/contract 테스트. init tables/avcC,64-bit tfdt, sequence, trun offset·duration·flags, mdat 바이트 보존, B-slice·malformed·config change 거부. `fmt --check`, `clippy --all-targets -- -D warnings`, release build 통과.
- **원본 URTSPClient.dll 출력**에서 first IDR인 frame11부터 frame39까지29frames를 그대로 mux했다.309,311bytes의 AVCC sample 입력을 사용했다. 생성 파일은 `evidence/port/original-capture-rust.mp4`이며 fixture는 합성 테스트 영상이다.
- ffprobe의 읽기 전용 frame decode/count: H264 constrained baseline640×360, timebase1/90000, start0,29frames. stderr0bytes. `evidence/port/mp4-ffprobe.json`, `mp4-ffprobe-errors.txt`.
- 실제 Windows Edge의 MSE에서 Rust 생성 파일을 append하고 끝까지 재생:29frames, dropped0, decoder error 없음. `evidence/port/mp4-browser-result.json`, 재현 script `mp4-browser-verify.mjs`.
- 이 검증은 Rust mux 결과의 표준 호환·재생 검증이다. 원본 server mux 출력과의 byte equivalence나 전체 RTSP→WebRTC service 검증이 아니다. ffprobe는 개발 검증에만 사용하며 crate 실행 경로에 없다.

```sh
cd ports/unreal-mp4
export CARGO_TARGET_DIR=/root/uvms-mp4-target
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
cargo run --locked --example capture_mux -- \
  ../../evidence/runtime/rtsp-original-mta ../../evidence/port/original-capture-rust.mp4
```

오디오, B-frame CTS, edit lists, timeshift, adaptive variant switching, H265, original fragment batching, final nonfragmented MP4 index/mfra는 미포함이다. fragmented MP4를 그대로 이어 쓰는 recording은 마지막 완전한 fragment까지 읽을 수 있지만 파일 durability와 중간 write failure 복구는 상위 recording layer의 책임이다.
