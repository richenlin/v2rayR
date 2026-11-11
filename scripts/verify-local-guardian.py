#!/usr/bin/env python3
"""macOS native guardian smoke: real sing-box, loopback HTTP, no proxy writes."""
import argparse, hashlib, http.server, json, os, pathlib, signal, socket, subprocess, tempfile, threading, time, sys

def wait(check, seconds=15):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        if check(): return
        time.sleep(.05)
    raise AssertionError('deadline exceeded')

def alive(pid):
    try: os.kill(pid, 0); return True
    except ProcessLookupError: return False

def port():
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0)); return s.getsockname()[1]

def ready(p):
    try:
        with socket.create_connection(('127.0.0.1', p), timeout=.1): return True
    except OSError: return False

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200); self.end_headers(); self.wfile.write(b'guardian-local-ok')
    def log_message(self, *_): pass

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--executable', required=True)
    parser.add_argument('--core', required=True)
    parser.add_argument('--output', required=True)
    args = parser.parse_args()
    if sys.platform != 'darwin': raise SystemExit('This native smoke script requires macOS')
    out = pathlib.Path(args.output).resolve(); out.mkdir(parents=True, exist_ok=True)
    before = subprocess.check_output(['/usr/sbin/scutil', '--proxy'])
    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    results = []
    try:
        for mode in ['lease-close', 'parent-sigkill', 'hup-int-term', 'core-exits-first']:
            with tempfile.TemporaryDirectory(prefix='v2rayr-native-') as temp:
                root = pathlib.Path(temp); listen = port()
                config = root / 'config.json'
                config.write_text(json.dumps({'inbounds':[{'type':'mixed','listen':'127.0.0.1','listen_port':listen}],
                    'outbounds':[{'type':'direct','tag':'direct'}], 'route':{'final':'direct'}}))
                journal = root / 'takeover.json'
                # Valid macOS snapshot without owned slots: exercises capture /
                # restore / journal deletion, never changes the system proxy.
                journal.write_text(json.dumps({'services':[], 'applied_at_ms':1}))
                command = [str(pathlib.Path(args.executable).resolve()), '--internal-core-guardian', str(journal),
                    str(pathlib.Path(args.core).resolve()), 'run', '-c', str(config)]
                log = open(out / (mode + '.log'), 'wb')
                helper = None; guardian = None; child_pid = None; guardian_pid = None
                start = time.monotonic()
                try:
                    if mode == 'parent-sigkill':
                        manifest = root / 'guardian.pid'
                        code = 'import subprocess,sys,time,pathlib;p=subprocess.Popen(sys.argv[2:],stdin=subprocess.PIPE,start_new_session=True);pathlib.Path(sys.argv[1]).write_text(str(p.pid));time.sleep(60)'
                        helper = subprocess.Popen(['python3','-c',code,str(manifest),*command], stdout=log, stderr=log)
                        wait(manifest.exists); guardian_pid = int(manifest.read_text())
                    else:
                        guardian = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=log, stderr=log, start_new_session=True)
                        guardian_pid = guardian.pid
                    wait(lambda: ready(listen))
                    child_pid = int(subprocess.check_output(['/usr/sbin/lsof','-t',f'-iTCP:{listen}','-sTCP:LISTEN']).decode().strip())
                    evidence = subprocess.check_output(['ps','-o','pid,ppid,pgid,stat,command','-p',f'{guardian_pid},{child_pid}']).decode()
                    request = f'GET http://127.0.0.1:{server.server_port}/health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n'.encode()
                    with socket.create_connection(('127.0.0.1', listen), timeout=5) as conn:
                        conn.sendall(request); data = b''
                        while True:
                            try: chunk = conn.recv(4096)
                            except ConnectionResetError: break
                            if not chunk: break
                            data += chunk
                    assert b'200 OK' in data and b'guardian-local-ok' in data, data
                    cleanup_start = time.monotonic()
                    if mode == 'lease-close': guardian.stdin.close()
                    elif mode == 'parent-sigkill': helper.kill(); helper.wait(timeout=3)
                    elif mode == 'core-exits-first': os.kill(child_pid, signal.SIGKILL)
                    else:
                        for sig in [signal.SIGHUP, signal.SIGINT]:
                            os.kill(guardian_pid, sig); time.sleep(.15)
                            assert guardian.poll() is None and ready(listen)
                        os.kill(guardian_pid, signal.SIGTERM)
                    if guardian:
                        guardian.wait(timeout=15)
                        assert (guardian.returncode != 0) if mode == 'core-exits-first' else (guardian.returncode == 0), guardian.returncode
                    else: wait(lambda: not alive(guardian_pid))
                    wait(lambda: not alive(child_pid))
                    assert not ready(listen)
                    if mode != 'core-exits-first': assert not journal.exists(), 'no-op journal not recovered'
                    # A spontaneous core exit is reported to the app; the app,
                    # not this standalone guardian fixture, owns that recovery.
                    results.append({'case':mode,'result':'PASS','cleanupSeconds':round(time.monotonic()-cleanup_start,3),
                        'guardianPid':guardian_pid,'corePid':child_pid,'processes':evidence,
                        'journalRemoved':not journal.exists()})
                finally:
                    if helper and helper.poll() is None: helper.kill(); helper.wait()
                    if guardian and guardian.poll() is None:
                        guardian.stdin.close(); guardian.wait(timeout=15)
                    log.close()
    finally:
        server.shutdown()
        after = subprocess.check_output(['/usr/sbin/scutil', '--proxy'])
        (out / 'system-proxy-before.txt').write_bytes(before)
        (out / 'system-proxy-after.txt').write_bytes(after)
        (out / 'native-guardian-results.json').write_text(json.dumps({'executable':str(pathlib.Path(args.executable).resolve()),
            'sha256':hashlib.sha256(pathlib.Path(args.executable).read_bytes()).hexdigest(), 'cases':results,
            'systemProxyUnchanged':before == after, 'scope':'real core + HTTP; native proxy restore with no owned slots; no OS proxy writes'}, indent=2))
        assert before == after, 'system proxy changed during test'
    print(json.dumps(results, indent=2))

if __name__ == '__main__': main()
