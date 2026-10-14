# H.265 → H.264 NVIDIA 확장 — v0.4.0

## 구현과 검증 상태

Rust 서비스의 카메라 공유 입력에 NVIDIA SDK 직접 변환 경로를 연결했다.
H.264는 기존 압축 데이터 전달을 유지하며, H.265는 카메라 alias당 하나의
NVDEC → NVENC 작업을 거쳐 H.264로 바뀌도록 구현했다. 결과는 기존
WebRTC·WebSocket MSE·HLS·MP4 녹화·RTSP 재송출에서 공유한다.
시청자 수에 따라 GPU 인코더를 추가하지 않는다. 같은 URL을 서로 다른
alias로 등록하면 각각 별도 입력·변환 세션을 쓴다.

**현재 장비는 AMD Radeon 내장 그래픽이며 NVIDIA GPU/드라이버가 없다.
코드·빌드·HEVC 수신·GPU 부재 처리·기존 H264 회귀 검증까지 완료했지만,
실제 NVDEC/NVENC 변환과 그 결과의 브라우저 재생은 아직 검증하지 못했다.**
이 구현은 GPU 장비에서 아래 인수 시험을 통과하기 전까지 하드웨어 검증이
남은 기능으로 취급한다. 소프트웨어 코덱으로 대신 변환한 결과를 GPU
변환 성공으로 표시하지 않는다.

## 처리 구조

```
RTSP/TCP → Rust RTP 조립
  H264 ──────────────────────────────┐
  H265 → NVDEC → GPU NV12 → NVENC H264 ├→ 공유 프레임 → 기존 모든 출력
  음성 → 기존 공유 음성 처리 ──────────┘
```

`ports/unreal-rtsp`가 RFC7798 단일 NAL·AP·FU를 처리한다. VPS/SPS/PPS,
패킷 손실 후 IRAP 복귀, 길이·레이어·시퀀스·타임스탬프 제한을 검사한다.
`src/video.rs`는 카메라별 전용 스레드와 bounded queue를 사용하고 GPU
블로킹 호출을 Tokio 실행 스레드에서 분리한다. 기존 Hub의 alias 잠금이
중복 RTSP 연결 및 중복 변환기 생성을 막는다.

`ports/unreal-nvidia/bridge.cpp`는 작은 C++ ABI 연결 코드다. NVIDIA의
CUDA Driver API·NVDEC·NVENC를 직접 호출한다. Rust가 서비스와 작업 수명을
관리하고 C++ 연결 코드가 SDK 구조체 및 동기식 콜백을 처리한다.
디코딩된 NV12는 GPU에 유지한 채 NVENC에 등록한다. 등록·매핑은 프레임마다
해제하며, host로 복사하는 것은 압축된 출력이다. 아직 성능을 측정한
것은 아니므로 최대 카메라 수나 지연 수치를 보장하지 않는다.

FFmpeg·libavcodec·외부 변환 프로세스는 서비스에서 사용하지 않는다.
테스트 카메라 생성에만 FFmpeg를 쓰고, ffprobe는 서비스 밖 검증 도구다.
이번 GPU 코드는 원본 uVMS의 GPU 구현을 복원했다는 주장이 없는 새 확장이다.

## 지원 범위와 설정

- Linux x86_64, NVENC API 12.0 이상을 지원하는 NVIDIA 드라이버.
- GPU에 HEVC Main 8-bit NVDEC와 H264 NVENC가 모두 있어야 한다.
  CUDA 연산을 지원한다는 사실만으로 두 비디오 엔진을 보장하지 않는다.
- 입력은 8비트 4:2:0, 순차 주사, B프레임 없는 H.265 카메라 영상.
- Main10·다중 레이어·DONL·PACI·인터레이스·B프레임 재정렬은 미지원이다.
- 출력은 원 해상도 H.264 baseline, P4 ultra-low-latency, CBR,
  B프레임/룩어헤드 없음, 약 1초 GOP와 IDR마다 SPS/PPS 반복이다.
  프레임 속도는 HEVC 스트림 정보, 없으면 첫 RTP 프레임 간격을 사용한다.
- 해상도/파라미터 변경과 변환 오류는 연결을 종료하고 기존 재연결 경로를 따른다.
  연결 종료 시 지연된 tail 프레임을 파일 변환기처럼 모두 flush하지 않는다.

기존 카메라 URL과 자체 인증을 그대로 사용한다. 서비스 로그인·토큰은 없다.
기존 설정에 아래 필드를 생략하면 기본값을 적용한다.

```toml
nvidia_device = 0
nvidia_bitrate = 4000000 # H264 목표 비트레이트, bits/s
```

`GET /api/gpu`는 컴파일 포함 여부, GPU 초기화 가능 여부와 이유를 반환한다.
probe 성공은 드라이버·decode capability·encode session 생성 확인이며,
해당 카메라의 실제 변환 완료와는 구분한다. probe 결과는 프로세스에서
캐시하므로 GPU/드라이버를 설치하거나 바꾸면 서비스를 재시작한다.
`/api/streams`의 `video`는 입력/출력 코덱, 변환 방식 또는 초기화 오류를
제공한다. GPU 없는 H265 WebRTC 요청은 HTTP 502이며 H264에는 영향이 없다.

## 빌드·설치

배포 바이너리에는 `nvidia` 기능을 포함한다. 실행에는 NVIDIA 드라이버의
`libcuda.so.1`, `libnvcuvid.so.1`, `libnvidia-encode.so.1`과 GPU 장치 접근
권한이 필요하다. CUDA Toolkit·nvcc·FFmpeg 설치는 실행 조건이 아니다.
Ubuntu 24.04의 libstdc++6·glibc에서 빌드/로딩을 확인했다.

소스 빌드는 C++ 컴파일러와 CUDA 개발 헤더가 필요하며 GPU 없이도 가능하다.

```sh
# 이미 CUDA Toolkit 헤더가 있으면 이 다운로드 단계 생략
sudo python3 scripts/prepare-nvidia-headers.py
export CUDA_INCLUDE_DIR=/opt/uvms-nvidia-sdk/nvidia/cuda_runtime/include
cargo build --release --locked --features nvidia
sudo UVMS_BIN="$PWD/target/release/uvms-gateway" bash scripts/install-linux.sh
sudo systemctl restart uvms
curl http://127.0.0.1:8080/api/gpu
```

일반 `cargo build`는 GPU 기능을 포함하지 않는다. 이 경우에도 H264 기능은
동작하며 H265에는 GPU 빌드가 필요하다는 오류를 반환한다. NVIDIA codec
헤더의 출처·SHA256·MIT 고지는 `ports/unreal-nvidia/headers.toml`과 각 파일에
있다. CUDA 헤더 다운로드는 NVIDIA wheel SHA256을 검증하며 드라이버를 설치하지 않는다.

Docker는 `docker build --build-arg NVIDIA=1 -t uvms-gpu .`를 제공한다.
실행 호스트에 NVIDIA Container Toolkit을 갖추고 `--gpus all`과
`NVIDIA_DRIVER_CAPABILITIES=compute,video,utility`를 사용해야 한다.
Docker GPU 실행은 이 장비에서 검증하지 않았다.

## 검증과 GPU 장비 인수 시험

현재 확인한 내용:

- 실제 NVIDIA API 헤더로 feature-on 빌드·Clippy 및 feature-off 빌드.
- HEVC AP/FU·손실·타임스탬프 wrap·역행 거부·파라미터 변경 단위 검증.
- 합성 H265 Main 640×360/15fps를 Rust RTSP로 30프레임 수신, ffprobe가
  HEVC 30프레임으로 정상 해석함을 확인. GPU 변환 결과가 아니다.
- GPU 부재 상태에서 명확한 초기화 오류와 H265 요청 실패 확인.
- 같은 GPU 포함 바이너리에서 H264/AAC의 WebRTC·MSE·HLS 영상과 실제 음성
  디코딩 회귀 검증 통과.

증거는 `test-results/nvidia/integration.json`, `native.hevc`,
`h264-audio/audio.json`에 있다. integration의 `hardware_transcode_tested`는
현재 **false**다. 프로토콜·모의 테스트를 실제 GPU 시험으로 세지 않는다.

NVIDIA Linux 장비에서는 별도 시험 fixture를 시작한 뒤 다음 순서로 확인한다.
`nvidia-fixture.py`는 기존 `surveillance-fixture.py`가 설치한 MediaMTX 테스트
장비를 이용하며 실제 카메라 설정을 변경하지 않는다.

```sh
sudo python3 scripts/surveillance-fixture.py
sudo python3 scripts/nvidia-fixture.py target/release/uvms-gateway
python3 scripts/verify-nvidia.py # GPU가 없으면 실패함
UVMS_TEST_URL=http://127.0.0.1:8093 UVMS_TEST_STREAM=h265 \
  UVMS_TEST_MODE=webrtc UVMS_TEST_OUTPUT=test-results/nvidia/gpu-webrtc \
  node scripts/browser-smoke.mjs
# 같은 명령의 MODE/OUTPUT을 mse, hls로 바꿔 각 경로도 확인
```

positive gate는 실제 RTSP 재송출의 H264 디코딩 프레임과
`input=H265, processing=nvidia_nvdec_nvenc`를 검사한다. 이후 브라우저별
재생·A/V 동기화·장기 실행·다중 카메라 메모리·GPU 성능 시험이 필요하다.
이 작업에서는 NVIDIA 장비가 없어 해당 positive gate를 수행하지 못했다.

실제 NVIDIA 서버에 등록한 카메라로 검사할 때는
`python3 scripts/verify-nvidia.py --url http://127.0.0.1:8080 --stream camera_live`
처럼 alias를 지정할 수 있다. 원본 카메라 URL이나 비밀번호를 시험 명령에
넣지 않는다. RTSP 재송출 bind가 로컬 이외 주소라면 해당 서버 주소로 접속한다.
