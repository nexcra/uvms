#!/usr/bin/env python3
"""Local ONVIF camera emulator + Rust integration tests. Never moves real hardware."""
import argparse
import base64
import concurrent.futures
import datetime
import hashlib
import html
import http.server
import json
import os
import pathlib
import re
import signal
import socket
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request
import xml.etree.ElementTree as ET

ROOT=pathlib.Path(__file__).resolve().parents[1]
BASE='http://127.0.0.1:8097'
SOAP='http://www.w3.org/2003/05/soap-envelope'
DEVICE='http://www.onvif.org/ver10/device/wsdl'
MEDIA='http://www.onvif.org/ver10/media/wsdl'
MEDIA2='http://www.onvif.org/ver20/media/wsdl'
PTZ='http://www.onvif.org/ver20/ptz/wsdl'
TT='http://www.onvif.org/ver10/schema'
WSSE='http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd'
WSU='http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd'
ADDRESS='http://schemas.xmlsoap.org/ws/2004/08/addressing'
DISCOVERY='http://schemas.xmlsoap.org/ws/2005/04/discovery'
USER='fixture<&user'
PASSWORD='synthetic-onvif-password'
events=[]
mode={'move':'normal'}
lock=threading.Lock()


def node_text(root,namespace,name):
    node=root.find('.//{'+namespace+'}'+name)
    return node.text if node is not None else None


class Camera(http.server.BaseHTTPRequestHandler):
    def log_message(self,*args):pass
    def reply(self,status,body,headers=None):
        data=body.encode()
        self.send_response(status)
        self.send_header('Content-Type','application/soap+xml' if body.startswith('<') else 'application/json')
        self.send_header('Content-Length',str(len(data)))
        for k,v in (headers or {}).items():self.send_header(k,v)
        self.end_headers()
        try:self.wfile.write(data)
        except (BrokenPipeError,ConnectionResetError):pass
    def do_GET(self):
        with lock:self.reply(200,json.dumps({'events':events,'mode':mode}))
    def do_POST(self):
        data=self.rfile.read(int(self.headers.get('Content-Length','0')))
        if self.path=='/fixture/mode':
            with lock:mode.update(json.loads(data))
            return self.reply(200,'{}')
        root=ET.fromstring(data)
        operation=list(root.find('{'+SOAP+'}Body'))[0]
        ns,name=operation.tag[1:].split('}')
        if name!='GetSystemDateAndTime':
            # Real SHA-256 HTTP Digest challenge plus WS-Security PasswordDigest.
            auth=self.headers.get('Authorization','')
            if not auth:
                return self.reply(401,'',{'WWW-Authenticate':'Digest realm="fixture", nonce="fixture-nonce", algorithm=SHA-256, qop="auth"'})
            fields={k:quoted or plain for k,quoted,plain in re.findall(r'(\w+)=(?:"([^"]*)"|([^, ]+))',auth)}
            digest=lambda s:hashlib.sha256(s.encode()).hexdigest()
            expected=digest(f'{digest(USER+":fixture:"+PASSWORD)}:fixture-nonce:{fields.get("nc")}:{fields.get("cnonce")}:auth:{digest("POST:"+self.path)}')
            try:
                nonce=base64.b64decode(node_text(root,WSSE,'Nonce'))
                created=node_text(root,WSU,'Created')
                password_digest=base64.b64encode(hashlib.sha1(nonce+created.encode()+PASSWORD.encode()).digest()).decode()
                timestamp=datetime.datetime.fromisoformat(created.replace('Z','+00:00')).timestamp()
                valid=(fields.get('response')==expected and node_text(root,WSSE,'Username')==USER and node_text(root,WSSE,'Password')==password_digest and abs(timestamp-(time.time()+90))<5)
            except (TypeError,ValueError):valid=False
            if not valid:return self.reply(403,'')
        with lock:events.append({'operation':name,'path':self.path,'time':time.monotonic(),'profile':node_text(operation,PTZ,'ProfileToken'),'timeout':node_text(operation,PTZ,'Timeout')})
        endpoint='http://127.0.0.1:18690'+self.path
        if name=='GetSystemDateAndTime':
            now=datetime.datetime.fromtimestamp(time.time()+90,datetime.timezone.utc)
            payload=f'<t:SystemDateAndTime><tt:UTCDateTime><tt:Time><tt:Hour>{now.hour}</tt:Hour><tt:Minute>{now.minute}</tt:Minute><tt:Second>{now.second}</tt:Second></tt:Time><tt:Date><tt:Year>{now.year}</tt:Year><tt:Month>{now.month}</tt:Month><tt:Day>{now.day}</tt:Day></tt:Date></tt:UTCDateTime></t:SystemDateAndTime>'
        elif name=='GetServices':
            if self.path=='/legacy':return self.reply(500,'')
            media_ns=MEDIA2 if self.path=='/media2' else MEDIA
            url='http://example.invalid/never-send-credentials' if self.path=='/hostile' else endpoint
            payload=''.join(f'<t:Service><t:Namespace>{namespace}</t:Namespace><t:XAddr>{html.escape(url)}</t:XAddr></t:Service>' for namespace in [media_ns,PTZ])
        elif name=='GetCapabilities':
            payload=f'<t:Capabilities><tt:Media><tt:XAddr>{endpoint}</tt:XAddr></tt:Media><tt:PTZ><tt:XAddr>{endpoint}</tt:XAddr></tt:PTZ></t:Capabilities>'
        elif name=='GetProfiles':
            config='<t:Configurations><t:PTZ token="ptz1"/></t:Configurations>' if ns==MEDIA2 else '<tt:PTZConfiguration token="ptz1"/>'
            prefix='t' if ns==MEDIA2 else 'tt'
            payload=f'<t:Profiles token="main"><{prefix}:Name>Mock &lt;camera&gt;</{prefix}:Name>{config}</t:Profiles><t:Profiles token="fixed"><{prefix}:Name>Fixed profile</{prefix}:Name></t:Profiles>'
        elif name=='GetDeviceInformation':payload='<t:Manufacturer>UVMS test</t:Manufacturer><t:Model>SOAP emulator</t:Model><t:FirmwareVersion>1</t:FirmwareVersion>'
        elif name=='GetConfigurationOptions':
            payload='<t:PTZConfigurationOptions><tt:Spaces><tt:ContinuousPanTiltVelocitySpace><tt:URI>http://www.onvif.org/ver10/tptz/PanTiltSpaces/VelocityGenericSpace</tt:URI><tt:XRange><tt:Min>-1</tt:Min><tt:Max>1</tt:Max></tt:XRange><tt:YRange><tt:Min>-1</tt:Min><tt:Max>1</tt:Max></tt:YRange></tt:ContinuousPanTiltVelocitySpace><tt:ContinuousZoomVelocitySpace><tt:URI>http://www.onvif.org/ver10/tptz/ZoomSpaces/VelocityGenericSpace</tt:URI><tt:XRange><tt:Min>-1</tt:Min><tt:Max>1</tt:Max></tt:XRange></tt:ContinuousZoomVelocitySpace></tt:Spaces><tt:PTZTimeout><tt:Min>PT0S</tt:Min><tt:Max>PT1M</tt:Max></tt:PTZTimeout></t:PTZConfigurationOptions>'
        elif name=='GetStatus':payload='<t:PTZStatus><tt:MoveStatus><tt:PanTilt>IDLE</tt:PanTilt><tt:Zoom>IDLE</tt:Zoom></tt:MoveStatus></t:PTZStatus>'
        elif name=='ContinuousMove':
            if mode['move']=='fault':return self.reply(500,'')
            if mode['move']=='timeout':time.sleep(6)
            payload=''
        elif name=='Stop':payload=''
        else:return self.reply(500,'')
        self.reply(200,f'<s:Envelope xmlns:s="{SOAP}" xmlns:tt="{TT}"><s:Body><t:{name}Response xmlns:t="{ns}">{payload}</t:{name}Response></s:Body></s:Envelope>')


def discover(udp,done):
    udp.settimeout(.5)
    while not done.is_set():
        try:data,peer=udp.recvfrom(65536)
        except socket.timeout:continue
        except OSError:break
        try:
            root=ET.fromstring(data);message=node_text(root,ADDRESS,'MessageID')
            response=f'<s:Envelope xmlns:s="{SOAP}" xmlns:a="{ADDRESS}" xmlns:d="{DISCOVERY}"><s:Header><a:RelatesTo>{message}</a:RelatesTo></s:Header><s:Body><d:ProbeMatches><d:ProbeMatch><a:EndpointReference><a:Address>urn:uuid:uvms-test-camera</a:Address></a:EndpointReference><d:Scopes>onvif://www.onvif.org/name/Local%20PTZ%20fixture</d:Scopes><d:XAddrs>http://127.0.0.1:18690/device?channel=1</d:XAddrs></d:ProbeMatch></d:ProbeMatches></s:Body></s:Envelope>'
            udp.sendto(response.encode(),peer)
        except (ET.ParseError,OSError):pass


def api(path,data=None,headers=None):
    request=urllib.request.Request(BASE+path,data=None if data is None else json.dumps(data).encode(),headers={'Content-Type':'application/json',**(headers or {})})
    try:response=urllib.request.urlopen(request,timeout=35)
    except urllib.error.HTTPError as error:response=error
    with response:
        body=response.read()
        try:value=json.loads(body)
        except ValueError:value=body.decode()
        return response.status,value


def run_tests():
    code,found=api('/api/onvif/discover',{})
    assert code==200 and len(found['devices'])==1,found
    assert found['devices'][0]['matched_streams']
    for alias in ['fixture','alias','legacy','media2']:
        code,info=api('/api/onvif/'+alias,{})
        assert code==200 and len(info['profiles'])==2,info
        assert 88000<info['clock_offset_ms']<91000,info
        code,detail=api('/api/onvif/'+alias+'/profile',{'profile_token':'main'})
        assert code==200 and detail['supported'],detail
    assert api('/api/onvif/fixture/profile',{'profile_token':'fixed'})[1]['supported'] is False
    before=len(events)
    assert api('/api/onvif/hostile',{})[0]==502
    assert not any('example.invalid' in e['path'] for e in events[before:])
    move={'action':'move','profile_token':'main','pan':.5,'tilt':0,'zoom':0,'duration_ms':300}
    before=len(events)
    assert api('/api/onvif/fixture/ptz',move)[0]==200
    actions=[e['operation'] for e in events[before:]]
    assert actions==['ContinuousMove','Stop'],actions
    assert events[before]['timeout']=='PT0.300S'
    for changes in [{'pan':2},{'duration_ms':5000},{'profile_token':'fixed'},{'profile_token':'unknown'}]:
        before=len(events);assert api('/api/onvif/fixture/ptz',{**move,**changes})[0]>=400;assert len(events)==before
    assert api('/api/onvif/fixture/ptz',move,{'Origin':'http://unrelated.invalid'})[0]==403
    with concurrent.futures.ThreadPoolExecutor() as pool:
        pending=pool.submit(api,'/api/onvif/fixture/ptz',{**move,'duration_ms':700})
        time.sleep(.1)
        assert api('/api/onvif/alias/ptz',move)[0]>=400
        assert pending.result()[0]==200
    for fault in ['fault','timeout']:
        mode['move']=fault;before=len(events)
        assert api('/api/onvif/fixture/ptz',move)[0]==502
        assert [e['operation'] for e in events[before:]]==['ContinuousMove','Stop']
    mode['move']='normal'
    # Closing the caller must not cancel the delayed Stop.
    payload=json.dumps({**move,'duration_ms':700}).encode()
    before=len(events)
    client=socket.create_connection(('127.0.0.1',8097))
    client.sendall(b'POST /api/onvif/fixture/ptz HTTP/1.1\r\nHost: 127.0.0.1:8097\r\nContent-Type: application/json\r\nContent-Length: '+str(len(payload)).encode()+b'\r\n\r\n'+payload)
    deadline=time.monotonic()+2
    while len(events)==before and time.monotonic()<deadline:time.sleep(.02)
    assert len(events)>before
    client.close();time.sleep(1)
    assert [e['operation'] for e in events[before:]]==['ContinuousMove','Stop']
    assert api('/api/onvif/fixture/ptz',{'action':'stop','profile_token':'main'})[0]==200
    return {'passed':True,'discovery':found,'verified':['WS-Discovery correlation and deduplication','WS-Security SHA1 nonce digest with XML escaping and camera clock correction','HTTP SHA-256 Digest with query string','Media1, Media2, GetCapabilities fallback','fixed profile rejection and velocity/timeout validation','ContinuousMove + Stop','same-device alias serialization','Stop after HTTP failure, timeout and caller disconnect','same-origin rejection','cross-host XAddr rejection'],'soap_requests':len(events)}


def main():
    parser=argparse.ArgumentParser();parser.add_argument('--serve',action='store_true');args=parser.parse_args()
    work=pathlib.Path(tempfile.mkdtemp(prefix='uvms-onvif-'))
    server=http.server.ThreadingHTTPServer(('127.0.0.1',18690),Camera)
    thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
    udp=socket.socket(socket.AF_INET,socket.SOCK_DGRAM);udp.bind(('127.0.0.1',3702));done=threading.Event()
    threading.Thread(target=discover,args=(udp,done),daemon=True).start()
    config=f'bind="127.0.0.1:8097"\nrtsp_bind="127.0.0.1:18578"\nice_udp_min=51920\nice_udp_max=51920\nweb_root="{ROOT}/web"\nrecording_dir="{work}/recordings"\n'
    for alias in ['fixture','alias','legacy','media2','hostile']:
        endpoint='device?channel=1' if alias in ['fixture','alias'] else alias
        config+=f'[streams.{alias}]\nurl_env="ONVIF_RTSP"\nrecord=false\nonvif_url="http://127.0.0.1:18690/{endpoint}"\nonvif_username_env="ONVIF_USER"\nonvif_password_env="ONVIF_PASSWORD"\n'
    (work/'config.toml').write_text(config)
    env={**os.environ,'UVMS_CONFIG':str(work/'config.toml'),'ONVIF_RTSP':'rtsp://127.0.0.1:18579/camera','ONVIF_USER':USER,'ONVIF_PASSWORD':PASSWORD}
    log=(work/'server.log').open('wb')
    process=subprocess.Popen(['/root/uvms-target/release/uvms-gateway'],env=env,stdout=log,stderr=log)
    report={'passed':False,'work':str(work)}
    try:
        for _ in range(100):
            try:
                if api('/healthz')[0]==200:break
            except OSError:pass
            time.sleep(.1)
        else:raise RuntimeError('ONVIF fixture failed to start')
        if args.serve:
            print(json.dumps({'ready':True,'url':BASE,'work':str(work)}),flush=True)
            signal.signal(signal.SIGTERM,lambda *_:done.set())
            while not done.wait(.5):pass
        else:
            report.update(run_tests());print(json.dumps(report),flush=True)
    finally:
        process.terminate()
        try:process.wait(timeout=15)
        except subprocess.TimeoutExpired:process.kill();process.wait()
        done.set();server.shutdown();udp.close();log.close()
        if not args.serve:
            output=ROOT/'test-results/onvif/backend.json';output.parent.mkdir(parents=True,exist_ok=True)
            report['events']=events;output.write_text(json.dumps(report,indent=2))


if __name__=='__main__':main()
