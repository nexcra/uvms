"""Read-only Linux service metrics, emitted once per second; no camera secrets."""
import json, os, pathlib, subprocess, time
pid = int(subprocess.check_output(['systemctl', 'show', '-p', 'MainPID', '--value', os.environ.get('UVMS_BENCH_SERVICE','uvms')], text=True))
hz = os.sysconf('SC_CLK_TCK')
previous = None
while True:
    root = pathlib.Path(f'/proc/{pid}')
    try:
        fields = (root/'stat').read_text().rsplit(')', 1)[1].split()
        ticks = int(fields[11]) + int(fields[12])
        status = dict(line.split(':', 1) for line in (root/'status').read_text().splitlines())
        now = time.time()
        sockets = set()
        for fd in (root/'fd').iterdir():
            try:
                target = fd.readlink().as_posix()
                if target.startswith('socket:['): sockets.add(target[8:-1])
            except OSError: pass
        camera_inputs = 0
        source_inputs = 0
        for line in (root/'net/tcp').read_text().splitlines()[1:]:
            parts = line.split()
            if parts[9] in sockets and parts[3] == '01':
                port = int(parts[2].split(':')[1], 16)
                if port == 554: camera_inputs += 1
                if port == int(os.environ.get('UVMS_BENCH_SOURCE_PORT', '554')): source_inputs += 1
        udp_ports = []
        for table in ['udp', 'udp6']:
            for line in (root / 'net' / table).read_text().splitlines()[1:]:
                parts = line.split()
                if parts[9] in sockets: udp_ports.append(int(parts[1].split(':')[1], 16))
        data = {'time':now, 'pid':pid, 'rss_mib':int(status['VmRSS'].split()[0])/1024,
                'logical_cpus':os.cpu_count(), 'camera_inputs':camera_inputs,
                'source_inputs':source_inputs, 'udp_ports':sorted(udp_ports), 'threads':int(status['Threads']),
                'fd_count':len(list((root/'fd').iterdir()))}
        if previous:
            data['cpu_percent_one_core'] = 100*(ticks-previous[1])/hz/(now-previous[0])
        previous = (now,ticks)
        print(json.dumps(data), flush=True)
    except (FileNotFoundError, ProcessLookupError):
        print(json.dumps({'error':'service process exited'}), flush=True)
        break
    time.sleep(1)
