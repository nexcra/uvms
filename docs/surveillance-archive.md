# Rust 관제 녹화·검색·HLS

이 기능은 원본 CHM의 HLS·녹화 요구사항에 맞춘 Rust 확장이다. 원본에서
확인한 HLS는 TS와 m3u8 파일을 외부 웹 서버로 제공하는 방식이다. 이 구현은
기존 Rust MP4 mux를 재사용한 **fMP4 HLS**이며, 원본 TS mux 알고리즘의
복원이라고 주장하지 않는다. FFmpeg 프로세스를 실행하지 않는다.

## HLS 유휴 자원 회수

`/api/hls/{stream}` 상태 조회는 카메라 연결이나 HLS 생성을 시작하지 않는다.
플레이리스트 요청으로 카메라별 생성 작업을 시작하며, 동시에 여러 요청이
들어와도 작업과 입력 연결은 공유한다. 재생 목록 또는 존재하는 미디어
조각을 요청하면 유휴 시간이 갱신된다. 상태 조회는 유휴 시간을 갱신하지 않는다.

기존 웹의 WebRTC/MSE 시청 중 되감기 준비는 유지한다. 플레이어가 활성인
동안 `POST /api/hls/{stream}/keepalive`를 5초마다 호출하고, 해제·전환·화면
종료 시 갱신을 중지한다. 이는 인증 토큰이나 로그인 절차가 아닌 재생 수요
알림이며 기존 변경 API와 같은 origin 검사를 적용한다. HLS 자체 재생은
재생 목록·조각 요청으로 갱신하므로 이 알림을 따로 보내지 않는다.

`hls_idle_seconds`(기본 60, 허용 15~86400초) 동안 재생 요청이 없으면 HLS
작업과 RAM 되감기 캐시를 해제한다. 이후 재접속은 새 IDR·재생 구간을
기다리며, 이전 URL을 재사용하지 않는다. 유휴 종료 전의 RAM 되감기 영상은
남지 않는다. 활성 시청 중의 `hls_window_seconds` 정책과 디스크 녹화는 유지한다.
다른 WebRTC/MSE/녹화/RTSP 소비자가 있으면 공통 입력은 계속 유지된다.
모든 소비자가 없으면 기존 입력 유휴 유예 시간(약 30초) 후 카메라 연결도 닫힌다.

상태 응답에 `running`, `idle_seconds`가 추가됐다. 이는 별도 HLS 생성 작업의
상태이며 카메라 전체의 온라인 여부와는 다르다. 카메라가 중단되어 연결을
재시도하는 중에도 HLS 유휴 종료 시간은 적용된다.

`scripts/verify-hls-idle.py <새 바이너리> --baseline <기존 바이너리>`로
합성 영상에 대한 상태 조회·유휴 해제·재접속·동시 요청·녹화 유지 회귀를
검증한다. 기존 서비스와 카메라 설정을 사용하지 않는 별도 시험이다.

2026-10-14 v0.4.1 검증: 합성 H.264 640×360 입력, HLS 목표 2초,
보관 창 30초, 시험 유휴 시간 15초(운영 기본값은 60초), 동시 재생 목록
요청 8개를 사용했다. 변경 전에는 상태 조회만으로 RTSP 연결 1개가 생겼고
재생 요청 종료 18초 후에도 캐시 3,884,779바이트와 입력 1개가 유지됐다.
변경 후 상태 조회만으로 연결이 생기지 않았고, 유휴 종료 시 캐시 보관량은
0바이트, 추가 입력 유예 시간 후 RTSP 연결도 0개였다. 녹화 활성 상태에서는
HLS만 정리되고 입력 1개와 녹화가 계속됐다. 이는 유휴 자원 회수 검증이며
전체 RSS가 같은 바이트만큼 감소하거나 활성 시청 처리량이 향상됐다는 뜻은 아니다.

26개 Rust 테스트와 Clippy, 수동/예약 녹화·Range 다운로드·HLS 조각과 되감기
API 회귀를 통과했다. WebRTC/MSE 음성, HLS 신규 연결과 순차 재생의 실제
음성 샘플도 확인했다. 최초 녹화 회귀와 병행한 음성 검사에서 Edge 네이티브
HLS에 DEMUXER_ERROR_COULD_NOT_PARSE가 1회 발생했으며, 별도 신규 연결 및
순차 재검사는 통과했다. 같은 조건의 기존 바이너리 검사도 통과해 이 1회의
당시에는 원인을 확정하지 못했다. 아래 v0.4.2 검증에서 동일한 오류를 재현하고
영상 조각 경계의 음성 누락을 수정했다.

`scripts/browser-hls-idle.mjs`는 웹의 WebRTC 시청 중 되감기 캐시 유지,
해제 후 캐시 회수, 재접속 시 재생 구간 재생성을 별도 시험 포트에서 확인한다.

## v0.4.2 HLS 음성 경계 오류

원인은 `src/hls.rs`가 음성의 보정된 `timestamp_90k`를 현재 **영상 조각의
시작 시각**과 비교해 이전 시각이면 버리던 조건이다. 영상·음성은 서로 다른
수신 큐를 사용한다. RTCP Sender Report로 음성 시각을 앞당기거나 음성 도착이
늦어지면, 새 영상 IDR 이후에도 이전 영상 구간에 해당하는 정상 음성이 온다.
이를 버리면서 매 조각마다 음성 구간에 구멍이 생겼다.

수정 후에는 최초 IDR로 정한 **mux 전체의 시작 시각** 이전 음성만 제외한다.
그 이후 음성 AU는 영상 조각 전환과 관계없이 원래 AAC 바이트, 보정된 시각,
1024샘플 길이를 그대로 유지한다. 조각 바이트 제한과 HLS 유휴 정리는 유지한다.
추가 인코더·FFmpeg 프로세스·인증은 없다. RTCP 시각 보정 자체를 끄거나
오류를 플레이어 재시작으로 감추는 변경도 아니다.

재현은 실카메라 대신 합성 H.264 + AAC 48kHz 2채널/440Hz 입력에 음성 SR의
NTP 시각만 -4초 이동시킨 별도 RTSP 프록시로 진행했다. v0.4.1에서 네이티브
Edge HLS가 `DEMUXER_ERROR_COULD_NOT_PARSE`로 중단됐고, DevTools Media 로그의
하위 오류는 `HlsDemuxerStatus::Loaded ranges conflict with media time`이었다.
오류 시 재생 시각은 36.846954초, 가용 구간은
`[[35.184392,36.202058],[38.31362,39.209286]]`이었다.
HTTP 요청은 정상이며 코덱 변환 오류도 없었다. 저장한 18개 미디어 조각에서
음성 DTS 공백은 최대 **1097ms**였다.

같은 시계 오차 조건에서 수정 후 90초 음성 분석은 통과했고, 실제 재생 시각은
95초 이상 진행했다. 최초 수정 시험은 900개 분석 창 중 869개에서 음성을
검출했고, 최종 v0.4.2 첫 시험은 871개였다. 최종 첫 시험의 저장 조각 50개에서
음성 DTS의 양의 공백은 0ms, 브라우저 미디어 오류는 0개였다. 이 수치는
의도적으로 큰 시계 차이를 가한 단기 회귀 결과이며 모든 카메라나 브라우저의
장기 안정성·무음 없는 완벽한 동기화를 주장하지 않는다. RTCP slew 중 작은
AAC 시간 겹침 및 시계 변환 반올림은 기존 동작으로 남는다.

최종 반복 결과: 네이티브 HLS 90초 × 2회에서 음성 871/900, 881/900창,
hls.js 90초에서 866/900창을 확인했고 모두 재생이 진행되며 미디어 오류가
없었다. 일반 합성 AAC·PCMU·PCMA·Opus 입력은 네이티브 HLS와 hls.js에서
각각 100/100창 음성을 검출했다. AAC WebRTC/MSE, 수동·예약 녹화, Range
다운로드, HLS/DVR API, 상태 조회·유휴 회수·재접속 검사도 통과했다.
Rust 테스트 28개, fmt, Clippy를 통과했다. 실제 카메라 음성 및 Safari/iOS의
검증 결과는 포함하지 않는다.

회귀 자료는 로컬 `test-results/hls-audio-fix/`에 저장한다. 특히
`skew-before/aac-hls-0-media.json`은 실패 원인,
`final-skew-native/audio.json`은 최종 반복 재생,
`timeline-summary.json`은 실제 조각의 시간축 비교다. 합성 미디어 캡처는
Git 및 배포 패키지에 포함하지 않는다.

재현 절차(기존 `surveillance-fixture.py` 합성 송출기 필요):

```sh
# Linux/WSL: 독립 프록시 18565, Rust HTTP 8097/RTSP 18561/UDP 51700
python3 scripts/hls-audio-skew-fixture.py --binary /absolute/path/uvms-gateway
# 별도 터미널: Node + Playwright/Edge (Windows에서는 $env:NAME='value' 사용)
UVMS_TEST_URL=http://127.0.0.1:8097 UVMS_TEST_STREAMS=aac \
UVMS_TEST_MODES=hls UVMS_COLD_HLS=1 UVMS_AUDIO_WINDOWS=900 \
UVMS_TEST_REPEATS=2 UVMS_MIN_AUDIO_RATIO=0.9 UVMS_CAPTURE_HLS=1 \
UVMS_TEST_OUTPUT=test-results/hls-audio-regression node scripts/browser-audio.mjs
```

`UVMS_FORCE_HLS_JS=1`로 같은 입력을 hls.js 경로에서도 검사한다. 시험 스크립트는
미디어 오류를 반드시 실패로 처리하고, 실제 음성 샘플·재생 진행·침묵 구간과
[DevTools Media 진단](https://chromedevtools.github.io/devtools-protocol/tot/Media/)
이벤트를 기록한다. 종료 시 프록시 터미널에서 Ctrl-C로 임시 서비스를 정리한다.

## 녹화 제어

`GET /api/recording/{stream}`으로 상태를 읽고 같은 주소에 JSON을 POST한다.
추가 서비스 인증 없이 기존 카메라 접속 설정을 사용한다. 브라우저의 다른
사이트에서 보내는 변경 요청은 기존 same-origin 검사로 제한한다.

```json
{"mode":"auto","schedules":[{"start_ms":1791936000000,"end_ms":1791939600000,"repeat_seconds":86400}]}
```

- `on`: 즉시 녹화를 요청한다. 첫 IDR을 받은 시점부터 재생 가능한 파일을 쓴다.
- `off`: 현재 파일을 정상 종료하고 녹화를 중지한다.
- `auto`: 예약 목록이 있으면 해당 구간에만 녹화한다. 목록이 비어 있으면
  기존 `streams.{id}.record` 설정을 따른다.
- 시간은 UTC Unix 밀리초이며 구간은 `[start_ms,end_ms)`이다.
- `repeat_seconds`를 생략하거나 null이면 한 번만 실행한다. 86400은 매일,
  604800은 매주다. UTC 고정 주기이므로 일광절약시간의 벽시계 이동을 따르지 않는다.
- POST는 mode와 전체 예약 목록을 교체한다. 최대 64개다. 수동 상태와 예약은
  `recording_dir/archive.sqlite`에 저장되어 서비스 재시작 후에도 유지된다.

응답은 `mode`, `schedules`, `active`, `status`를 포함한다. `active`는 녹화
의도이며, 실제 상태는 `connecting`, `recording`, `off`, `scheduled`,
`retrying`, `storage_full` 등으로 구분된다. 카메라에 연결할 수 없으면
`active:true`여도 아직 파일이 생성되지 않을 수 있다.

녹화 파일은 `.partial`에 쓰고 flush·sync 후 `.mp4`로 원자적으로 바꾼 뒤
검색 색인에 등록한다. 디스크 쓰기 실패의 불완전 파일은 공개하지 않는다.
전체 녹화 용량 한도에 도달하면 중지하며 기존 녹화를 자동 삭제하지 않는다.
한도에는 영상 파일과 남은 partial 파일이 포함되고 SQLite 메타데이터는
포함하지 않는다. 외부에서 파일을 정리한 뒤 용량 재계산은 재시작으로 수행한다.

## 검색·재생·다운로드

`GET /api/recordings?stream=camera&from=...&to=...&limit=200&offset=0`

카메라 및 시간 구간과 겹치는 파일을 최신순으로 반환한다. limit은 최대
1000이며 offset으로 추가 결과를 요청한다. 각 파일에는 `start_ms`,
`end_ms`, `bytes`, `url`, `estimated`가 있다. 기존 파일은 시작 시각을 파일
이름에서, 길이를 MP4 tfdt/trun 메타데이터에서 읽는다. 해석할 수 없는 이전
파일은 길이를 추정하지 않고 `end_ms=start_ms, estimated=true`로 표시한다.
외부에서 삭제한 파일이나 심볼릭 링크는 결과에서 제외한다.

파일 URL은 `video/mp4`와 HTTP Range를 지원하여 브라우저에서 탐색할 수
있다. `?download=1`은 attachment 다운로드다. 단일 범위·suffix 범위·끝까지
범위를 처리하고 잘못된 범위에는 416을 반환한다. 파일 전체를 RAM에 읽지 않는다.

## HLS와 라이브 되감기

- 라이브: `/hls/{stream}/index.m3u8`
- 되감기: `/hls/{stream}/index.m3u8?from={UTC Unix ms}`
- 상태: `/api/hls/{stream}`의 `oldest_ms`, `newest_ms`, `ready`, `error`

첫 HLS 요청 또는 상태 요청이 해당 카메라의 작업을 시작한다. 작업은 다른
출력과 동일한 Hub 입력을 공유하고 서비스 종료까지 최근 영상을 유지한다.
오래된 캐시보다 이른 from은 캐시의 가장 이른 시점으로 제한된다. from은
가까운 이전 IDR 세그먼트 경계부터 재생한다. 일반 HLS이므로 WebRTC보다
지연이 크며, Low-Latency HLS는 구현하지 않았다.

기본 캐시 기간은 300초, 카메라당 완료 세그먼트 예산은 128MiB다.
현재 생성 중인 세그먼트도 별도로 같은 크기로 제한되므로 최악의 순간에는
그 두 배와 mux/입력 버퍼가 필요할 수 있다. 여러 카메라는 독립된 캐시와
작업을 사용한다. 바이트 한도가 기간보다 먼저 도달하면 실제 되감기 기간은
짧아진다. 캐시는 RAM에만 있으며 재시작 시 사라진다. 영구 MP4 녹화는
별도 검색·재생 경로를 사용한다.

`hls_segment_seconds`는 고정 TARGETDURATION이기도 하다. IDR 간격이 이
값보다 길면 재생 가능한 세그먼트를 잘라 낼 수 없으므로 카메라 GOP를
줄이거나 설정을 늘려야 한다. 규격의 반올림 EXTINF 제한을 초과한 세그먼트는
공개하지 않고 재연결한다. 첫 재생 목록은 합계가 TARGETDURATION의 3배
이상이 되는 완료 세그먼트가 준비된 뒤 반환한다. [RFC 8216 §6.3.3](https://www.rfc-editor.org/rfc/rfc8216#section-6.3.3)의
라이브 시작 지점 권고에 맞춘 것으로, 기본 2초 설정에서는 약 6초 분량이다.
첫 요청은 `3 × hls_segment_seconds + 15`초(최대 45초)까지 기다린 뒤
준비되지 않으면 Retry-After가 있는 503을 반환한다. 입력 재연결 시 새 init 파일과
DISCONTINUITY를 사용하며, 서비스 재시작 후 이전 캐시 URL도 재사용하지 않는다.

HLS와 MP4 음성은 공유 입력의 정규화된 AAC 트랙을 사용한다. AAC 카메라의
압축 음성을 보존하고 G.711/Opus 입력은 별도 공유 음성 변환 경로를 따른다.
분기별 미디어 코덱과 A/V 동기화 제한은 음성 모듈 문서를 함께 확인한다.

## 검증

단위 검증은 일정 경계·반복·수동 우선순위, SQLite 재열기, Range 경계,
이전 MP4 길이 해석, 캐시 용량·기간 제거, HLS 불연속·되감기 목록을 다룬다.
`scripts/verify-surveillance-archive.py`는 별도 테스트 서비스에서 실제
녹화 시작·종료, 예약 시작·종료, 색인, Range 206/416 및 HLS/DVR 미디어
다운로드를 검사한다. 8080과 8088 운영 서비스에는 실행되지 않도록 제한한다.

2026-10-14 별도 8092 서비스에서 다음을 확인했다.

- 수동 녹화 시작·종료와 예약 구간의 실제 시작·종료 통과.
- 검색 색인의 시작·끝 시각 및 파일 바이트 수, Range 206·suffix·416 통과.
- HLS 라이브와 from 되감기 목록에서 실제 세그먼트 다운로드 통과.
- MP4 녹화와 3개 HLS 세그먼트 모두 H.264 영상과 48kHz 스테레오 AAC
  음성 트랙 확인. 녹화 16 영상/47 음성 패킷, HLS 90 영상/283 음성 패킷의
  각 트랙 PTS·DTS가 엄격하게 증가했다. 이는 미디어 타임스탬프 검증이며
  현장 카메라의 입술 동기화나 모든 브라우저 호환성 검증을 의미하지 않는다.
- 실제 서비스 재시작 후 반복 예약 및 기존 8개 녹화의 시각·바이트·확정
  표시가 그대로 유지됨을 확인했다. 테스트한 opus 카메라의 제어 상태는
  이전 값으로 복원했고 녹화 파일은 삭제하지 않았다.

검증 결과는 `test-results/surveillance/archive.json`,
`archive-media-validation.json`, `archive-persistence.json`에 보존한다.

첫 접속의 1개 세그먼트 목록에서 Edge 네이티브 HLS가
`DEMUXER_ERROR_COULD_NOT_PARSE`로 실패하는 문제를 별도로 재현했다.
처음부터 3 target duration 이상을 제공하도록 수정한 뒤, 서비스를 새로
시작하여 AAC·PCMU·PCMA·Opus 입력 네 종류 모두 첫 목록에 2초 세그먼트
3개가 포함되고 영상과 음성이 재생되는 것을 확인했다. 각 입력의 오디오
검사 40개 구간 모두 테스트 음이 검출됐고 MediaError는 없었다.
근거: `test-results/surveillance-final-cold-gated/audio.json`.
