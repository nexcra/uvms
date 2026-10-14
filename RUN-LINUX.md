# Linux 실행과 현재 데모

v0.4.0에는 NVIDIA SDK 직접 H265→H264 연결 코드가 포함된다. 현재 AMD 장비에서는 GPU 변환을 실행할 수 없으며 실제 하드웨어 변환은 미검증이다. `/api/gpu`로 상태를 확인하고 [NVIDIA 실행·검증 절차](docs/nvidia-transcoding.md)를 따른다. H264는 GPU 없이 기존대로 재생한다.

현재 WSL Ubuntu-24.04에 uvms.service로 설치되어 있다.
브라우저: http://127.0.0.1:8080
페이지를 열면 토큰 입력 없이 기본 카메라를 재생한다. 카메라 자체 신호와 H.264 설정이 정상이어야 하며, 목록에서 합성 테스트 카메라를 선택할 수도 있다.
현재 기본 카메라는 `default_stream = "camera_live"`다. 웹과 RTSP 재송출의 서비스 토큰 인증은 제거되어 있다. 카메라 자체 인증만 입력 주소에서 사용한다.

- 실서비스 바이너리: /opt/uvms/uvms-gateway
- 설정: /etc/uvms/config.toml
- 환경변수: /etc/uvms/uvms.env (root만 읽기)
- 녹화: /var/lib/uvms/recordings
- RTSP 재송출: rtsp://127.0.0.1:8555/camera_live (아이디·비밀번호 입력 없음)
- WebRTC: 모든 시청자가 `ice_udp_min`(기본50000)의 단일 UDP 포트를 공유한다. 여러 서비스를 실행할 때 각각 다른 포트를 지정한다.
- systemctl status uvms / journalctl -u uvms
- 테스트 카메라 생성기만 FFmpeg를 쓴다. uvms.service 안에서는 /usr/bin/ffmpeg 접근도 차단된다.

Windows에서 WSL을 완전히 종료하면 브라우저 접속도 중단된다. WSL을 켜면 enabled 서비스가 다시 시작한다.
필요하면 터미널에서 다음 명령으로 WSL과 서비스 로그를 열어 둔다.

~~~powershell
wsl -d Ubuntu-24.04 -u root --exec journalctl -fu uvms
~~~

테스트 도구는 별도 서비스다.

~~~sh
systemctl status uvms-demo-rtsp uvms-demo-source
# 실카메라로 전환할 때:
systemctl disable --now uvms-demo-source uvms-demo-rtsp
# /etc/uvms/uvms.env의 CAMERA_RTSP_URL 수정
systemctl restart uvms
~~~

데모 녹화 한도는256MiB다. 꽉 차면 storage_full로 멈추며 기존 파일을 지우지 않는다.
실운영 시 config.toml의 recording_limit_bytes·segment_seconds를 조정하고 저장 공간을 관리한다.
record=false인 camera_copy도 같은 합성 영상을 가리킨다. 독립 alias 재생 검증용이다.

배포 파일은 dist/uvms-native-linux-x86_64.tar.gz, 소스는 dist/uvms-native-source.zip.
Linux 바이너리는 Ubuntu24.04 glibc2.39에서 검증했다. 다른 배포판은 소스 빌드를 권장한다.
Dockerfile 제공, Docker 실행 검증은 하지 않았다.

자세한 입력 제한과 원본 구현 대응은 README.md, 검증 결과는 docs/native-service-verification.md에 있다.

## v0.3.0 관제 기능

단일 화면에서 WebRTC·웹소켓·HLS를 선택한다. 음성은 브라우저 자동재생 정책 때문에 처음에는 꺼져 있으며 음성 버튼으로 켠다. 카메라에 음성 트랙이 없거나 지원하지 않는 코덱이면 상태가 표시된다. 영상은 H.264로 설정한다.

실시간 시청을 시작하면 해당 카메라의 HLS 되감기 캐시도 준비한다. 과거 영상이 쌓인 범위에서 되감고 실시간 버튼으로 복귀한다. 기본 최대 5분·카메라당 128MiB이며 용량에 따라 기간이 짧아질 수 있다. 캐시는 RAM에 있으므로 재시작 때 초기화된다. 상세 메모리 상한과 GOP 조건은 docs/surveillance-archive.md를 참조한다.

녹화 영역에서 카메라를 선택하고 시작·중지·예약을 조작한다. 수동 상태와 예약은 SQLite에 저장된다. 카메라·시간으로 완료된 녹화를 검색한 뒤 재생하거나 다운로드한다. 녹화 중인 파일은 정상 종료 후 검색에 나타난다. 기존 256MiB 한도가 꽉 찬 설치에서는 먼저 저장 공간과 recording_limit_bytes를 조정해야 새 녹화가 가능하며, 업그레이드가 기존 녹화를 삭제하지 않는다.

음성 포함 기능은 별도 합성 시험 서비스 http://127.0.0.1:8092 에서 AAC·PCMU·PCMA·Opus 4종으로 검증했다. 해당 시험 서비스는 scripts/surveillance-fixture.py로 재현하며 운영 설정·실카메라를 바꾸지 않는다. 재부팅 후 자동 시작하는 운영 서비스와 별개다.

바이너리 archive로 새 Linux에 설치:

~~~sh
tar xzf uvms-native-linux-x86_64.tar.gz
cd uvms-native
sudo UVMS_BIN="$PWD/uvms-gateway" bash scripts/install-linux.sh
# /etc/uvms/uvms.env 생성 및 카메라 주소 설정 후:
sudo systemctl enable --now uvms
~~~

다중 카메라 데모:
- camera / camera_two를 선택해4분할에서 동시에 재생한다.
- camera_two는 별도 uvms-demo-source-two.service가 만드는10fps 테스트 영상이며 녹화도 별도 폴더에 저장된다.
- 실카메라로 전환할 때 uvms-demo-source-two도 disable --now로 중지한다.
- 두 번째 테스트 source 재현 도구는 scripts/setup-multicamera-demo.py다. 기존 테스트 fixture와systemd 설치 후root로 실행하는 로컬 데모 전용 도구다.
