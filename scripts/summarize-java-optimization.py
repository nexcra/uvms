#!/usr/bin/env python3
import hashlib, json, pathlib, xml.etree.ElementTree as ET

root = pathlib.Path(__file__).resolve().parents[1]
out = root/'test-results/java-optimization'
read = lambda name: json.loads((out/name).read_text())
before, after = read('fixture-before/viewers.json'), read('fixture-after/viewers.json')
assert before['passed'] and after['passed']
result = {'fixture':'H264 testsrc2 640x360 15fps, 16 viewers, 25 seconds per stage',
          'cpu_unit':'100 percent equals one logical core',
          'baseline_sha256':hashlib.sha256((out/'before/uvms-java.jar').read_bytes()).hexdigest(),
          'optimized_sha256':hashlib.sha256((root/'java/target/uvms-java-0.1.0.jar').read_bytes()).hexdigest(),
          'stages':[]}
rows=[]
for b,a in zip(before['stages'],after['stages']):
    assert b['mode']==a['mode'] and b['viewers']==a['viewers']==16
    reduction=100*(1-a['cpuMeanOneCorePercent']/b['cpuMeanOneCorePercent'])
    stage={'mode':a['mode'],'before_cpu':b['cpuMeanOneCorePercent'],'after_cpu':a['cpuMeanOneCorePercent'],
           'cpu_relative_reduction_percent':reduction,'before_rss_mib':b['rssPeakMiB'],'after_rss_mib':a['rssPeakMiB'],
           'before_mbps':b['totalMbps'],'after_mbps':a['totalMbps'],
           'before_fps':b['meanFps'],'after_fps':a['meanFps'],
           'before_browser_dropped':sum(c['dropped'] for c in b['clients']),
           'after_browser_dropped':sum(c['dropped'] for c in a['clients']),
           'after_source_inputs':a['sourceInputsMax'],'after_udp_sockets':a['udpSocketsMax'],
           'after_udp_ports':a['udpPorts']}
    if a['mode']=='webrtc':
        stage['before_packets_lost']=sum(c['packetsLost'] for c in b['clients'])
        stage['after_packets_lost']=sum(c['packetsLost'] for c in a['clients'])
    result['stages'].append(stage)
    rows.append(f"| {a['mode']} | {stage['before_cpu']:.2f}% | {stage['after_cpu']:.2f}% | {reduction:.1f}% | {stage['before_rss_mib']:.1f} / {stage['after_rss_mib']:.1f} | {stage['before_fps']:.2f} / {stage['after_fps']:.2f} |")
(out/'summary.json').write_text(json.dumps(result,indent=2))
checks={name:read(name)['passed'] for name in ['shared-port/result.json','deployed-shared-port/result.json',
        'sessions/result.json','mse-commands/result.json','recording.json','installation.json','fixture-after/relay.json']}
suites=[ET.parse(p).getroot() for p in (root/'java/target/surefire-reports').glob('TEST-*.xml')]
tests=sum(int(s.attrib['tests']) for s in suites)
failures=sum(int(s.attrib['failures'])+int(s.attrib['errors']) for s in suites)
assert tests==11 and failures==0 and all(checks.values())
(out/'validation.json').write_text(json.dumps({'passed':True,'unit_tests':tests,'unit_failures':failures,'checks':checks},indent=2))
# Correct a legacy hard-coded label in the relay reporter; executed URI was /camera.
relay=read('fixture-after/relay.json')
relay['camera']='camera'
relay['source']='local synthetic fixture'
(out/'fixture-after/relay.json').write_text(json.dumps(relay,indent=2))
section='''| 방식 | 변경 전 CPU | 변경 후 CPU | 상대 감소 | 전 / 후 RSS MiB | 전 / 후 평균 FPS |
|---|---:|---:|---:|---:|---:|
'''+ '\n'.join(rows)+'''

수신 payload 합계는 WebRTC 전/후 18.52/18.49Mbps, MSE 18.65/18.60Mbps로 비슷했다. WebRTC RTP 손실은 전후 모두 0이었다. MSE에는 전후 모두 브라우저 렌더링 누락이 관측됐으며 평균 표시 FPS는 약10이었다. 이 수치를 서버 RTP 손실로 해석하지 않는다.

메모리는 WebRTC에서 약4.1MiB 늘고 MSE에서 약5.8MiB 줄었다. 전체 메모리가 일관되게 감소했다고 볼 수 없다. RSS에는 커널 소켓 버퍼가 포함되지 않는다. 프로세스 GC/JIT·배경 부하를 완전히 통제한 장기 통계가 아닌 단기 관측이다.

검증: 단위 테스트11개, 고정 암호 벡터6개(RTP rollover와 SRTCP), 16브라우저/2별칭 공용 포트, 8명 종료 후 나머지 재생과 8명 재접속, pre-offer 종료80회, MSE pause/continue/switch, RTSP16명 각100프레임, MP4 2초30프레임 녹화·다운로드 일치·종료 후 partial0개를 통과했다. 배포된 8088에서도 16뷰어 공유 포트 시험을 통과했다.

Linux 서비스 배포 JAR SHA-256: `04ed9860cb434f37c46480eb15ace8e7c9c27e883775334f38e96e622cb4deef`, 크기13,140,354bytes(13.14MB). 운영 공용 포트50200에 고정 소켓16개가 확인됐고, 테스트 인스턴스는 종료했다. 카메라 설정·환경변수 파일 해시는 배포 전후 동일하다. 원본 Rust 서비스는 계속 active 상태다.
'''
doc=root/'docs/java-optimization.md'
text=doc.read_text(encoding='utf-8')
text=text.replace('최종 측정 수치와 검증 결과는 작업 완료 시 이 절에 기록한다.',section)
doc.write_text(text,encoding='utf-8')
print(json.dumps(result,indent=2))
