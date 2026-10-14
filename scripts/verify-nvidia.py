"""Driver/HEVC integration gate. Never report a CPU-only run as GPU validation."""
import argparse, http.client, json, pathlib, subprocess, urllib.parse, urllib.request
parser=argparse.ArgumentParser()
parser.add_argument('--url',default='http://127.0.0.1:8093')
parser.add_argument('--expect-no-gpu',action='store_true')
parser.add_argument('--stream',default='h265')
parser.add_argument('--output',default='test-results/nvidia/integration.json')
args=parser.parse_args()
base=args.url.rstrip('/')
def api(path):
    with urllib.request.urlopen(base+path,timeout=35) as r:return json.load(r)
report={'hardware_transcode_tested':False,'gpu':api('/api/gpu'),'passed':False}
try:
    assert report['gpu']['compiled'], 'Build --features nvidia first'
    if not report['gpu']['available']:
        assert args.expect_no_gpu, 'NVIDIA hardware unavailable: GPU conversion remains UNVERIFIED'
        u=urllib.parse.urlsplit(base)
        assert u.hostname in ('localhost','127.0.0.1') and u.port==8093,'negative test uses the isolated fixture only'
        connection=http.client.HTTPConnection(u.hostname,u.port,timeout=35)
        connection.request('GET','/webrtc_playnow/singleport/udp/h265',headers={
            'Connection':'Upgrade','Upgrade':'websocket','Sec-WebSocket-Version':'13',
            'Sec-WebSocket-Key':'dGhlIHNhbXBsZSBub25jZQ=='})
        response=connection.getresponse();response.read();report['hevc_request_status']=response.status
        assert response.status==502,'Missing hardware must fail explicitly'
        connection.close()
        video=next(s['video'] for s in api('/api/streams')['streams'] if s['id']=='h265')
        assert video['state']=='unavailable' and 'NVIDIA' in video['error']
        report['hevc_failure']=video
        report['passed']=True
        report['scope']='GPU-less negative path only; native HEVC depacketization validated separately'
    else:
        assert not args.expect_no_gpu,'Unexpected GPU; run positive hardware validation'
        streams=api('/api/streams')
        rtsp=f"rtsp://{urllib.parse.urlsplit(base).hostname}:{streams['rtsp_port']}/{urllib.parse.quote(args.stream,safe='')}"
        probe=subprocess.run(['ffprobe','-v','error','-rtsp_transport','tcp','-read_intervals','%+10','-count_frames','-show_entries','stream=codec_name,width,height,nb_read_frames','-of','json',rtsp],capture_output=True,text=True,timeout=40,check=True)
        media=json.loads(probe.stdout)
        video=next(s for s in media['streams'] if s['codec_name']=='h264')
        assert int(video['nb_read_frames'])>=15
        state=next(s['video'] for s in api('/api/streams')['streams'] if s['id']==args.stream)
        assert state['input']=='H265' and state['processing']=='nvidia_nvdec_nvenc' and state['output']=='H264'
        report.update(passed=True,hardware_transcode_tested=True,media=media,processing=state,
                      scope='Actual NVDEC/NVENC RTSP output; run browser-smoke.mjs for each browser transport')
finally:
    output=pathlib.Path(args.output);output.parent.mkdir(parents=True,exist_ok=True)
    output.write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report))
