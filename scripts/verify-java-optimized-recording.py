#!/usr/bin/env python3
"""Record the synthetic fixture with the isolated Java benchmark service."""
import hashlib, json, pathlib, subprocess, time, urllib.request

root = pathlib.Path(__file__).resolve().parents[1]
path = pathlib.Path('/etc/uvms-java-opt-bench.json')
original = path.read_text()
config = json.loads(original)
assert config['recording_dir'] == '/var/lib/uvms-java-opt-bench/recordings'
directory = pathlib.Path(config['recording_dir'])
existing = set(directory.rglob('*'))
result = {'passed': False, 'source': 'local synthetic H264, 640x360 15fps'}
try:
    config['streams']['camera']['record'] = True
    config['segment_seconds'] = 2
    path.write_text(json.dumps(config))
    subprocess.run(['systemctl','restart','uvms-java-opt-bench'],check=True)
    selected = None
    for _ in range(60):
        candidates = [p for p in directory.rglob('*.mp4') if p not in existing]
        if candidates:
            selected = candidates[0]
            break
        time.sleep(.25)
    assert selected is not None, 'No completed MP4 segment'
    result['probe'] = json.loads(subprocess.check_output([
        'ffprobe','-v','error','-count_frames','-show_entries',
        'stream=codec_name,width,height,duration,nb_read_frames','-of','json',str(selected)
    ],text=True))
    with urllib.request.urlopen('http://127.0.0.1:8090/api/recordings/camera/'+selected.name) as response:
        result['download_status'] = response.status
        result['download_matches'] = hashlib.sha256(response.read()).digest() == hashlib.sha256(selected.read_bytes()).digest()
    pid = subprocess.check_output(['systemctl','show','-p','MainPID','--value','uvms-java-opt-bench'],text=True).strip()
    result['child_processes'] = pathlib.Path(f'/proc/{pid}/task/{pid}/children').read_text().strip()
    stream = result['probe']['streams'][0]
    result['passed'] = (stream['codec_name']=='h264' and stream['width']==640
        and int(stream['nb_read_frames'])>=20 and result['download_matches']
        and result['download_status']==200 and not result['child_processes'])
finally:
    subprocess.run(['systemctl','stop','uvms-java-opt-bench'],check=True)
    path.write_text(original)
    result['remaining_partial_files'] = len(list(directory.rglob('*.partial')))
    result['passed'] = result['passed'] and result['remaining_partial_files']==0
    # Only files created by this isolated verification are removed.
    for p in directory.rglob('*'):
        if p.is_file() and p not in existing:
            assert p.resolve().is_relative_to(directory.resolve())
            p.unlink()
    output = root/'test-results/java-optimization/recording.json'
    output.write_text(json.dumps(result,indent=2))
    print(json.dumps(result))
if not result['passed']: raise SystemExit(1)
