#!/usr/bin/env python3
"""Read-only validation of every completed synthetic soak MP4 and its index."""
import argparse
import concurrent.futures
import hashlib
import json
import pathlib
import sqlite3
import subprocess
import urllib.request


def probe(item):
    path, clip = item
    result = {'stream': clip['stream'], 'file': path.name, 'bytes': path.stat().st_size, 'failures': []}
    command = ['ffprobe', '-v', 'error', '-count_frames', '-show_packets', '-show_streams',
               '-show_entries', 'packet=stream_index,dts_time,duration_time,flags:stream=index,codec_type,codec_name,nb_read_frames,start_time,duration',
               '-of', 'json', str(path)]
    try:
        process = subprocess.run(command, capture_output=True, text=True, timeout=40)
    except subprocess.TimeoutExpired:
        result['failures'].append('Decoder inspection exceeded 40 seconds')
        result['tracks'] = []
        return result
    if process.returncode or process.stderr.strip():
        result['failures'].append({'decode': process.stderr[-2000:], 'exit': process.returncode})
    try:
        data = json.loads(process.stdout or '{}')
    except json.JSONDecodeError:
        result['failures'].append('Invalid decoder inspection output')
        data = {}
    streams = data.get('streams', [])
    result['tracks'] = []
    if sorted(s.get('codec_type') for s in streams) != ['audio', 'video']:
        result['failures'].append('Missing audio/video track')
    for stream in streams:
        packets = [p for p in data.get('packets', []) if p['stream_index'] == stream['index']]
        if any('dts_time' not in p for p in packets):
            result['failures'].append('Missing packet decode timestamp')
        timed_packets = [p for p in packets if 'dts_time' in p]
        times = [float(p['dts_time']) for p in timed_packets]
        gaps = [times[i] - times[i-1] - float(timed_packets[i-1].get('duration_time', 0)) for i in range(1, len(times))]
        track = {**stream, 'packet_count': len(packets), 'max_gap_ms': max([0, *gaps])*1000,
                 'max_overlap_ms': max([0, *[-g for g in gaps]])*1000,
                 'dts_monotonic': all(a <= b for a, b in zip(times, times[1:]))}
        result['tracks'].append(track)
        if not times or not track['dts_monotonic'] or int(stream.get('nb_read_frames', 0)) <= 0:
            result['failures'].append('Non-decodable or non-monotonic ' + stream.get('codec_type', '?'))
        if track['max_gap_ms'] > 250:
            result['failures'].append('Packet timeline gap exceeds 250ms: ' + stream.get('codec_type', '?'))
        if stream.get('codec_type') == 'video' and (not packets or 'K' not in packets[0].get('flags', '')):
            result['failures'].append('Recording does not start at a keyframe')
    if result['bytes'] != clip['bytes'] or clip['estimated'] or clip['end_ms'] <= clip['start_ms']:
        result['failures'].append('Incorrect recording metadata')
    result['sha256'] = hashlib.sha256(path.read_bytes()).hexdigest()
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('result', help='soak-surveillance.mjs result.json')
    parser.add_argument('--offline', action='store_true', help='Validate saved files/index only, without accessing the current fixture')
    args = parser.parse_args()
    result_path = pathlib.Path(args.result).resolve(strict=True)
    soak = json.loads(result_path.read_text(encoding='utf-8-sig'))
    root = pathlib.Path(soak['fixture']['recording_dir']).resolve(strict=True)
    if root.parent != pathlib.Path('/var/lib/uvms-soak'):
        raise ValueError('Only isolated soak recordings may be checked')
    database = sqlite3.connect(f'file:{root}/archive.sqlite?mode=ro', uri=True)
    database.row_factory = sqlite3.Row
    integrity = database.execute('PRAGMA integrity_check').fetchone()[0]
    clips = [dict(row) for row in database.execute('SELECT * FROM clips ORDER BY stream,start_ms')]
    database.close()
    files = {str(p.relative_to(root)): p for p in root.glob('*/*.mp4')}
    indexed = {c['stream'] + '/' + c['file'] for c in clips}
    failures = []
    if integrity != 'ok':
        failures.append('SQLite integrity check failed')
    if set(files) != indexed:
        failures.append('MP4 files and index disagree')
    partials = list(root.glob('*/*.partial'))
    if partials:
        failures.append('Unfinished partial recordings remain')
    outages = []
    for event in soak['events']:
        if 'affected' not in event:
            continue
        restored = next((e['at'] for e in soak['events'] if e['kind']=='restore' and
                         e.get('camera')==event.get('camera') and e['at']>event['started']), event['actionFinished'])
        outages.append((event['affected'], event['started'], restored+75000))
    gaps = []
    for alias in soak['fixture']['aliases']:
        ordered = [c for c in clips if c['stream']==alias]
        for previous, current in zip(ordered, ordered[1:]):
            gap = current['start_ms']-previous['end_ms']
            if gap <= 2000:
                continue
            expected = any(alias in affected and began<=current['start_ms'] and ended>=previous['end_ms']
                           for affected, began, ended in outages)
            gaps.append({'stream':alias,'from_ms':previous['end_ms'],'to_ms':current['start_ms'],'gap_ms':gap,'injected_outage':expected})
            if not expected:
                failures.append({'unexpected_recording_gap':gaps[-1]})
    verified = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
        for result in pool.map(probe, [(files[c['stream']+'/'+c['file']], c) for c in clips if c['stream']+'/'+c['file'] in files]):
            verified.append(result)
            if result['failures']:
                failures.append({'file': result['file'], 'errors': result['failures']})
            if len(verified) % 50 == 0:
                print(json.dumps({'verified': len(verified), 'total': len(clips), 'failures': len(failures)}), flush=True)
    # Verify search pagination and Range download only against this exact fixture run.
    downloads = []
    if not args.offline:
        current = json.loads(pathlib.Path('/etc/uvms-soak/run.json').read_text())
        if current['recording_dir'] != str(root):
            failures.append('Live fixture belongs to another run; use --offline for historical files')
        else:
            live_check(clips, files, indexed, soak['fixture']['aliases'], downloads, failures)
    report = {'passed': not failures, 'integrity': integrity, 'files': len(files), 'bytes': sum(p.stat().st_size for p in files.values()),
              'partial_files': len(partials), 'live_api_checked': bool(downloads), 'recording_gaps_over_2s':gaps,
              'failures': failures, 'downloads': downloads, 'recordings': verified}
    (result_path.parent/'recordings.json').write_text(json.dumps(report, indent=2))
    print(json.dumps({k: v for k, v in report.items() if k not in ['recordings','downloads']}))
    return 0 if report['passed'] else 1


def live_check(clips, files, indexed, aliases, downloads, failures):
    searched = []
    for offset in range(0, len(clips)+1, 1000):
        with urllib.request.urlopen(f'http://127.0.0.1:8099/api/recordings?limit=1000&offset={offset}', timeout=20) as response:
            listing = json.load(response)
        searched.extend(listing['files'])
        if len(listing['files']) < 1000:
            break
    if {c['stream']+'/'+c['file'] for c in searched} != indexed:
        failures.append('Search API does not match completed files')
    for alias in aliases:
        camera = [c for c in clips if c['stream'] == alias]
        if not camera:
            failures.append('No recordings for '+alias)
            continue
        for clip in [camera[0], camera[-1]]:
            name = clip['stream']+'/'+clip['file']
            request = urllib.request.Request('http://127.0.0.1:8099/api/recordings/'+name, headers={'Range': 'bytes=0-127'})
            with urllib.request.urlopen(request, timeout=10) as response:
                body = response.read()
                okay = response.status == 206 and body == files[name].read_bytes()[:128]
            downloads.append({'file': name, 'range_matches': okay})
            if not okay:
                failures.append('Range download mismatch: '+name)


if __name__ == '__main__':
    raise SystemExit(main())
