# ONVIF 카메라 검색·PTZ — Rust v0.6.0

2026-10-15. 기존 영상 경로와 별개인 Rust 제어 기능이다. ONVIF 공식 WSDL과
WS-Security 규격을 기준으로 구현했으며, 원본 uVMS ONVIF 내부 코드를 복원했다는
의미는 아니다. Java, 영상 코덱·전송 경로, 서비스 인증 정책은 변경하지 않았다.

## 웹에서 사용

1. 오른쪽 **ONVIF · PTZ → 네트워크 카메라 검색 → ONVIF 카메라 찾기**를 누른다.
2. 등록된 RTSP 카메라와 IP가 일치하는 장비의 **이 카메라 기능 확인**을 누른다.
   검색되지 않으면 제어할 카메라를 고르고 ONVIF 주소를 입력해 **카메라 기능 확인**을 누른다.
3. 제어 프로파일을 선택한다. 카메라가 지원한다고 응답한 방향·줌 버튼만 활성화된다.
4. 방향·줌 버튼 한 번으로 짧게 이동하고 서버가 Stop을 요청한다. 별도 정지 버튼도 있다.

제어할 카메라 선택은 재생 카메라 선택과 독립적이다. 조작 전에 이름을 확인할 수 있다.
이동은 기본 300ms이며, 장비가 요구하는 최소·최대 시간에 맞춘다. 서비스가 허용하는
범위는 100~2,000ms이고 장비 범위와 겹치지 않으면 이동을 비활성화한다.
실제 시험한 Amcrest 장비는 최소 1,000ms를 보고했다.

새 장비도 검색 목록에 표시하지만 **RTSP 스트림 자동 등록은 하지 않는다**.
미등록 장비는 환경변수와 `[streams.별칭]`에 RTSP 입력을 등록하고 서비스를 재시작해야 한다.
현재 PTZ 범위는 ContinuousMove 방향·줌과 Stop이다. 프리셋 저장/호출, 홈 위치,
AbsoluteMove/RelativeMove, 순찰, ONVIF 이벤트 구독·설정 변경은 포함하지 않는다.

## 카메라 계정·주소

기본적으로 입력 RTSP URL의 아이디·비밀번호를 재사용한다. 서비스 로그인이나
접속 토큰은 없다. 장비가 ONVIF 전용 계정을 요구할 때만 환경변수를 추가한다.

```toml
[streams.camera]
url_env = "CAMERA_RTSP_URL"
record = true
# 선택: 기본값은 http://RTSP호스트/onvif/device_service
onvif_url = "http://192.168.1.20/onvif/device_service"
# 선택: 카메라 자체 ONVIF 계정이 RTSP 계정과 다를 때
onvif_username_env = "CAMERA_ONVIF_USER"
onvif_password_env = "CAMERA_ONVIF_PASSWORD"
```

조회 성공한 endpoint만 SQLite에 저장한다. 주소 우선순위는 현재 조회 요청 →
명시적 `onvif_url` 설정 → 저장된 주소 → 기본 후보 주소다. 기본 후보 주소가 모든
카메라에서 유효하다고 가정하지 않는다. 프로필·지원 범위 캐시는 재시작 시 비워지고
10분 후 만료된다. 만료되면 **카메라 기능 확인**으로 다시 조회한다.

SOAP 1.2, WS-Security UsernameToken PasswordDigest와 HTTP Digest(MD5/SHA-256)를
사용한다. GetSystemDateAndTime의 UTC로 시계 차이를 보정한다. HTTP Basic과
UsernameToken PasswordText는 지원하지 않는다. 장비의 ONVIF 기능이 켜져 있어야 한다.
HTTPS 인증서 검증을 끄는 우회는 없으며 자체 서명 인증서 장비는 별도 신뢰 구성이 필요하다.

## 검색 범위

- 서버에서 IPv4 UDP `239.255.255.250:3702`로 WS-Discovery Probe를 보낸다.
  약 3초 동안 응답을 수집하고 장비 ID/IP 중복을 제거한다.
- 숫자 IPv4로 등록된 카메라에는 unicast Probe도 보낸다. 최대 64개 등록 항목까지다.
- 여러 네트워크 카드가 있으면 UI의 서버 검색 인터페이스에 서버의 IPv4를 입력한다.
  클라이언트 PC의 주소가 아니라 Rust 서버에 실제로 할당된 주소여야 한다.
- 다른 VLAN, Docker 브리지, WSL NAT에서는 multicast가 전달되지 않을 수 있다.
  이번 WSL 시험에서는 기존 카메라의 응답을 수신했지만, 미등록 LAN 장비가 모두
  검색된다는 뜻은 아니다. 같은 LAN의 Linux 인터페이스 또는 직접 주소 조회를 사용한다.
- IPv6 검색, DNS 이름과 IP의 자동 동등성 판정, Discovery Proxy는 미지원이다.

카메라 암호를 다른 호스트로 전달하지 않도록 서비스 XAddr는 등록된 RTSP 호스트와
일치해야 한다. 검색 응답의 XAddr도 응답 송신 IP와 대조한다. HTTP redirect는 따르지 않는다.
XML은 1MiB/20,000노드로 제한하고 DTD를 거부한다. 검색은 한 번에 하나,
기능 조회는 최대 4개, 이동은 같은 호스트의 여러 alias를 합쳐 하나만 실행한다.

## API

서비스 URL을 기준으로 다음 JSON API를 제공한다. POST는 기존 same-origin 검사를 사용한다.

| 요청 | 본문·동작 |
|---|---|
| `POST /api/onvif/discover` | `{}` 또는 `{"interface":"192.168.1.10"}` |
| `GET /api/onvif/{stream}` | 저장 주소·캐시 정보만 조회. 카메라에 접속하지 않음 |
| `POST /api/onvif/{stream}` | `{}` 또는 `{"device_url":"http://카메라호스트/onvif/device_service"}` |
| `POST /api/onvif/{stream}/profile` | `{"profile_token":"카메라가 반환한 토큰"}` |
| `POST /api/onvif/{stream}/ptz` | 아래 이동/정지 명령 |

```json
{"action":"move","profile_token":"카메라가 반환한 토큰","pan":0.5,"tilt":0,"zoom":0,"duration_ms":1000}
```

```json
{"action":"stop","profile_token":"카메라가 반환한 토큰"}
```

프로필 조회 후 이동할 수 있다. pan/tilt/zoom은 -1~1의 정규화된 속도이며,
장비가 반환한 실제 좌표 공간과 범위로 변환한다. duration_ms는 해당 프로필의
min_duration_ms/max_duration_ms 안에 있어야 한다. 0벡터, 알 수 없는 프로필,
미지원 축, 범위 밖 속도·시간은 전송하지 않는다.

이동 작업은 HTTP 호출자의 연결 종료와 분리한다. ContinuousMove 실패·시간 초과에도
Stop을 요청하며 정상 이동은 지정 시간 후 Stop을 요청한다. 장비에도 같은 Timeout을
전달한다. Stop 응답 성공은 **명령 수락 확인**이지 물리적 정지를 센서로 검증했다는 뜻이
아니다. 네트워크 단절·장비 오동작 중 물리적 정지를 보증하지 않는다.
조회 성공과 PTZ 상태는 기존 `/api/events`의 `onvif/inspected`,
`ptz/moving`, `ptz/stopped`, `ptz/stop_failed`로 기록한다. 계정 정보는 넣지 않는다.

## 검증

- Rust: 테스트 45개 통과, 수동 벤치마크 1개 제외. NVIDIA 기능 빌드 및 Clippy 경고 0.
- 모의 HTTP/UDP 장비: 검색 중복 제거, WSSE nonce digest·XML escape·90초 시계 보정,
  SHA-256 HTTP Digest의 query URI, Media1/Media2, GetCapabilities fallback 통과.
- 실패 경로: 잘못된 속도/시간/프로필, 다른 호스트 XAddr, 다른 Origin 거절,
  동일 장비의 alias 중복 이동 제한, HTTP 실패/5초 timeout/호출자 TCP 종료 후 Stop 확인.
- Edge: 검색 선택·프로필 전환·방향 4개/줌 2개 각각 Stop·명시적 Stop·오류 시 비활성화 통과.
  1440/930/390px 화면 넘침 없음, page error 0. umbra 접근성·화면 검사 통과.
  정적 색/크기 감사의 유일한 예외는 기존 `931px = 930px breakpoint + 1px`이며 CSS에 근거가 있다.
- 실제 Amcrest IP2M-841W-V3: WS-Discovery 응답, 장비 정보, Media1 프로필 2개,
  PTZ 범위·IDLE 상태 조회 성공. **실제 모터 이동과 줌은 시험하지 않았다.**
- 실제 카메라 입력은 별도 읽기 전용 ffprobe 검사에서 HEVC/H.265 + G.711 A-law로
  확인됐다. GPU가 없는 현재 서버에서 해당 영상 재생은 실패한다. 운영 웹의 영상
  재생 회귀 검증은 기존 H.264 합성 카메라로 통과했다. 카메라 인코딩 설정은 변경하지 않았다.
- Linux systemd v0.6.0 배포 후 `/readyz` 200. 기존 카메라 설정·녹화 파일과
  10GiB 한도 유지, 두 카메라 동시 녹화 재개 확인. Java는 변경하지 않았다.

모의 시험은 실제 장비의 모든 제조사별 동작을 증명하지 않는다. 재현 명령:

```sh
# Linux / WSL, release 바이너리를 /root/uvms-target에 빌드한 환경
python3 scripts/verify-onvif.py
# 브라우저 시험 동안만 별도 실행 후 종료
python3 scripts/verify-onvif.py --serve
# Windows, Edge/Playwright 설치 환경 (위 모의 장비만 대상으로 고정)
node scripts/browser-onvif.mjs
```

공식 근거:
[Device WSDL](https://www.onvif.org/ver10/device/wsdl/devicemgmt.wsdl),
[Media1 WSDL](https://www.onvif.org/ver10/media/wsdl/media.wsdl),
[Media2 WSDL](https://www.onvif.org/ver20/media/wsdl/media.wsdl),
[PTZ WSDL](https://www.onvif.org/ver20/ptz/wsdl/ptz.wsdl),
[PTZ specification](https://www.onvif.org/specs/2512/ONVIF-PTZ-Service-Spec-v2512.pdf),
[OASIS UsernameToken](https://docs.oasis-open.org/wss/v1.1/wss-v1.1-spec-os-UsernameTokenProfile.pdf).
Media2의 `Configurations/PTZ` namespace와 `GetProfiles Type=All`은 공식 WSDL 원문으로 확인했다.
