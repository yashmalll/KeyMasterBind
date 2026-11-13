"""Integration checks without third-party dependencies or physical input devices."""
import copy
import http.client
import json
from pathlib import Path
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
subprocess.run(['cargo', 'build', '--offline'], cwd=ROOT, check=True)
BINARY = ROOT / 'target/debug/keymasterbind'

def request(path, data=None, headers=None, raw=None):
    conn = http.client.HTTPConnection('127.0.0.1', 8765, timeout=4)
    body = raw if raw is not None else json.dumps(data, ensure_ascii=False).encode() if data is not None else None
    h = {'Content-Type': 'application/json'} if body is not None else {}
    h.update(headers or {})
    conn.request('POST' if body is not None else 'GET', path, body, h)
    res = conn.getresponse()
    result = res.status, res.read()
    conn.close()
    return result

def launch(directory):
    p = subprocess.Popen([str(BINARY), '--no-auto'], cwd=directory, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    for _ in range(60):
        if p.poll() is not None:
            raise RuntimeError(p.stderr.read().decode())
        try:
            request('/api/status')
            return p
        except OSError:
            time.sleep(.05)
    p.terminate()
    raise RuntimeError('Server failed to start')

# Do not touch an already-running user instance.
try:
    conn = http.client.HTTPConnection('127.0.0.1', 8765, timeout=.2)
    conn.connect()
except OSError:
    pass
else:
    conn.close()
    raise SystemExit('Port 8765 is occupied; stop the app before running smoke tests')

with tempfile.TemporaryDirectory(prefix='keymasterbind-test-') as directory:
    p = launch(directory)
    try:
        for path in ['/', '/app.js', '/style.css']:
            status, body = request(path)
            assert status == 200 and body, path
        status, body = request('/api/config')
        config = json.loads(body)
        config['profiles'][0]['bindings']['71'] = {'kind': 'chord', 'keys': [29, 46], 'label': 'Копировать'}
        config['profiles'][0]['bindings']['72'] = {'kind': 'macro', 'steps': [{'keys': [29, 47], 'delay': 20}]}
        assert request('/api/config', config)[0] == 200
        assert json.loads(request('/api/config')[1]) == config
        invalid = copy.deepcopy(config)
        invalid['profiles'][0]['bindings']['71']['keys'] = [9999]
        assert request('/api/config', invalid)[0] == 400
        invalid = copy.deepcopy(config)
        invalid['profiles'][0]['layout'] = [1] * 19
        assert request('/api/config', invalid)[0] == 400
        assert request('/api/config', raw=b'{')[0] == 400
        assert request('/api/config', config, {'Origin': 'https://example.com'})[0] == 403
        assert request('/api/config', config, {'Content-Type': 'text/plain'})[0] == 415
        assert request('/api/status', headers={'Host': 'attacker.invalid'})[0] == 403
        assert request('/api/start', {'device': '/dev/input/not-authorized'})[0] == 200
        time.sleep(.05)
        status = json.loads(request('/api/status')[1])
        assert not status['running'] and status['error']
        assert status['firmware_supported'] is False
        assert request('/api/stop', {})[0] == 200
        assert json.loads(request('/api/config')[1]) == config
    finally:
        p.terminate()
        p.wait(timeout=5)
    p = launch(directory)
    try:
        assert json.loads(request('/api/config')[1]) == config
        assert json.loads(request('/api/status')[1])['running'] is False
    finally:
        p.terminate()
        p.wait(timeout=5)
print('HTTP smoke: pages, profiles, validation, persistence, Host/Origin, device restrictions OK')
