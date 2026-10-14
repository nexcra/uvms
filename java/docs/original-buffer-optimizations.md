# 원본 압축 프레임·NAL 처리와 Java 최적화 근거

분석일: 2026-10-14. 원본 정적 Evidence를 재검토하고, 확인되지 않았던 내부
샘플 생성·참조·큐 함수를 별도 REA 세션 `evidence/native/buffer-engine/`에서
추적했다. 원본 서버를 실행하거나 COM 등록·서비스 설치를 하지 않았다.

대상은 `UMediaServer.exe` SHA-256
`19fdb1d0888b38a7b5886a57cce774d9ddf0abc7fe02a62ee6e3673ca8b2a1e8`와
`UnrealWebRTC.dll` SHA-256
`3a283df31ced9fdca9cf02d367213b0be0bf01394744d56ff85f9ce627a75287`이다.
주소는 preferred image base 기준이며 함수명·지역변수명은 Ghidra 복원명이다.
이 문서의 '관측'은 별도로 표시하지 않으면 **정적 코드 관측**이다.
공유 UDP 포트의 별도 엔진 분석은 [original-shared-udp.md](original-shared-udp.md)에 있다.

## 확인 질문과 상태

| 질문 | 상태 | 근거와 범위 |
|---|---|---|
| WebRTC용 NAL 정규화를 시청자마다 반복하는가? | 확인 | 한 입력 프레임의 소비자 순회에서 `local_58`를 한 번 만들고 재사용 |
| 전달되는 NAL은 복사된 개별 배열인가? | 확인 | 하나의 압축 payload와 offset/length trailer이며 RTC callback에 원 payload 주소 전달 |
| 전체 경로가 zero-copy인가? | 아님 | normalizer의 명시적 allocation·memcpy와 최종 RTP payload 복사가 존재 |
| 내부 샘플의 참조계수·큐 소유권은 무엇인가? | 확인한 경로 답변 | 최초 payload 복사, 공통 큐 포인터 공유·참조계수, 특수 분기 deep copy |
| 지연된 소비자는 어떻게 처리하는가? | 일부 확인 | 제한모드의 backlog 축소·keyframe gate; mode 설정값·전체 RTC pacing 미확인 |
| 모든 원본 워커 수·스케줄링·pacing을 복원했는가? | 미확인 | 입력 재예약과 callback 경계만 확인; 전체 IOCP·libwebrtc 스케줄러 분석 아님 |
| Java에 그대로 적용할 수 있는 구조는 무엇인가? | 제안 구체화 | 입력당 NAL 인덱스 공유, 최종 RTP 버퍼에 직접 복사; 아래 변경 경계 참고 |

## 1. 입력당 한 번 정규화하고 여러 RTC 소비자에게 재사용

서버 `0x1400265f0`, 보존 파일
`evidence/native/server-v2/039-sink-source.c`, Evidence
`ev_fa6e29482ae06bdba06ae20a478140ad41ea593409879fecef88bdb172985850`.

함수는 provider vtable `+0x28`에서 한 sample을 받은 다음 소비자 목록을 순회한다.
순회 전에 `local_58 = null`로 둔다. 연결 종류 `0xb/0xc`의 video 분기에서
변환이 필요한 경우, `local_58`가 null일 때만 `0x140025dc0(resource, sample)`을
호출해 그 결과를 저장한다. 이후 소비자는 동일한 `local_58`를 IMediaFlip
vtable `+0x28`로 받는다. 순회가 끝나면 원 입력 descriptor와 payload 및
`local_58` descriptor와 payload를 각각 해제한다.

따라서 확인한 이 경로의 정규화 비용은 소비자 수 N에 비례해 N번 반복되지 않는다.
소비자별 callback 호출·패킷화·암호화·송신 비용까지 공유한다는 의미는 아니다.
또한 이 포인터를 callback 반환 후에도 무조건 보관할 수 있다는 의미가 아니다.
원본은 순회 후 버퍼를 해제하므로 하위 계층이 보관한다면 별도 소유권 처리가
필요하다. 그 하위 모든 소유권 전이를 이 함수 하나로 확정할 수는 없다.

## 2. 인코딩 데이터와 NAL 인덱스를 분리

서버 정규화 함수 `0x140025dc0`,
`evidence/native/server-v2/040-rtc-normalize.c`, Evidence
`ev_0e04335317c183d431da5c8c1b1ffc55822656229939918d34507ed233d373a8`.

- AVCC length prefix 또는 4바이트 Annex-B start code를 순회해 offset/length를 구한다.
- 48바이트 descriptor와 `payload + optional prefix + 4 + nal_count*8` 크기의
  출력 버퍼를 따로 할당한다.
- 입력 payload를 `memcpy`한다. 선택적 keyframe prefix도 별도 `memcpy`한다.
- AVCC이면 복사한 버퍼의 length prefix를 Annex-B start code로 바꾼다.
- payload 끝에 u32 little-endian NAL 개수와 u32 offset/length 쌍을 쓴다.

RTC callback 함수 `0x1800a0b40`,
`evidence/native/webrtc-v2/016-pseudo-0x1800a0b40.c`, Evidence
`ev_2f4f3939c8825885f892694f99f4f2c39f7f26ee7326e21142cac5f0f44e32a9`는
payload 주소·길이를 encoded-image에 그대로 넣고, trailer를 객체의 fragmentation
offset/length 배열로 복사해 callback에 넘긴다. callback의 다섯 번째 ABI 인자인
fragmentation pointer는 assembly Evidence
`ev_3047cc5333de7232c039a1941773260099f3e5dbd92c87512687acb7fb96916a`로 확인했다.

이 함수 본문에는 NAL별 payload 배열 생성이 없다. 객체에 매달린 offset/length
배열에 매번 값을 쓰는 것은 확인되지만, 현재 보존한 constructor·frame 함수만으로는
배열의 초기 capacity·확장·pool 정책까지 확정할 수 없다. '매 프레임 자동 확장'이나
'동적 buffer pool'이 있다고 주장하지 않는다.

## 3. 패킷 계획은 범위를 보관하고 최종 패킷에 복사

RTC packetizer `0x1803dd024`는 fragmentation offset/length로 NAL 주소를 계산한다.
Evidence `ev_57ab17b33823eef7f84281b690efcc67e1b667b7c217bfaf1cd3dceebeb34067`,
`evidence/native/webrtc-v2/033-pseudo-0x1803dd830.c`.
파일 이름과 실제 복원 entry가 다르므로 본문의 entry 주소를 기준으로 한다.
SPS(type 7)는 검사·VUI 수정용 helper와 별도 buffer를 사용할 수 있어,
모든 NAL이 항상 같은 메모리를 끝까지 가리킨다고 일반화하지 않는다.

FU 계획 함수 `0x1803db418`는 원 NAL의 첫 header를 제외한 byte range를
pointer/length와 first/last flag로 큐에 넣는다. 이 함수의 확인된 핵심은
payload 전체 복사 대신 **범위 메타데이터**를 전달하는 것이다.
Evidence `ev_3d7542caa0bd978b4da9cd836fe191c5d10da29b3b361048ac8ead535a997365`,
`evidence/native/webrtc-v2/052-packet-fragment.c`.

FU 출력 함수 `0x1803db0e8`는 최종 packet payload 공간을 얻고 FU-A 2바이트 header를
쓴 다음 계획에 보관한 원 byte range를 그 공간에 복사한다.
Evidence `ev_31789a5a8be06bd405ee7f59495e3d0ea05e50732fc601743bcc7bc938c0ac9d`,
`evidence/native/webrtc-v2/080-pseudo-0x1803db0e8.c`.
최종 전송 payload에는 복사가 있으며 SRTP 보호·retransmission cache 등 추가 경계도
있다. 따라서 이 구조의 이점은 **중간 payload 복사 제거**라고 표현해야 한다.

## 4. 내부 샘플 생성·공유·특수 분기 복사

새 세션 `evidence/native/buffer-engine/010`~`013`의 요청·응답·Evidence와 `.c`에
다음 네 함수를 보존했다. 기존 팬아웃 함수의 호출 위치와 이어서 해석한다.

| 함수 | 관측 | Evidence |
|---|---|---|
| `0x1400227e0` | `payload_length+0x28` inline buffer를 할당하고 descriptor 메타데이터와 payload를 복사. wrapper의 `+0xc` refcount를 0으로 초기화 | `ev_d9234cd9c80f597e158fcda211352a479e55158455fc04f068532cf285a04ff3` |
| `0x140022c10` | admission helper가 수용하면 **같은** sample의 `+0xc`를 원자 증가하고 sample 포인터를 연결 목록에 삽입 | `ev_b79fe8478e5d361e94365669f3e518e8495562b0f5287c1a3b8403d64575fc92` |
| `0x1400229c0` | refcount 원자 감소; 이전 값이 1이면 payload와 wrapper를 free | `ev_4826bf375936919f60812a3fff32ddc1f3e3cd23a5eda9bf519de41719dd4f82` |
| `0x1400228a0` | 새 buffer를 할당하고 기존 inline buffer 전체를 memcpy하는 **deep copy** | `ev_a1b28ed20a936913eefdb3730f6b0407a04a52c06e573a72ab10d3e0ad1af894` |

`0x1400265f0`은 최초 wrapper에 기준 참조 하나를 더하고, 일반 소비자에게
같은 wrapper를 `0x140022c10`으로 넣은 뒤 기준 참조를 해제한다. 특정 소비자
조건 분기에서는 `0x1400228a0`으로 복사한다. 따라서 원본을 '모든 소비자가
항상 무복사'라고 설명하는 것은 틀리며, 확인된 모델은 **공통 payload를 참조로
공유하되 특정 변형 경계에서 복사**하는 것이다.

## 5. 제한된 큐와 keyframe 재동기화

새 Evidence `014-queue-admission`, 함수 `0x140023100`:
`ev_f9952645476c4d5e422b21488487e32b2492df0fca434be40646d7ea1cec226b`.
`+0x4c` mode가 0이면 바로 수용한다. 제한모드에서는 video gate(`+0x48`)가
닫혔을 때 비키프레임을 거부하며, keyframe이 오면 gate를 연다. 큐 count(`+0x38`)가
capacity(`+0x40`)에 도달하면 gate를 닫고 `0x140022fb0`을 호출한다. 현재 프레임이
비키프레임이면 거부하고, keyframe이면 gate를 다시 열어 수용한다.

`015-queue-reset`, 함수 `0x140022fb0`, Evidence
`ev_a1798d1e56d8372fcf6c361d5a47e0af1314b328f6fa13d3b689bc835457a4d2`는
목록의 마지막 node를 순회 종료 경계로 잡아 **마지막 1개를 남기고** 앞의 node들을
제거하며 각 sample 참조를 해제한다. 완전한 queue clear라고 요약하면 부정확하다.
끝에 전역 counter를 `+0x54`에 기록한다. admission의 mode 2에는 추가 backlog
threshold(`+0x50`)와 이 counter의 차이 `<10` 조건이 있으나 counter 시간 단위와
mode/capacity 설정 경로는 이 조사에서 확정하지 않았다.

또한 앞서 확인한 RTC 연결 `0xb/0xc`는 frame callback을 직접 호출하는 분기이므로
이 공통 큐의 정책을 전체 WebRTC pacing·drop 정책으로 일반화하지 않는다.

Java live 구독에 적용할 수 있는 대응은 bounded queue 초과 시 backlog를 완전히
비우고 다음 IDR부터 재개하는 방식이다. 이는 압축 프레임의 의존성을 고려하는
원본 gate에 근거한 **Java 대응**이며, 원본의 마지막 1개 유지나 mode 2 threshold를
그대로 재현한 것은 아니다. 녹화는 영상 누락을 조용히 허용하지 않도록 기존
실패·재연결 처리를 유지하는 것이 별도의 명시적 제품 선택이다.

## 6. MSE 어댑터는 크기를 먼저 계산하고 출력 버퍼를 재사용

`0x14009abf0`, `evidence/native/server-v2/029-mse-setup.c`, Evidence
`ev_d885c7e98de82df037906478a0edccd4e0cd16108903b3e95a5179ba1a8dbe7d`는
MP4 fragment 길이 + 전용 10바이트 헤더 + WebSocket 길이별 헤더 공간을 먼저
계산한다. adapter의 현재 capacity(`param_1[2]`)가 그 크기보다 작을 때만
새 allocation과 기존 output pointer(`param_1[1]`)의 free를 수행한다.
충분하면 같은 출력 공간에 WS 헤더·전용 헤더·fragment를 쓴다.

초기화 `0x14009ba60`도 capacity를 비교해 같은 output pointer에 쓴다.
Evidence `ev_42bc933a7827d5fbd6804f8afd06ca01ef2a75c59a8d7a517d973a50cc80fa79`,
`evidence/native/server-v2/028-mse-frame.c`.
이는 adapter별 capacity 재사용의 근거이며, 서버 전체 공용 buffer pool을
확인한 것은 아니다. MP4 serializer가 반환한 buffer를 output에 복사하는 경계도
남아 있으므로 'MSE 전체 무복사'라는 해석도 맞지 않는다.

Java Netty는 비동기 전송이므로, write 후 재사용 가능한 시점을 확인하지 않고
하나의 mutable byte[]를 덮어쓰면 아직 전송되지 않은 프레임이 손상될 수 있다.
우선 적용 가능한 안전한 대응은 최종 크기를 선계산한 한 번의 allocation과
그 buffer에 직접 쓰기다. 실제 reusable pool은 write 완료에 묶인 반환과
닫힘·취소 경로를 함께 설계·검증해야 한다.

## 7. callback·스레드 관련 확인 범위

서버 `0x1400265f0`은 provider 호출 전 resource critical section을 풀고, 호출 후
다시 잠근다. 정상 frame fanout 뒤와 provider의 '아직 sample 없음' 반환 후
`PostQueuedCompletionStatus`로 resource를 다시 예약한다. 후자에는 `Sleep(2)`도
존재한다. RTC callback은 소비자별 critical section 안에서 직접 호출된다.

이 관측은 입력마다 무제한 새 OS 스레드를 생성한다는 모델과 다르지만, IOCP의
전체 워커 개수나 공정성, 하위 libwebrtc pacing thread 수를 증명하지 않는다.
Java의 source당 virtual thread와 제한된 구독 큐는 현재 별도의 구현 선택이다.
원본과 같은 thread engine이라고 부르지 않는다.

`0x1800a13a0`의 encoder factory 공유 슬롯과 critical section은 세션 설명 설정
단계의 전달 장치이다. `Sleep(5)`와 약 1001ms 제한을 가진 대기도 보인다.
이는 video frame 처리마다 전역 lock을 잡는 증거가 아니다. 프레임 hot path에
이 패턴을 복제할 이유가 없다.
Evidence `ev_a4e170f2d5414a7997645aa491109ae3f79a47a15a898334313328140c30ead4`.

## Java 변경에 연결할 범위

변경 전 `Media.Frame`은 AVCC byte[]를 공유하지만 `Rtp.packetize`가 소비자마다
`Media.nals`를 호출해 NAL별 byte[]를 만든다. 큰 NAL은 별도 FU payload byte[]로
다시 복사되고, 마지막 RTP byte[]에 한 번 더 복사된다. `Mp4.fragment`도 매번
NAL 배열을 추출해 검증한다. 이는 원본에서 확인한 입력당 NAL 인덱스 공유와
최종 출력 직전 범위 복사 구조를 적용할 수 있는 구체적인 차이다.

1. Frame 생성 시 AVCC를 한 번 검증하고 NAL offset/length를 보관한다. 여러
   구독자는 하나의 압축 byte[]와 인덱스를 읽는다. Java 배열은 변경 가능한 타입이므로
   producer가 게시 후 수정하지 않는 수명 규칙을 코드로 명시해야 한다.
2. RTP packetizer는 인덱스로 원본 byte[]를 읽고 최종 RTP packet에 직접 복사한다.
   FU-A 계획에 offset/length만 필요하며 중간 NAL/FU payload 배열은 필요 없다.
3. SSRC, sequence, SRTP key/counter, retransmission cache는 반드시 세션별로 유지한다.
   공유할 수 있는 것은 입력 프레임과 인덱스이며 암호화된 RTP packet 자체가 아니다.
4. MP4의 NAL 검증도 인덱스 기반으로 할 수 있다. MP4 box를 한 번 크기 계산 후
   쓰는 변경은 위 MSE 어댑터의 크기 선계산에서 가져오는 Java 대응이며, 원본과 같은 serializer나
   '원본이 시청자 사이 MP4 조각을 공유한다'는 증거는 아직 없다.
5. live 큐 초과 시 다음 IDR로 복구하는 정책은 위 원본 admission을 바탕으로
   선택할 수 있다. recording 실패 정책과는 구분한다. 공유 프레임을 무제한
   보관하는 cache 또는 N개 소비자를 기다리는 전역 lock을 추가하지 않는다.

검증은 NAL payload 바이트 동일성, FU-A 재조립, marker/sequence 경계, 잘못된
AVCC의 조기 거부, 다중 소비자의 서로 다른 RTP 헤더, 실제 브라우저 재생으로
나눠 수행해야 한다. CPU·RSS 변화는 동일 입력·동일 시청자 수로 측정하고,
할당 감소와 전체 서비스 성능 우위를 혼동하지 않는다.

추가 암호화 회귀 근거는 `test-results/java-optimization/crypto-golden.json`이다.
`scripts/CryptoGolden.java`를 변경 전 보존 JAR만 classpath에 넣어 실행했고,
고정 key/salt/SSRC에서 RTP sequence 65535→0→1과 SRTCP index 0→1→2의 출력을
기록했다. 원본 보존 JAR SHA-256은
`171b2a3a25cf025e79654d8c0ec1aa48577701c561e14eb85c9770ae612595e2`이다.
`SrtpGoldenTest`는 이 여섯 출력과 source buffer 불변 여부를 비교한다.
이는 Java 최적화 전후의 바이트 회귀 oracle이며, 원본 DLL의 암호화 결과나
독립 RFC 표준 벡터라고 주장하지 않는다. 실행 결과는 최종 Java 검증 기록에서 확인한다.

새 버퍼 분석의 Evidence 목록과 digest는
`evidence/native/buffer-engine/buffer-finding-ledger.json`에 있다. 재사용 가능한
provider-neutral snapshot은 같은 디렉터리의 `analysis-snapshot.json`에 저장했다.
세션에서 함께 처리한 UDP 요청은 담당자의 `evidence/native/udp-engine/`에도
복사되어 있다.
