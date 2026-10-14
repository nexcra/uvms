# Rust v0.3.0 음성·관제 확장

2026-10-14. Rust 서비스에 음성, HLS, 라이브 되감기, 수동·예약 MP4 녹화,
카메라·시간별 녹화 검색, 브라우저 녹화 재생·Range 다운로드를 추가했다.
웹·WebSocket·RTSP 재송출에 서비스 토큰이나 로그인을 추가하지 않았다.
카메라 자체 RTSP 인증은 기존 설정대로 사용한다. Java 구현은 변경하지 않았다.

## 원본 근거와 새 구현의 경계

원본 URTSPClient.dll의 음성 코덱 선택 근거는
`evidence/native/rtsp-v2/021-pseudo.c`, 함수 `0x180007880`,
Evidence `ev_3d442e3ab8efecffcd014c9227f882f1ebf381a394d2356997a8747b12c05b12`다.
분석 대상 DLL SHA256은
`056726f883b96982bc303ae84ce9b3cb8d4c7c3640e82d42ef96bb08976ad148`이다.
이 함수의 MPEG4-GENERIC·Opus·PCMU·PCMA 선택과 기존 원본 SDK의
codec-first WebRTC 메시지·MSE 영상/음성 헤더를 구현 기준으로 재사용했다.
이번에 전체 원본 바이너리를 새로 실행하거나 모든 오디오 경로를 복원한 것은 아니다.

`original-protocol.md`에 원본 SDK 메시지 근거가 있다. 기존 압축 프레임 공유,
단일 UDP 포트, AVCC 정규화와 재인코딩 없는 영상 전달 구조를 유지했다.
AAC RTP 해석·RTCP 시각 동기화·오디오 변환·저장 정책은 해당 프로토콜을
구현한 새 코드다. 원본의 모든 내부 최적화와 동일하다고 주장하지 않는다.

원본 CHM의 HLS는 TS와 m3u8을 외부 웹 서버로 제공한다. 이번 HLS는 기존
Rust MP4 mux를 확장한 **fMP4 HLS**다. 원본 TS mux의 이식으로 부르지 않는다.
원본 별도 Archival Server의 DB 스키마·COM 구현 대신 SQLite와 HTTP API로
수동·예약 녹화 및 검색을 구현했다.

## 음성 경로

| 카메라 입력 | WebRTC | WebSocket MSE·HLS·MP4 | RTSP 재송출 |
|---|---|---|---|
| AAC-LC MPEG4-GENERIC | Opus 변환 | AAC 압축 데이터 유지 | AAC 유지 |
| PCMU·PCMA, 8kHz mono | G.711 유지 | AAC 변환 | G.711 유지 |
| Opus, 48kHz | Opus 유지 | AAC 변환 | Opus 유지 |

영상 H.264는 모든 경로에서 재인코딩하지 않는다. 카메라별 RTSP 입력을 공유하고,
필요한 출력에 대한 음성 변환도 카메라별 한 번 수행한다. 시청자마다 카메라에
새로 연결하거나 별도 인코더 프로세스를 띄우지 않는다. 서로 다른 alias로 같은
주소를 중복 등록한 경우는 별도 입력으로 취급한다.

AAC 디코딩에는 Rust Symphonia, AAC 인코딩에는 FDK-AAC,
Opus 인코딩/디코딩에는 libopus를 사용한다. FDK-AAC·libopus는 바이너리에
정적으로 연결한 C/C++ 라이브러리이며, 전체 코덱이 순수 Rust라는 뜻은 아니다.
FFmpeg는 합성 입력 생성에만 쓰고 서비스 실행에는 쓰지 않는다.
`THIRD_PARTY.md`, `licenses/`, 소스 ZIP의 `third-party-sources/`에
라이선스와 연결한 코덱의 원본 소스를 포함한다.

오디오 RTP의 원래 시계와 영상의 90kHz 시계를 구분한다. 최초 패킷 도착 시각을
임시 기준으로 쓰고, 두 트랙의 RTCP Sender Report가 도착하면 공통 NTP 기준으로
보정한다. 비정상 SSRC·큰 시각 점프를 거르고 프레임별 보정 폭을 제한하여
타임스탬프를 역행시키지 않는다. SR을 보내지 않는 카메라는 네트워크 지터에
영향을 받는 최초 도착 기준으로 남는다. `/api/streams`에 동기화 상태와
보정량·음성 변환 오류를 표시한다. 수십 시간 장기 드리프트나 모든 실카메라를
검증했다는 의미는 아니다.

AAC-LC 기본 프레임 1024 samples, mono/stereo를 대상으로 한다. HE-AAC,
MP4A-LATM, 다채널 AAC, H.265는 지원 범위가 아니다. 지원하지 않는 음성은
상태를 표시하고 가능한 영상 경로를 유지한다.

## 관제 사용

기존 다중 카메라 화면에서 음성을 켜고 HLS를 선택할 수 있다. 라이브 시청 시
되감기 캐시를 준비하며 캐시가 쌓인 범위에서 되감기·실시간 복귀를 제공한다.
녹화 카메라를 선택해 시작·중지하거나 한 번/매일/매주 예약을 설정한다.
시간 구간으로 완료된 MP4를 검색하여 브라우저에서 재생·탐색·다운로드한다.

녹화 상태·예약·색인은 `recording_dir/archive.sqlite`에 저장한다. 기존 MP4는
시작 시 가져오며, 파악할 수 없는 과거 파일의 길이는 추정값으로 꾸미지 않고
`estimated`로 구분한다. 카메라 장애와 저장 한도 초과를 화면에 표시한다.
기존 녹화를 자동 삭제하지 않는다. RAM 되감기 캐시는 재시작 때 사라지고,
완료된 MP4 및 예약은 유지된다.

API·시간 구간·용량·HLS GOP 설정은 [surveillance-archive.md](surveillance-archive.md)를 참조한다.
HLS는 세그먼트를 기다리므로 WebRTC보다 지연이 크며 LL-HLS는 아니다.
Safari/iOS/Android 실제 장치 전체 호환성은 아직 시험하지 않았다.

## 검증 자료

- `test-results/surveillance-final-audio/audio.json`: 합성 AAC·PCMU·PCMA·Opus의
  MSE·HLS 재생과 WebAudio RMS 소리 출력 확인. 이 실행에서 발견한 WebRTC
  재생 시작 경쟁은 UI에서 수정했고 `surveillance-final-webrtc/audio.json`의
  네 코덱 재검증으로 통과를 확인했다.
- `test-results/surveillance-final-cold-gated/audio.json`: HLS 첫 접속 실패를
  수정한 뒤 캐시가 없는 상태에서 네 코덱 모두 영상·음성 재생 확인.
- `test-results/surveillance-final-ui-pass/result.json`: 수동 녹화, 예약 변경,
  검색, 완료 녹화 브라우저 재생, Range/attachment 다운로드, HLS·되감기·복귀.
- `test-results/surveillance/archive.json`: 수동/예약 시작·종료, 색인,
  Range 206/416, HLS/DVR 미디어.
- `test-results/surveillance/archive-persistence.json`: 실제 서비스 재시작 후
  예약 및 기존 AAC 녹화 8개의 메타데이터 일치.
- `test-results/surveillance/archive-media-validation.json`: AAC 녹화와 HLS를
  ffprobe로 검증. 영상·음성 타임스탬프 증가와 디코딩 가능 프레임 확인.
- `test-results/surveillance/recording-quota.json`: 저장 한도 초과 후 기존 파일
  불변, 무인증 목록·다운로드, 정상 종료 시 partial 완료, 자식 프로세스 없음.
- `test-results/surveillance/original-sdk/`: 변경하지 않은 추출 원본 SDK의
  WebRTC/MSE 재생. SDK 코드는 배포본에 포함하지 않는다.
- `test-results/rust-surveillance/audio-rtsp.json`: 네 음성 코덱의 RTSP 입력과
  재송출을 각각 5초 준비 후 30초 측정. SR 동기화·음성 PTS 증가·영상/음성
  진행 길이를 확인했다. PCMU·PCMA는 변경하지 않은 원본 SDK에서도 재생했다.
- `test-results/rust-surveillance/wall/result.json`: 네 카메라를 WebRTC·MSE·HLS로
  동시에 시청하면서 각 영상 및 음성 진행, 카메라당 하나의 RTSP 입력,
  WebRTC 미디어 UDP 한 포트 공유를 확인했다. mDNS 보조 소켓은 별도다.

합성 입력 시험이며 카메라 자체 설정을 바꾸지 않았다. 새 기능 전체의 최대
동시 접속 성능 비교를 수행한 것은 아니다. 기존 최적화의 전후 측정은
`rust-optimization.md`에 있으며 이번 기능 추가만으로 성능 우위를 주장하지 않는다.

RTMP 입력·브라우저 카메라 송출·MKV/ASF·Flash·사용자 권한은 이번 관제 범위에
포함하지 않았다. 사용자 요청에 따라 관제를 먼저 구현하며 RTMP 확장은 후순위다.
배포 파일 용량과 SHA256은 `dist/native-manifest.json`에 기록한다.
