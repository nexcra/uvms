# 원본 제품 기능과 현재 Rust·Java의 차이 — 2026-10-14

> 이 문서는 v0.3.0 작업 전의 기능 차이 기록이다. 이후 Rust에 음성·HLS·되감기·수동/예약 녹화·SQLite 검색·Range 재생을 추가했다. 최신 범위는 [rust-surveillance.md](rust-surveillance.md)를 참조한다. Java에는 이번 확장을 적용하지 않았다.

현재 구현은 원본에서 분석한 RTSP H.264 영상 입력·실시간 재생을 중심으로 한 Linux 서비스다. 원본 제품 설명의 전체 기능과 같지는 않다. 이 문서는 추가 기능을 구현했다는 보고가 아니라, 소스와 기존 원본 증거를 대조한 범위표다. 같은 날 진행하는 Rust 성능 변경과는 별개이며 해당 변경의 측정 결과를 대신하지 않는다.

## 확인 범위

- 원본 제품의 9개 항목: 사용자가 제공한 제품 설명. 전체 원본 실행으로 검증한 사실과 구분한다.
- 원본 배포 설명: MSI의 `UServerConfig.chm`에서 이미 추출한 `evidence/runtime-plan/help/` 및 대응 `.txt`, 기존 SDK 설명을 재사용했다. 문서에 기능이 명시되어 있어도 바이너리 실행을 관찰했다는 뜻은 아니다.
- 원본 네이티브: 기존 `docs/original-{rtsp,server,webrtc}-native.md` 및 `docs/original-protocol.md`에 연결된 REA Evidence를 재사용했다. 이번 비교를 위해 새 네이티브 세션이나 원본 서비스 실행을 하지 않았다.
- 현재 구현: Rust `src/main.rs`, `src/config.rs`, `src/mse.rs`, `src/record.rs`, `ports/unreal-mp4/src/lib.rs`; Java `Gateway.java`, `Hub.java`, 양쪽 README와 기존 검증 보고서. 부재 판정은 이 서비스의 소스·설정 범위다.

## 9개 제품 항목 대조

| 원본 설명 | Rust | Java | 현재 의미와 남은 작업 |
|---|---|---|---|
| 1. 자기 서버에서 실행 | 지원·Linux 서비스 검증 | 지원·Linux 서비스 검증 | 외부 호스팅 서비스가 필요 없는 설치형 서비스다. |
| 2. 브라우저·RTMP·RTSP·모바일·웹캠·캡처카드의 실시간 음성/영상 발행 | RTSP/TCP H.264 영상 입력만 | RTSP/TCP H.264 영상 입력만 | WebRTC는 수신자에게 보내는 역할만 한다. 브라우저의 카메라 발행, RTMP push/pull, 음성 입력, 캡처 장치 입력은 없다. 장치가 지원되는 RTSP 출력을 제공할 때 간접 연결하는 경우와 장치 직접 캡처는 다르다. |
| 3. WebRTC·MSE·Flash·Unreal Player·HLS 재생 | WebRTC·MSE·RTSP 재송출 지원 | WebRTC·MSE·RTSP 재송출 지원 | **HLS playlist/segment 제공은 양쪽 모두 없다.** 원본 UMS 전용 플레이어 프로토콜과 Flash/RTMP 출력도 없다. 원본 웹 SDK 호환 시험과 Unreal 전용 플레이어 지원은 다르다. |
| 4. 라이브 되감기(time-shift) | 미지원 | 미지원 | pause 중 프레임을 폐기하고 continue 때 현재 IDR부터 다시 받는다. 과거 영상을 저장해 되감거나 중단 지점부터 이어 보는 DVR가 아니다. |
| 5. 모든 주요 브라우저·OS·모바일 | 일부 데스크톱 Edge 검증 | 일부 데스크톱 Edge 검증 | Safari/iOS/Android/Firefox 전체 호환성을 보장하지 않는다. 코덱·자동재생·네트워크/NAT 조건을 포함한 장치별 시험이 필요하다. |
| 6. 수동/예약 MP4·MKV·ASF 녹화 | 설정 기반 분할 MP4만 | 설정 기반 분할 MP4만 | `record=true`와 재시작으로 활성화한다. 실행 중 수동 시작/중지 API, 주간/기간 예약, MKV/ASF mux는 없다. |
| 7. 검색 가능한 재생 녹화 DB | 파일 목록만 | 파일 목록만 | 디렉터리를 나열하는 API다. 카메라·시각 범위의 색인 DB, 녹화 구간/공백, 타임라인·seek 재생은 없다. |
| 8. 다단계 접근 제어·사용자 권한 | 사용자 요청에 따라 제외 | 사용자 요청에 따라 제외 | 카메라 자체 RTSP 자격 증명만 사용한다. 이 항목을 보완한다는 이유로 토큰·로그인을 다시 추가하지 않는다. |
| 9. 녹화 파일 다운로드 | 지원·검증 | 지원·검증 | 완료된 MP4의 목록과 HTTP 다운로드가 있다. 권한별 운영자 구분 및 다운로드 재개/HTTP Range 처리는 현재 구현하지 않았다. |

오디오·H.265·B-frame·트랜스코딩은 현재 범위 밖이다. HLS 출력만 추가해도 이러한 입력 제한이 자동으로 해결되지는 않는다. 같은 URL을 서로 다른 별칭에 등록하면 별도 입력 연결이 만들어지는 점도 양쪽 공통이다.

## HLS: 원본에서 확인된 내용과 이식 선택

**배포 문서 관찰:** `evidence/runtime-plan/streaming.txt:3–23`은 원본이 `.ts`와 `.m3u8`을 지정 폴더에 생성하고 별도 웹서버가 전달하는 방식을 설명한다. `Networking.txt:13–17`도 HLS 시청자가 미디어 서버에 직접 접속하지 않는다고 설명한다. HLS는 사용자가 인용한 홍보 문구에만 존재하는 기능이 아니다. 다만 원본 TS mux 함수와 실제 출력 파일은 이번에 분석·실행 검증하지 않았다. 설명서의 세그먼트 설정값은 실제 종단간 지연 측정치가 아니다.

**구현 제안:** 기존 H.264 fMP4 mux를 활용하는 HLS는 가능한 확장이다. Apple의 HLS 규격은 H.264에 fMP4 또는 MPEG-TS를 허용한다. 기존 init/fragment 생성기를 재사용하고 카메라별 IDR 경계 세그먼트·공유 저장소·playlist·HTTP 제공을 추가하면 영상 재인코딩이나 FFmpeg 런타임 없이 구성할 수 있다. 이는 원본의 TS 출력 알고리즘을 그대로 이식하는 것과는 다른, 기존 분석 결과에 연결한 확장 설계다. 실제 이식에 앞서 원본 TS 경로와 요구하는 플레이어의 호환 범위를 정하면 된다. [Apple HLS authoring specification](https://developer.apple.com/streaming/hls-authoring-specification-for-apple-devices.html)

기존 MP4 조각을 단순히 `.m3u8`에 나열하면 끝나는 작업은 아니다. 초기화 세그먼트 참조, 연속적인 decode time, IDR 경계, 입력 재연결/코덱 변경 시 discontinuity, 파일 공개 순서, 보관 한도와 HTTP cache 정책을 함께 설계해야 한다. MSE의 원본 전용 10/20바이트 헤더는 HLS 미디어에 포함하지 않는다. 기존 녹화기처럼 파일마다 시간을 0으로 만드는 동작도 연속 라이브 playlist에 그대로 적용할 수 없다.

HLS는 HTTP 배포·캐시·녹화 재생을 결합할 때 유용하다. 다만 일반 HLS가 현재 WebRTC와 같은 지연을 보장하지 않는다. LL-HLS는 partial segment, blocking playlist reload, preload hint 등의 추가 동작이 필요하므로 별도 단계로 검증한다. [Apple Low-Latency HLS](https://developer.apple.com/documentation/http-live-streaming/enabling-low-latency-http-live-streaming-hls)

## 카메라 관제 용도에서의 보완 순서

1. **HLS 및 녹화 색인·타임라인:** 같은 카메라의 압축 데이터를 공유하고, 카메라/시작·종료 시각/파일/코덱/공백 메타데이터를 기록한다. 목록 전체를 매번 스캔하는 방식 대신 범위 검색이 가능하게 한다. HLS 플레이어/장치 시험과 저장 실패·재시작 복구를 함께 다룬다.
2. **DVR·라이브 되감기:** 보관된 세그먼트와 색인을 이용한 범위 seek, 실시간 복귀, 여러 카메라의 시간축을 구현한다. 현재 MSE pause와 다르다. 원본 SDK의 `seek:{seconds}`와 timeshift 필드는 정적 분석되어 있으나 원본 저장/회수 엔진 전체를 복원한 상태는 아니다.
3. **수동·예약 녹화:** 재시작 없이 시작/중지하고, 예약의 시간대·중복·재시작 후 복원·입력 장애를 처리한다. 단순 MP4 파일 생성과 별도의 상태 관리 작업이다.
4. **음성:** RTSP 오디오 depacketizer, A/V 시각 동기화, 출력별 코덱 협상을 추가한다. 원본 RTSP DLL에는 AAC/LATM/Opus/PCMU/PCMA 선택 분기가 있고 원본 WebRTC에는 Opus/PCMU/PCMA 경로가 확인됐지만 현재 이식에는 포함되지 않았다. 코덱이 출력과 맞지 않는 경우 무조건 passthrough로 해결되지는 않는다.
5. **발행 입력·추가 장치:** 브라우저/모바일 발행이 필요하면 WebRTC ingest, OBS 등 인코더가 필요하면 RTMP를 별도 확장한다. RTSP 카메라 관제만 필요한 환경에서는 우선순위를 낮출 수 있다. H.265·RTSP/UDP·SPS/PPS 없는 SDP 지원도 장비 요구와 실제 샘플에 맞춰 판단한다.

운영 완성도에서는 장시간 다중 카메라 시험, 입력 장애/포맷 변경, 느린 시청자, 디스크 고장·용량 도달, 재연결, 손실/NAT를 추가 검증해야 한다. 현재 Java 녹화기의 예외 뒤 `finally`가 partial을 publish하는 경로는 디스크 오류에서 불완전 MP4를 공개할 가능성이 있으므로 보완 대상이다. Rust는 해당 실패 파일을 공개하지 않는 경로가 있다. 이 문서 작성에서 코드를 수정하거나 이 장애를 재현하지는 않았다.

Flash 출력은 새 개발 우선순위에서 제외하는 편이 적절하다. Adobe는 Flash Player 지원을 2020-12-31에 종료했고 2021-01-12부터 실행을 차단했다. MKV/ASF와 UMS 전용 플레이어는 연결할 레거시 시스템이 있을 때 판단할 별도 호환 작업이다. [Adobe Flash Player EOL](https://www.adobe.com/products/flashplayer/end-of-life.html)

## 근거 목록과 남은 불확실성

| 결론 | 근거 유형·출처 | 한계 |
|---|---|---|
| 원본 HLS TS 파일/playlist 출력 | MSI 포함 help: `streaming.txt:3–23`, `Networking.txt:13–17` | 문서 근거. 원본 바이너리 HLS 실행·mux 구현 미확인 |
| 원본 time-shift wire field와 seek | SDK 정적 분석: `docs/original-protocol.md:64–93`; EXE media field `0x14009abf0`, `ev_d885c7e98de82df037906478a0edccd4e0cd16108903b3e95a5179ba1a8dbe7d` | 버퍼 보관/검색/동시 세션 전체 실행 미검증 |
| 원본 오디오 경로 | RTSP `0x180007880`, `evidence/native/rtsp-v2/021-pseudo`; WebRTC `0x1800a08a0`, `ev_4f824c3d3c220e60be98c69e81ac3c867d4a0f83911b41af2180a76f61314aa6` | 정적 분기·미디어 형식 근거. 오디오 E2E 실행 미검증 |
| 수동/예약 녹화는 별도 원본 구성요소와 관련 | SDK `Archival Server/Recording Control/Readme.htm:129–136`의 UArchivalServer.exe와 `:236–390`의 manual/scheduled 상태 | 사용자 제공 Media Server MSI만으로 Archival 제품 전체 알고리즘을 복원했다고 할 수 없음 |
| 현재 HLS/DVR/DB/예약 없음 | 위 두 서비스의 라우팅·녹화·설정 소스 대조 | 향후 변경 시 갱신 필요 |
| 현재 재생/녹화 검증 범위 | `docs/native-service-verification.md`, `docs/java-comparison.md`, `docs/java-optimization.md` | 그 보고서에 기재된 환경만 검증. 모든 플랫폼·최대 용량 보장 아님 |

9개 항목의 현행 차이는 확인했다. 원본 HLS mux, time-shift 저장 엔진, Archival 녹화 DB 전체 구현은 미해결이며 다음 분석 대상으로 남긴다. 이 문서의 우선순위는 카메라 관제라는 현재 용도에 대한 제안이고, 추가 기능 구현 완료나 사용자 승인으로 취급하지 않는다.
