#!/usr/bin/env python3
"""Summarize every comparison run, retaining failures and min/max CPU spread."""
import json, pathlib, statistics

root = pathlib.Path(__file__).resolve().parents[1] / 'test-results/go2rtc-comparison'
rows = []
for mode, filename in [('browser', 'capacity.json'), ('transport', 'transport.json')]:
    for path in sorted((root / mode).glob('*/' + filename)):
        report = json.loads(path.read_text(encoding='utf-8-sig'))
        for stage in report['stages']:
            row = {'mode': mode, 'run': path.parent.name, 'engine': report['engine'],
                   'connections': stage['viewers'], 'passed': stage['passed']}
            if mode == 'browser' and 'clients' in stage:
                row.update(cpu=stage['cpuMeanOneCorePercent'], rss_mib=stage['rssPeakMiB'],
                           fps=stage['meanFps'], min_fps=stage['minFps'], mbps=stage['totalMbps'],
                           lost=stage['totalLost'], freezes=stage['totalFreezes'],
                           client_cpu=stage['browserCpuOneCorePercent'],
                           input_connections=stage['sourceInputsMax'],
                           protocols=sorted(set(c.get('protocol', 'missing') for c in stage['clients'])))
            elif mode == 'transport':
                row.update(cpu=stage['server_cpu_one_core_percent'], rss_mib=stage['server_peak_rss_mib'],
                           fps=statistics.mean(c['fps'] for c in stage['clients']),
                           min_fps=stage['min_complete_rtp_fps'], mbps=stage['total_mbps'],
                           sequence_discontinuities=stage['sequence_discontinuities'],
                           client_cpu=stage['receiver_cpu_one_core_percent'])
            else:
                row['error'] = stage.get('error', 'No metrics')
            rows.append(row)
groups = []
for key in sorted(set((r['mode'], r['engine'], r['connections']) for r in rows)):
    matched = [r for r in rows if (r['mode'], r['engine'], r['connections']) == key]
    measured = [r for r in matched if 'cpu' in r]
    group = dict(zip(['mode', 'engine', 'connections'], key))
    group.update(runs=len(matched), measured_runs=len(measured), all_passed=all(r['passed'] for r in matched))
    if measured:
        group.update({field: statistics.mean(r[field] for r in measured)
                      for field in ['cpu', 'rss_mib', 'fps', 'mbps', 'client_cpu']})
        group.update(cpu_min=min(r['cpu'] for r in measured), cpu_max=max(r['cpu'] for r in measured),
                     min_fps=min(r['min_fps'] for r in measured))
    groups.append(group)
runs = []
for mode, requested in [('browser', [32, 64]), ('transport', [128, 512])]:
    for index, engine in enumerate(['rust', 'go2rtc', 'go2rtc', 'rust'], 1):
        name = f'{index}-{engine}'
        directory = root / mode / name
        stages = [r for r in rows if r['mode'] == mode and r['run'] == name]
        entry = {'mode': mode, 'run': name, 'finished': False,
                 'unmeasured_connections': [n for n in requested if not any(
                     r['connections'] == n and 'cpu' in r for r in stages)]}
        for filename, key in [('environment.json', 'start'), ('completion.json', 'end')]:
            path = directory / filename
            if path.exists():
                data = json.loads(path.read_text(encoding='utf-8-sig'))
                entry[key] = {k: data[k] for k in ['startedAt', 'finishedAt', 'exitCode', 'hostCpuPercent'] if k in data}
        entry['finished'] = bool(stages) and 'end' in entry
        if entry['unmeasured_connections']:
            entry['reason'] = 'Stopped after quality failure' if stages and not stages[-1]['passed'] else 'Incomplete measurement'
        runs.append(entry)
summary = {'rows': rows, 'groups': groups, 'runs': runs,
           'all_runs_finished': all(r['finished'] for r in runs),
           'all_requested_stages_measured': not any(r['unmeasured_connections'] for r in runs)}
(root / 'summary.json').write_text(json.dumps(summary, indent=2))
print(json.dumps(summary, indent=2))
