# Rust RTSP 재송출

`src/relay.rs`는 같은 Rust ingest feed의 H264 압축 frame을 RTSP 1.0/RTP로 다시 보내는 기능이다. 기본 주소는 `rtsp://127.0.0.1:8555/{stream}`. 카메라를 별도로 다시 연결하지 않고 `Hub.acquire`/`Feed.subscribe`를 WebRTC·MSE·녹화와 공유한다. 사용자가 추가 요청한 RTSP 재송출에 대한 표준 기반 구현이며 원본 UMediaServer RTSP **출력** dispatcher의 함수 단위 복원이라고 표시하지 않는다.

## 계약

- OPTIONS, DESCRIBE, SETUP, PLAY, TEARDOWN, GET_PARAMETER. RTSP/1.0 only.
- 재송출 요청은 서비스 인증 없이 처리한다. 예: `rtsp://127.0.0.1:8555/camera_live`. 카메라 자체 아이디·비밀번호는 입력 RTSP 연결에서만 사용한다.
- 단일 H264 video track. TCP interleaved `RTP/AVP/TCP;unicast;interleaved=N-M`만 허용하며 UDP는461을 반환한다. 요청한 서로 다른8-bit RTP/RTCP 채널을 사용한다.
- SDP는 실제 SPS/PPS와 `profile-level-id`를 전달한다. output URL의 userinfo는 Content-Base에 복사하지 않는다. RTSP path는 등록된 stream ID와 선택 `trackID=0`으로 제한한다.
- RTP timestamp는 입력 `Frame.timestamp_90k`의 하위32bits, sequence/SSRC는 viewer별로 생성. single NAL 또는 FU-A fragment로 보낸다. 전송방식의 prefix/fragment header만 변경하며 NAL header와 압축 bytes를 보존한다. marker는 access unit의 마지막 RTP packet에만 설정한다.
- 최초 IDR까지 기다리고 broadcast receiver lag가 생기면 다음 IDR부터 다시 시작한다. SPS/PPS는 SDP와 ingest의 keyframe AVCC에서 제공한다. 재인코딩·scale·fps 변경 없음.
- 5초 주기 RTCP compound Sender Report+SDES(CNAME)를 전송한다. SR은 source RTP clock을 마지막 frame 수신 시각에서90kHz로 외삽해 NTP와 연결한다. 원본 카메라의 absolute wall-clock mapping을 복원하는 것은 아니다.
- client interleaved RR/SDES는 bounded consume한다. RR에 따른 전송률 제어나 PLI/NACK upstream forwarding은 미구현이다.

## 수명과 한계

각 TCP viewer는 다른 전달 경로와 같은 `App.slots` 한 개를 차지한다. RTSP header16KiB, body4KiB, inbound interleaved packet8KiB, input access unit32MiB로 제한한다. reader queue8개, request read60초, write3초 timeout이다. TEARDOWN/disconnect/shutdown시 reader/subscription/permit을 정리한다. 초기 설정·source reconnect는 shared hub에 맡긴다. root hub 자체의 camera idle 정책은 이 모듈 범위 밖이다.

live-only이므로 Range는 `npt=0-`, `npt=0.000-`, `npt=now-`만 허용한다. seek/playback rate, UDP/multicast, RTSP TLS, publishing(RECORD/ANNOUNCE), audio는 지원하지 않는다. binary/HTTP body parsing 실패는 연결을 종료한다. RTSP session keepalive는60초 이내 OPTIONS 또는 빈 GET_PARAMETER로 유지한다.

## 근거와 검증 범위

입력 원본 분석은 `docs/original-rtsp-native.md`, `docs/original-server-native.md`에 연결되어 있다. retransmission의 protocol framing은 [RTSP RFC2326](https://www.rfc-editor.org/rfc/rfc2326), [H264 RTP RFC6184](https://www.rfc-editor.org/rfc/rfc6184), [RTP/RTCP RFC3550](https://www.rfc-editor.org/rfc/rfc3550)에 따라 구현했다. 원본 설치 CHM의 RTSP 출력 지원과 자체 source/client의 압축 경계에 맞춰 연결한 것이며 원본 서버의 bytes/동시성/timeout과 완전 동일한 것으로 간주하지 않는다.

focused tests는 transport 선택과 credential 없는 URL 반환, single-NAL+FU-A 정확한 재조합, marker/sequence wrap/timestamp wrap, RTCP SR/SDES 구조, RTCP 뒤 fragmented+pipelined RTSP 요청 처리3개이며 모두 통과했다.

실제 Linux 서비스의8555 relay와8554 합성 원본 source를 동시에 `ports/unreal-rtsp`의 별도 client로 받아80frames씩 저장했다. **겹치는80개 AVCC frame 전체의 SHA256이 일치**했고 SPS/PPS도 동일했다. 별도의 ffprobe RTSP client가640×360 영상46frames를 decode/count했으며 stderr는0bytes였다. ffprobe는 이 검증에만 사용하며 relay 런타임 의존성이 아니다.

이전 인증 버전의 기록: 무인증401, 잘못된 암호401, 올바른 Basic 인증200, UDP SETUP461을 확인했었다. 현재 서비스 인증은 제거되었으며 이 인증 결과는 과거 기록이다. 재현 script `evidence/port/verify-relay.py`, 결과 `relay-verification.json`, `relay-ffprobe.json`, `relay-ffprobe-errors.txt`, source/output frame 증거는 `relay-source/`, `relay-output/`에 있다. 테스트 secret은 환경변수로 받고 결과/오류 파일에는 자격증명을 쓰지 않는다. source 영상은 합성 영상으로 실카메라 검증이 아니다.
