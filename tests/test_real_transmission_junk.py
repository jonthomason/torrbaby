"""Isolated real-Transmission acceptance. No production config or media is used.
Requires transmission-daemon 4.1+ and a built target/debug/torrbaby.
Run: python3 tests/test_real_transmission_junk.py
"""
import base64
import hashlib
import json
import os
from pathlib import Path
import shutil
import socket
import sqlite3
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[1]


def port():
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]


def wait_for(fn, description, timeout=65):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        try:
            last = fn()
            if last:
                return last
        except (OSError, urllib.error.URLError) as e:
            last = str(e)
        time.sleep(.25)
    raise AssertionError(f'{description}: {last}')


def bencode(v):
    if isinstance(v, int):
        return b'i' + str(v).encode() + b'e'
    if isinstance(v, bytes):
        return str(len(v)).encode() + b':' + v
    return b'd' + b''.join(bencode(k) + bencode(v[k]) for k in sorted(v)) + b'e'


def main():
    transmission = shutil.which('transmission-daemon')
    assert transmission, 'transmission-daemon is required'
    with tempfile.TemporaryDirectory(prefix='torrbaby-real-junk-') as folder:
        root = Path(folder).resolve()
        rpc_port, app_port, peer_port = port(), port(), port()
        processes = []
        logs = []
        for name in ('Downloads', 'Movies', 'TV', 'transmission'):
            (root / name).mkdir()
        media = root / 'Movies/Fixture.mp4'
        payload = b'disposable test media\n' * 1024
        media.write_bytes(payload)
        subtitle = media.with_suffix('.en.srt')
        subtitle.write_text('1\n00:00:00,000 --> 00:00:01,000\nFixture\n')
        neighbor = root / 'Movies/Neighbor.mp4'
        neighbor.write_bytes(b'preserve neighbor')
        info = {b'name': b'Fixture.mp4', b'length': len(payload), b'piece length': 32768,
                b'pieces': hashlib.sha1(payload).digest(), b'private': 1}
        metainfo = base64.b64encode(bencode({b'info': info})).decode()
        torrent_hash = hashlib.sha1(bencode(info)).hexdigest()
        session = ''

        def rpc(method, params=None):
            nonlocal session
            for _ in range(2):
                req = urllib.request.Request(f'http://127.0.0.1:{rpc_port}/transmission/rpc',
                    json.dumps(dict(jsonrpc='2.0', id=1, method=method, params=params or {})).encode(),
                    headers={'Content-Type': 'application/json', 'X-Transmission-Session-Id': session})
                try:
                    with urllib.request.urlopen(req, timeout=5) as response:
                        value = json.load(response)
                        assert 'error' not in value, value
                        return value['result']
                except urllib.error.HTTPError as error:
                    if error.code != 409:
                        raise
                    session = error.headers['X-Transmission-Session-Id']
                    error.close()
            raise AssertionError('RPC negotiation failed')

        def torrent():
            items = rpc('torrent_get', {'ids': [torrent_hash], 'fields': ['hash_string', 'download_dir', 'percent_done', 'is_finished', 'status']})['torrents']
            return items[0] if items else None

        def start(args, name):
            log = open(root / (name + '.log'), 'w+')
            logs.append(log)
            process = subprocess.Popen(args, stdout=log, stderr=log)
            processes.append(process)
            return process

        token = 'disposable-real-transmission-test-token'
        def request(method, path, body=None):
            req = urllib.request.Request(f'http://127.0.0.1:{app_port}' + path,
                None if body is None else json.dumps(body).encode(), method=method,
                headers={'Content-Type': 'application/json', 'Authorization': 'Bearer ' + token})
            with urllib.request.urlopen(req, timeout=5) as response:
                return json.load(response)

        def operation():
            return request('GET', '/v1/library/operations')['operations'][0]

        try:
            start([transmission, '-f', '-g', str(root / 'transmission'), '-p', str(rpc_port),
                   '-P', str(peer_port), '-r', '127.0.0.1', '-i', '127.0.0.1', '-I', '::1',
                   '-T', '-O', '-Y', '-M', '-C', '-w', str(root / 'Downloads')], 'transmission')
            wait_for(lambda: rpc('session_get'), 'Transmission startup')
            rpc('torrent_add', {'metainfo': metainfo, 'download_dir': str(media.parent), 'paused': True})
            rpc('torrent_verify', {'ids': [torrent_hash]})
            wait_for(lambda: (t := torrent()) and t['percent_done'] == 1, 'Torrent verification')
            rpc('torrent_set', {'ids': [torrent_hash], 'seed_ratio_mode': 2, 'seed_idle_mode': 2})
            (root / 'token').write_text(token)
            config = dict(listen_host='127.0.0.1', listen_port=app_port, advertise_lan=False,
                token_file=str(root / 'token'), state_file=str(root / 'state.json'),
                transmission_url=f'http://127.0.0.1:{rpc_port}/transmission/rpc',
                source=str(root / 'Downloads'), movies=str(root / 'Movies'), tv=str(root / 'TV'),
                poll_seconds=5, paused=True, background_subtitles=False)
            (root / 'config.json').write_text(json.dumps(config))
            db = sqlite3.connect(root / 'state.sqlite3')
            db.executescript((ROOT / 'server/schema.sql').read_text())
            db.executescript((ROOT / 'server/profiles_schema.sql').read_text())
            db.execute('PRAGMA user_version=10')
            asset = dict(id='asset-real-fixture', path=str(media), complete=True, available=True,
                         torrent_hash=torrent_hash, file_index=0, size=len(payload), role='main', status='matched', entity_id='fixture')
            db.execute('INSERT INTO library_assets VALUES(?,?)', (asset['id'], json.dumps(asset)))
            db.commit()
            db.close()
            server = start([str(ROOT / 'target/debug/torrbaby'), 'serve', '--config', str(root / 'config.json')], 'torrbaby')
            wait_for(lambda: request('GET', '/v1/profiles'), 'Torrbaby startup')
            route = '/v1/library/assets/asset-real-fixture/manage-files'
            preview = request('GET', route)
            assert 'junk' in preview['allowed_actions'], preview
            request('POST', route, {'action': 'junk', 'confirmation_token': preview['confirmation_token']})
            wait_for(lambda: operation()['phase'] == 'seeding', 'Relocation', 90)
            dest = Path(operation()['destination'])
            assert torrent()['download_dir'] == str(dest)
            assert (dest / media.name).read_bytes() == payload
            assert (dest / subtitle.name).exists() and not subtitle.exists() and not media.exists()
            assert torrent()['is_finished'] is False
            rpc('torrent_stop', {'ids': [torrent_hash]})
            time.sleep(7)
            assert operation()['phase'] == 'seeding' and (dest / media.name).exists()
            print('PASS: real relocation, sidecars, and paused seed retention', flush=True)
            rpc('torrent_start', {'ids': [torrent_hash]})
            # Starting after setting an already-met ratio disables that ratio in Transmission.
            rpc('torrent_set', {'ids': [torrent_hash], 'seed_ratio_mode': 1, 'seed_ratio_limit': 0})
            wait_for(lambda: (t := torrent()) and t['is_finished'], 'Seed-policy completion')
            wait_for(lambda: operation()['phase'] == 'done', 'Automatic cleanup', 90)
            assert torrent() is None and not dest.exists()
            assert neighbor.read_bytes() == b'preserve neighbor'
            print('PASS: seed-policy completion triggers detach and exact file cleanup', flush=True)
        except Exception:
            for log in logs:
                log.flush()
                log.seek(0)
                print(log.read()[-6000:])
            raise
        finally:
            for process in reversed(processes):
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()
            for log in logs:
                log.close()


if __name__ == '__main__':
    main()
