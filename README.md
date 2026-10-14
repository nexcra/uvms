# UVMS Rust Linux media service

v0.4.0은 NVIDIA SDK 직접 H265→H264 변환 경로를 추가한다. **실제 NVIDIA GPU가 없어 하드웨어 변환은 아직 미검증**이며, 지원 조건·빌드·시험 절차는 [docs/nvidia-transcoding.md](docs/nvidia-transcoding.md)에 있다. H264 입력은 기존 경로를 유지한다.

UMediaServer v15 MSI의 네이티브 분석·공식 SDK 프로토콜·원본 DLL 실행 관찰을 기준으로 만든 **H.264 카메라용 Rust 서비스**다. 원본 전체 소스 복원이나 모든 기능의 동일성을 뜻하지 않는다.

서비스의 RTSP 수신, H.264 NAL 처리, WebRTC 송신, WebSocket fMP4, MP4 녹화, RTSP 재송출에는 **FFmpeg·MediaMTX·원본 Windows DLL이 필요 없다**. FFmpeg와 MediaMTX는 합성 RTSP 카메라 테스트 도구다.

Java 21 구현은 [java/README.md](java/README.md), 같은 카메라를 사용한 Rust/Java 비교는 [docs/java-comparison.md](docs/java-comparison.md)에 있다. Java 서비스는 별도 포트8088에서 실행한다.

`java/`는 웹 화면·설치·문서·검증 도구까지 포함한 독립 프로젝트다. 해당 폴더에서 `mvn package` 후 `python3 scripts/package-java.py`를 실행하면 소스와 실행 JAR를 함께 담은 `dist/uvms-java-standalone.zip`이 만들어진다.

Rust 공유 UDP·버퍼 개선과 동일 입력 전후 측정은 [docs/rust-optimization.md](docs/rust-optimization.md), v0.3.0의 음성·HLS·관제 기능과 검증 범위는 [docs/rust-surveillance.md](docs/rust-surveillance.md)에 있다. Java는 이번 변경 대상이 아니다.

## 제공 기능

| 기능 | 동작 |
|---|---|
| RTSP 카메라 입력 | Rust TCP interleaved, OPTIONS/DESCRIBE/SETUP/PLAY, Basic/Digest, keepalive |
| WebRTC | H.264 영상과 Opus/G.711 음성, 공유 UDP ICE/DTLS/SRTP, 원본 codec-first 및 answer/candidate 메시지 |
| WebSocket MSE | 원본 v15 이진 헤더·UTF-16LE metadata·영상/AAC fMP4, pause/continue/switch |
| HLS·되감기 | fMP4 HLS, 카메라별 RAM 캐시, 라이브 되감기·실시간 복귀 |
| MP4 녹화 | 수동·기간/매일/매주 예약, H.264/AAC, IDR 분할, atomic publish, 전역 저장 용량 제한 |
| 녹화 검색·재생 | SQLite 카메라·시간 색인, 브라우저 재생·탐색, Range 다운로드 |
| RTSP 재송출 | TCP interleaved H.264와 원본 음성, 별도 인증 없음, RTCP SR/SDES, VLC/NVR용 |
| 운영 | 다중 alias, alias별 공통 입력·음성 변환, 음성 상태, 녹화 연결 재시도, systemd |

정상 종료 시 현재 녹화 파일도 완료 처리한다. 강제 종료나 디스크 쓰기 실패 시 .partial 파일은 자동 공개하지 않는다. 저장 한도에 도달하면 기존 녹화를 삭제하지 않고 쓰기를 중단한다. 용량 제한은 시작 시 기존 파일도 합산한다. 파일을 수동 정리한 뒤 서비스를 재시작하면 사용량을 다시 계산한다.

## 입력 조건과 범위

- 검증된 카메라 설정: H.264 constrained baseline, B-frame 없음, 주기적 IDR, SPS/PPS 제공.
- 해상도·FPS·비트레이트를 변환하지 않는다. 재생 가능한 코덱은 카메라에서 설정한다.
- 음성 입력은 AAC-LC(MPEG4-GENERIC), PCMU/PCMA(G.711), Opus를 지원한다. WebRTC용 AAC→Opus, MSE/HLS/MP4용 G.711·Opus→AAC 변환은 카메라별로 공유하며 영상은 재인코딩하지 않는다. 코덱 처리는 정적으로 연결한 libopus·FDK-AAC 및 Rust Symphonia를 사용한다.
- H.265는 NVIDIA 기능 빌드와 지원 GPU가 있을 때 H264로 변환하는 경로를 제공한다(현재 하드웨어 미검증). RTMP/MPEG-TS/SRT 입력, 브라우저 카메라 송출, MKV/ASF 녹화는 포함하지 않았다. HLS·녹화 검색·되감기는 Rust 확장으로, 원본의 TS HLS mux나 녹화 서버 전체를 복원한 것은 아니다.
- RTSP 입력과 재송출은 TCP interleaved만 지원한다. rtsps·RTSP/UDP는 미지원이다.
- WebRTC 시청자들은 `ice_udp_min`의 단일 UDP 포트를 공유한다. `ice_udp_max`는 기존 설정 호환을 위해 읽지만 포트 할당에 쓰지 않는다. 원본 공유 엔진의 분배 원리를 Rust ICE mux로 반영했으며 원본 IOCP 엔진 자체를 복원한 것은 아니다. TCP WebRTC 요청은 명시적으로 거부한다.
- 원본 SDK의 H264 offer 선택은 profile42에 제한된다. main/high profile 카메라는 원본 SDK 호환을 보장하지 않는다.
- SPS/PPS 변경·입력 장애 시 기존 재생 연결을 끝낸다. 녹화기는 재시도하고 새 파일을 만들며, 브라우저는 다시 연결할 수 있다.
- 웹과 RTSP 재송출은 별도 로그인 없이 원본 media wire 형식으로 연결한다. 카메라 자체 인증만 입력 RTSP 연결에서 처리한다. 원본 sid 인증 DB나 설정 파일 호환은 구현하지 않았다.

## Linux 실행

Rust1.99.0에서 검증했다. 소스 빌드에는 C/C++ 컴파일러·CMake·pkg-config가 필요하다. 배포 바이너리에는 음성 코덱을 정적으로 연결했으므로 FFmpeg나 별도 코덱 서비스 설치가 필요 없다. 실제 운영 카메라 주소는 환경변수에 둔다.

~~~sh
cargo build --release --locked
cp config.example.toml config.toml
export CAMERA_RTSP_URL='rtsp://user:password@camera:554/live'
export UVMS_CONFIG="$PWD/config.toml"
# 직접 실행 시 config.toml의 recording_dir를 쓸 수 있는 경로로 변경
./target/release/uvms-gateway
~~~

브라우저 http://127.0.0.1:8080 을 열면 기본 카메라가 자동 재생된다. 웹·영상 WebSocket·녹화 다운로드·RTSP 재송출에는 서비스 토큰이나 별도 로그인이 없다. 카메라 자체 인증은 서버에 저장한 입력 RTSP 주소의 아이디·비밀번호만 사용한다. `default_stream`은 처음 재생할 카메라다. 재송출 주소는 `rtsp://127.0.0.1:8555/camera`이며 사용자·암호 없이 연결한다.

외부 접속에는 bind/rtsp_bind와 public_ip를 맞춘다. HTTP/WebSocket은 TLS reverse proxy를 사용하고, WebRTC 공유 UDP 포트(기본50000)를 허용한다. 서비스마다 서로 다른 UDP 포트를 사용해야 한다. STUN만으로 모든 NAT를 통과하지는 않는다. TURN 인증 설정은 이 버전에 포함하지 않았다.

## systemd

~~~sh
sudo UVMS_BIN="$PWD/target/release/uvms-gateway" bash scripts/install-linux.sh
sudo install -m 600 /dev/null /etc/uvms/uvms.env
# 편집기로 CAMERA_RTSP_URL=... 설정 (카메라 자체 인증이 있으면 URL에 포함)
sudo systemctl enable --now uvms
systemctl status uvms
journalctl -u uvms
~~~

설치 스크립트는 기존 config와 비밀번호 파일을 덮어쓰지 않는다. 실행 계정은 uvms, 녹화 경로는 /var/lib/uvms/recordings. 런타임 Dockerfile에도 FFmpeg가 없다. Docker 실행 검증과 Linux systemd 실행 검증은 별도로 기록한다.

## 원본 근거와 구현 대응

| 분석된 원본 | Rust 구현 |
|---|---|
| URTSPClient.dll RTP/NAL 조립·AVCC·timestamp | ports/unreal-rtsp |
| UMediaServer.exe 0x140025dc0 / 0x140027e10 정규화 | ports/unreal-core/src/frame.rs |
| UMediaServer.exe 0x14009ba60 / 0x14009abf0 MSE header | ports/unreal-core/src/mse.rs |
| UMediaServer.exe 0x14002bf00 / 0x14002da50 init/fragment flow | ports/unreal-mp4 |
| 원본 WebRTC SDK 메시지·signaling 후 WS 종료 | src/rtc.rs |
| 원본의 압축 프레임 전달 | Rust webrtc0.14 RTP/ICE/DTLS/SRTP 계층 |

MP4 box 세부 작성과 RTSP 재송출 서버, 운영·저장 정책은 이 분석 경계에 맞춘 Rust 구현이다. 원본 AP4/Bento4와 libwebrtc 전체를 소스 대 소스로 변환한 것은 아니다. 원본의 동작을 증명한 범위와 설계 선택은 각 crate README에 따로 적었다.

분석 문서: docs/original-investigation.md, docs/original-server-native.md, docs/original-rtsp-native.md, docs/original-webrtc-native.md, docs/original-protocol.md.
현재 서비스 검증은 docs/native-service-verification.md를 본다. docs/verification.md는 이전 FFmpeg 시제품의 과거 기록이다.
원본 MSI·SDK·DLL·인증서 및 개인키는 배포 archive에 포함하지 않는다.

## 다중 카메라 화면

카메라 목록을 불러온 다음 상단의4·9·16분할을 선택하고, 동시에 볼 카메라를 체크한 뒤 연결한다. 1은 기존 단일 재생 화면이다.

- WebRTC와 WebSocket MSE 모두 카메라별 독립 재생.
- 카메라별 자동 재연결(2→4→8→16→30초), 개별 재연결 버튼.
- 확대·분할 복귀, 분할 수 변경 때 유지되는 카메라의 연결을 재생성하지 않는다.
- 재생 중 체크를 추가/해제하면 해당 카메라만 연결/해제한다.
- 각 화면에 수신 상태·실제 디코드 프레임·해상도·녹화 상태를 표시한다. 서버 녹화 상태는5초 간격으로 갱신한다.
- 카메라는 config.toml의 streams 항목과 각 URL 환경변수로 등록한다. 예:

~~~toml
[streams.entrance]
url_env = "ENTRANCE_RTSP_URL"
record = true

[streams.parking]
url_env = "PARKING_RTSP_URL"
record = true
~~~

환경변수를 추가한 후 systemctl restart uvms를 실행한다. 비밀번호를 포함한 RTSP URL을 UI에 노출하지 않는다. 카메라별 녹화 폴더는 recording_dir/alias다. 저장 용량 예산은 모든 카메라의 합계다.

현재 데모의 camera는15fps, camera_two는 별도 RTSP source의10fps 영상이다. camera_copy는 첫 source를 참조하는 기존 검증 alias다. 실제 동시 재생·장애 격리 검증은 독립 source2개로 수행했다. 9·16은 제공하는 화면 배치이며16대 실카메라의 처리량을 검증했다는 의미는 아니다.
