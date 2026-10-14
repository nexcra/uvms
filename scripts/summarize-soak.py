#!/usr/bin/env python3
"""Compact, reproducible summary; retains failures and distinguishes injected outages."""
import argparse
import collections
import json
import pathlib
import statistics

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('directory')
args = parser.parse_args()
root = pathlib.Path(args.directory)
report = json.loads((root/'result.json').read_text(encoding='utf-8-sig'))
recordings = json.loads((root/'recordings.json').read_text()) if (root/'recordings.json').exists() else None
start = report['samples'][0]['at']
events = []
for event in report['events']:
    if 'affected' not in event:
        continue
    restored = next((e['at'] for e in report['events'] if e['kind']=='restore' and e['camera']==event.get('camera') and e['at']>event['started']), event['actionFinished'])
    entry = {'kind':event['kind'], 'camera':event.get('camera'), 'start_seconds':(event['started']-start)/1000,
             'restored_seconds':(restored-start)/1000, 'recovery_seconds':{}, 'recording_recovery_seconds':{}}
    after = [s for s in report['samples'] if s['at']>=restored]
    for camera in event['affected']:
        for mode in ['webrtc','mse','hls']:
            key = mode+'/'+camera
            consecutive = 0
            recovered = None
            for sample in after:
                player = next((p for p in sample['players'] if p['id']==camera and p['mode']==mode), {})
                okay = player.get('state')=='live' and player.get('fps',0)>=5 and player.get('rms',0)>0.0001
                consecutive = consecutive+1 if okay else 0
                if consecutive>=2:
                    recovered = (sample['at']-restored)/1000
                    break
            entry['recovery_seconds'][key]=recovered
        entry['recording_recovery_seconds'][camera]=next(((s['at']-restored)/1000 for s in after
            if s.get('clips',{}).get(camera) and s['clips'][camera]['start_ms']>=restored),None)
    events.append(entry)

by_player = collections.defaultdict(list)
for sample in report['samples']:
    for player in sample['players']:
        if not player['expectedOutage'] and 'fps' in player:
            by_player[player['mode']+'/'+player['id']].append(player)
playback = {key:{'samples':len(rows),'mean_fps':statistics.mean(r['fps'] for r in rows),
                 'low_fps_samples':sum(r['fps']<5 for r in rows),
                 'silent_samples':sum(r['rms']<0.0001 for r in rows),
                 'audio_windows':sum(r.get('audioWindows',0) for r in rows),
                 'audible_windows':sum(r.get('audibleWindows',0) for r in rows),
                 'generations':sorted(set(r['generation'] for r in rows))} for key,rows in by_player.items()}

metrics = [m for m in report['metrics'] if 'rss_mib' in m]
quiet = [m for m in metrics if m['time']*1000>=start+60000 and m['time']*1000<=report['samples'][-1]['at'] and
         not any(e['start_seconds']<=(m['time']*1000-start)/1000<=e['restored_seconds']+75 for e in events)]
resources = {'peak_rss_mib':max(m['rss_mib'] for m in metrics),'final':report['finalMetrics']}
if quiet:
    last_pid = quiet[-1]['pid']
    same_process = [m for m in quiet if m['pid']==last_pid]
    first = [m for m in same_process if m['time']<=same_process[0]['time']+300]
    last = [m for m in same_process if m['time']>=same_process[-1]['time']-300]
    penultimate = [m for m in same_process if same_process[-1]['time']-600<=m['time']<same_process[-1]['time']-300]
    resources.update(quiet_mean_cpu_one_core_percent=statistics.mean(m.get('cpu_percent_one_core',0) for m in quiet),
                     quiet_input_connections=sorted(set(m['source_inputs'] for m in quiet)),
                     first_quiet_5min_mean_rss_mib=statistics.mean(m['rss_mib'] for m in first),
                     last_quiet_5min_mean_rss_mib=statistics.mean(m['rss_mib'] for m in last),
                     first_quiet_fd_range=[min(m['fd_count'] for m in first),max(m['fd_count'] for m in first)],
                     last_quiet_fd_range=[min(m['fd_count'] for m in last),max(m['fd_count'] for m in last)])
    if penultimate:
        resources['previous_quiet_5min_mean_rss_mib']=statistics.mean(m['rss_mib'] for m in penultimate)
        resources['last_two_quiet_5min_rss_delta_mib']=resources['last_quiet_5min_mean_rss_mib']-resources['previous_quiet_5min_mean_rss_mib']

archive = None
if recordings:
    archive = {k:recordings[k] for k in ['passed','files','bytes','partial_files','integrity','live_api_checked','recording_gaps_over_2s','failures']}
    archive['per_camera'] = {a:sum(r['stream']==a for r in recordings['recordings']) for a in report['fixture']['aliases']}
    archive['max_packet_gap_ms'] = {kind:max(t['max_gap_ms'] for r in recordings['recordings'] for t in r['tracks'] if t['codec_type']==kind) for kind in ['video','audio']}
unexpected_media_errors=[]
for group in report['mediaErrors']:
    for error in group['errors']:
        elapsed=(error['at']-start)/1000
        if not any(e['start_seconds']<=elapsed<=e['restored_seconds']+75 and
                   (e['camera'] is None or e['camera']==error.get('id')) for e in events):
            unexpected_media_errors.append({'mode':group['mode'],**error})
summary = {'passed':report['passed'] and bool(recordings and recordings['passed']) and not unexpected_media_errors,
           'started_at':report['startedAt'],'finished_at':report.get('finishedAt'),
           'measured_seconds':report.get('measuredSeconds'),'fixture':report['fixture'],
           'thresholds':report.get('thresholds'),'events':events,'playback':playback,'resources':resources,
           'archive':archive,'failures':report['failures'],'media_errors':report['mediaErrors'],
           'unexpected_media_errors':unexpected_media_errors,'restart_http_ready_ms':report.get('restartHttpReadyMs')}
summary['recording_controls_before_restart']=report.get('recordingControls')
summary['recording_controls_after_restart']=report.get('afterRestartControls')
for filename,key in [('archive-startup.json','archive_startup'),('quota.json','quota'),('disk-wait.json','disk_wait')]:
    evidence=root.parent/filename
    if evidence.exists():
        data=json.loads(evidence.read_text())
        summary[key]=({k:v for k,v in data.items() if k not in ['work','files','probes']} if key=='quota' else data)
        summary['passed'] &= data['passed']
previous=root.parent/'pre-recording-buffer'/'summary.json'
if previous.exists() and previous.parent!=root:
    data=json.loads(previous.read_text())
    summary['previous_failed_soak']={'passed':data['passed'],'measured_seconds':data['measured_seconds'],
                                    'binary_sha256':data['fixture']['binary_sha256'],
                                    'files':data['archive']['files'],'failures':data['archive']['failures']}
(root/'summary.json').write_text(json.dumps(summary,indent=2))
print(json.dumps(summary,indent=2))
