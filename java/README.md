# UVMS Java 0.1.0

이 폴더는 독립 배포 단위다. `java/` 폴더만 복사하거나 `uvms-java-standalone.zip`을 풀면 서버 소스, 실행 JAR(통합 ZIP), 웹 화면, 설정 예제, Linux 설치, 테스트·측정 도구, 분석 문서가 함께 있다. 상위 Rust 저장소나 실행 파일은 필요 없다.

`web/`은 Java에서 검증한 H.264 영상용 화면이다. Rust 전용 음성·HLS·예약 녹화·NVIDIA 변환은 포함하지 않는다. `docs/`의 기존 측정값은 당시 결과이며, 패키지를 다시 묶었다고 성능을 재측정한 것은 아니다. 문서에 언급된 원본 바이너리·로컬 분석 증거와 과거 테스트 결과 파일은 배포하지 않는다.

기존 Rust 서비스의 미디어 경로를 Java 21로 구현한 비교용 Linux 서비스다. 입력 RTSP 수신, H.264 RTP 조립, WebRTC 송신, 원본 형식의 WebSocket/fMP4, MP4 녹화, RTSP 재송출을 Java에서 처리한다. Rust 프로세스·JNI libwebrtc·FFmpeg를 실행하지 않는다. HTTP/WebSocket은 Netty, DTLS는 Bouncy Castle, SRTP 암호 연산은 JCA를 사용한다.

원본 바이너리를 Java로 자동 변환한 결과가 아니다. 분석된 프로토콜과 현재 Rust의 기능 범위를 바탕으로 새로 작성한 구현이며, 전체 Unreal Media Server 기능을 복제하지는 않는다. 원본 SDK 파일은 테스트에만 사용하고 배포물에 넣지 않았다.

## 현재 동작

- 웹 http://127.0.0.1:8088/ — 기본 카메라 자동 재생, 별도 접속 토큰 없음.
- RTSP `rtsp://127.0.0.1:8558/camera_live` — 서비스 인증 없음.
- 카메라 입력만 환경변수의 원래 RTSP 아이디·비밀번호를 사용한다.
- 같은 카메라 별칭을 보는 시청자는 입력 연결 하나를 공유한다. 서로 다른 별칭에 같은 URL을 넣으면 별도 입력이다.
- 카메라별 녹화 설정, IDR 기준 분할 MP4, 목록·다운로드, 전체 저장 용량 제한, 다중 카메라 웹 화면.
- WebRTC 시청자는 UDP 포트 하나(`ice_udp_min`, 기본 50200)를 공유한다. `ice_udp_max`는 이전 설정 호환용으로만 읽으며 추가 포트는 열지 않는다.
- Linux에서는 원본처럼 고정 수의 소켓이 같은 포트를 공유한다. 기본 `ice_udp_workers`는 16이며 1~16으로 설정할 수 있다. 피어 수에 따라 소켓을 늘리지 않는다. `SO_REUSEPORT`가 없는 플랫폼은 수신 소켓 하나로 동작한다.
- 실시간 시청자 큐가 넘치면 오래된 프레임을 버리고 다음 IDR부터 재개한다. 녹화 큐는 넘칠 때 오류로 처리하며 몰래 프레임을 버리지 않는다. 네트워크 쓰기 제한시간과 카메라 중단 시의 연결 종료·재접속 처리는 유지한다.

## 설치

Ubuntu 예시:

```sh
sudo apt-get install openjdk-21-jre-headless
tar -xzf uvms-java-linux.tar.gz
cd uvms-java
sudo bash install-linux.sh
sudoedit /etc/uvms-java/uvms.env
# REAL_CAMERA_RTSP_URL='rtsp://CAMERA_USER:CAMERA_PASSWORD@CAMERA_HOST:554/PATH'
sudo systemctl enable --now uvms-java
```

카메라 주소는 `uvms.env`에 실제 값으로 작성한다. 파일은 root 전용 0600으로 설치된다. 카메라 암호의 URL 예약 문자는 퍼센트 인코딩한다. `config.json`에는 URL 대신 환경변수 이름만 넣는다. 테스트 영상 생성용 FFmpeg나 MediaMTX는 실제 카메라 서비스 설치에 필요하지 않다.

`config.json`의 `streams`에 카메라별 별칭과 `url_env`를 추가하면 여러 입력을 쓸 수 있다. `record:true`로 바꾸면 해당 입력을 계속 수신하며 녹화한다. 용량 한도에 도달하면 녹화를 멈추며 자동으로 기존 파일을 삭제하지 않는다. 설정 변경은 서비스 재시작 후 적용된다. 기본 설치는 loopback 접속용이며 LAN 접속은 `bind`와 WebRTC UDP 포트 접근 경로를 함께 설정한다.

소스에서 빌드:

```sh
sudo apt-get install openjdk-21-jdk-headless maven
# 저장소에서는 java/, 압축을 풀었으면 uvms-java/ 폴더로 이동
cd uvms-java
mvn package
sudo bash install-linux.sh
```

개발 실행은 `UVMS_JAVA_CONFIG`로 설정 파일을 지정하고 `java -Djava.net.preferIPv4Stack=true -Xms64m -Xmx512m -jar target/uvms-java-0.1.0.jar`를 실행한다. JSON의 `web_root`는 이 폴더 안의 `web` 절대 경로로 지정한다. 통합 ZIP에는 같은 JAR가 루트의 `uvms-java.jar`로 들어 있다. JDK/JRE와 Maven 의존성은 별도이며 최초 빌드에는 네트워크가 필요하다.

## 한 폴더에서 패키징·검증

```sh
# 이 README가 있는 폴더에서 실행
mvn package
python3 scripts/package-java.py
python3 scripts/verify-java-package.py
```

`dist/uvms-java-standalone.zip`은 소스와 실행 JAR를 합친 파일이다. 소스 전용 ZIP, Linux 실행용 tar.gz와 SHA-256 목록도 생성한다. 실제 설정·비밀번호·녹화·과거 테스트 결과는 포함하지 않는다. 기존 설치나 카메라 없이 패키징할 수 있다.

브라우저 검증·성능 측정은 Node.js와 Playwright가 필요하다. `npm ci` 및 `npx playwright install chromium` 후 `UVMS_TEST_URL=http://127.0.0.1:8088 npm run test:browser`로 재생을 확인한다. Windows Edge 또는 `UVMS_BROWSER_PATH`로 지정한 브라우저도 사용할 수 있다. `UVMS_TEST_STREAM`으로 카메라 별칭, `UVMS_TEST_MODE=mse`로 MSE 검증을 선택한다.

`scripts/benchmark-viewers.mjs`는 Linux systemd Java 서비스의 CPU·메모리와 브라우저 수신을 측정한다. Windows에서는 WSL(`UVMS_WSL_DISTRO`, 기본 Ubuntu-24.04) 서비스를 측정한다. 입력 해상도·서비스·주소는 `UVMS_BENCH_WIDTH`, `UVMS_BENCH_HEIGHT`, `UVMS_BENCH_SERVICE`, `UVMS_BENCH_URL`로 지정한다. `scripts/benchmark-relay.py`는 Java Probe로 RTSP 재송출을 측정한다. 과거 Rust 클라이언트와 다른 측정 도구이므로 기존 보고서 수치와 직접 비교하지 않는다.

`java-optimization-fixture.py`와 `verify-java-optimized-recording.py`는 Linux root 및 별도로 준비한 `rtsp://127.0.0.1:8554/camera` 합성 입력이 필요한 시험 도구다. 후자는 녹화 확인용 `ffprobe`를 사용하며 서비스 자체 의존성은 아니다. `summarize-java-optimization.py`는 과거 전후 측정 파일이 있어야 실행되는 보고서 도구다. 일반 설치에서는 실행할 필요가 없다.

## 확인된 범위와 제한

실제 H.264 Main 1920×1080 약 10fps 카메라의 WebRTC·MSE·RTSP 재송출과 MP4 녹화/다운로드를 검증했다. 원본 SDK의 candidate 없는 offer 및 원본 MSE 프레이밍은 Baseline 테스트 영상으로 검증했다. 원본 SDK가 Baseline만 제안하므로 실제 Main 카메라의 원본 SDK WebRTC 테스트는 실패했다. 함께 제공하는 웹 화면은 Main 카메라를 정상 재생한다.

이 구현은 H.264 영상 전용이다. 오디오, H.265, 트랜스코딩, B-frame MP4, 카메라 SDP에 SPS/PPS가 없는 경우, RTSP UDP 전송, TURN 및 인터넷 NAT 횡단은 지원하지 않는다. WebRTC는 host candidate의 ICE-lite/DTLS 1.2/SRTP이며, NACK 재전송을 지원하지만 전체 libwebrtc 수준의 ICE·혼잡 제어·상위 카메라 PLI 제어는 구현하지 않았다. 원본의 UDP 릴레이 분배와 공유 프레임 인덱스를 추가 분석해 Java에 반영했다. Java는 Linux `SO_REUSEPORT`와 고정 소켓 풀을 사용해 내부 릴레이 없이 각 세션으로 직접 전달한다. 원본 Windows IOCP 구현을 그대로 복제한 것은 아니다.

기능 검증과 일부 단기 비교를 진행한 초기 구현이다. 긴 시간의 장애/네트워크 손실/저장 장치 장애 시험, 외부 보안 검토 및 최대 처리량 시험을 통과했다는 의미는 아니다. 저장 장치 쓰기 오류 시 용량 계산이 보수적으로 남을 수 있어 점검 후 재시작으로 다시 계산한다.

JCA/JDK의 표준 런타임까지 Java 바이트코드라는 뜻은 아니다. 애플리케이션 미디어 코드에 별도 native 미디어 엔진을 붙이지 않았다는 의미다.

측정 방법과 결과는 배포물의 `docs/java-comparison.md`에 있다. CPU 결과는 이 두 구현의 비교이며 언어 자체의 우열을 증명하지 않는다.

2026-10-14 추가 분석·변경·전후 비교는 `docs/java-optimization.md`, 원본 함수 근거는 `docs/original-shared-udp.md`와 `docs/original-buffer-optimizations.md`를 참조한다.
