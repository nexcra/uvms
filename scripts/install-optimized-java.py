#!/usr/bin/env python3
"""Deploy verified Java artifacts, preserve camera configuration, record identity."""
import hashlib, json, pathlib, subprocess, time, urllib.request

root = pathlib.Path(__file__).resolve().parents[1]
config = pathlib.Path('/etc/uvms-java/config.json')
env = pathlib.Path('/etc/uvms-java/uvms.env')
before = [hashlib.sha256(p.read_bytes()).hexdigest() for p in [config, env]]
subprocess.run(['bash','install-linux.sh'],cwd=root/'java',check=True)
assert before == [hashlib.sha256(p.read_bytes()).hexdigest() for p in [config, env]]
subprocess.run(['systemctl','restart','uvms-java'],check=True)
for attempt in range(40):
    try:
        with urllib.request.urlopen('http://127.0.0.1:8088/healthz',timeout=1) as response:
            assert response.status==200
        break
    except OSError:
        if attempt==39: raise
        time.sleep(.25)
jar = pathlib.Path('/opt/uvms-java/uvms-java.jar')
digest = hashlib.sha256(jar.read_bytes()).hexdigest()
assert digest == hashlib.sha256((root/'java/target/uvms-java-0.1.0.jar').read_bytes()).hexdigest()
pid = subprocess.check_output(['systemctl','show','-p','MainPID','--value','uvms-java'],text=True).strip()
sockets=set()
for fd in pathlib.Path(f'/proc/{pid}/fd').iterdir():
    try:
        target=fd.readlink().as_posix()
        if target.startswith('socket:['): sockets.add(target[8:-1])
    except OSError: pass
ports=[]
for table in ['udp','udp6']:
    for line in pathlib.Path(f'/proc/{pid}/net/{table}').read_text().splitlines()[1:]:
        fields=line.split()
        if fields[9] in sockets: ports.append(int(fields[1].split(':')[1],16))
result = {'passed':True,'jar_sha256':digest,'jar_bytes':jar.stat().st_size,
          'camera_configuration_preserved':True, 'http':8088,'rtsp':8558,
          'udp_socket_count':len(ports),'udp_ports':sorted(set(ports)),
          'service_active':subprocess.check_output(['systemctl','is-active','uvms-java'],text=True).strip(),
          'service_enabled':subprocess.check_output(['systemctl','is-enabled','uvms-java'],text=True).strip(),
          'rust_service_active':subprocess.check_output(['systemctl','is-active','uvms'],text=True).strip()}
assert result['udp_socket_count']==16 and result['udp_ports']==[50200]
(root/'test-results/java-optimization/installation.json').write_text(json.dumps(result,indent=2))
print(json.dumps(result))
