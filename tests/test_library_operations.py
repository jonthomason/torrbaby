"""Disposable end-to-end tests for the Rust daemon; never contacts household services.
Run after `cargo build --locked`: python3 tests/test_library_operations.py
"""
import json
from pathlib import Path
import socket
import sqlite3
import subprocess
import tempfile
import threading
import time
import unittest
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = Path(__file__).resolve().parents[1]


class LibraryOperationsTest(unittest.TestCase):
    def test_delete_confirmation_restart_and_cleanup(self):
        self.run_operation("delete")

    def test_junk_move_keeps_seeding_and_only_deletes_when_finished(self):
        self.run_operation("junk")

    def run_operation(self, action):
        with tempfile.TemporaryDirectory(prefix="torrbaby-operation-") as directory:
            root = Path(directory).resolve()
            for folder in ("Downloads", "Movies", "TV"):
                (root / folder).mkdir()
            media = root / "Movies" / "Fixture.mp4"
            subtitle = media.with_suffix(".en.srt")
            neighbor = root / "Movies" / "Neighbor.mp4"
            media.write_bytes(b"disposable fixture")
            subtitle.write_text("1\n00:00:00,000 --> 00:00:01,000\nFixture\n")
            neighbor.write_bytes(b"keep me")
            torrent_hash = "a" * 40
            torrent = dict(hash_string=torrent_hash, name=media.name,
                           download_dir=str(media.parent), metadata_percent_complete=1.0,
                           percent_done=1.0, left_until_done=0, error=0, status=0,
                           is_finished=action == "delete", added_date=1,
                           files=[dict(name=media.name, length=media.stat().st_size,
                                       bytes_completed=media.stat().st_size)])
            live = [torrent]
            calls = []
            moves = []
            starts = []
            class RPC(BaseHTTPRequestHandler):
                def log_message(self, *args):
                    pass
                def do_POST(self):
                    payload = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                    method, params = payload["method"], payload.get("params", {})
                    if method == "torrent_get":
                        result = {"torrents": list(live)}
                    elif method == "torrent_add":
                        torrent["download_dir"] = params["download_dir"]
                        torrent["percent_done"] = 0
                        torrent["files"][0]["bytes_completed"] = 0
                        live[:] = [torrent]
                        result = {"torrent_added":{"hash_string":torrent_hash,"name":torrent["name"]}}
                    elif method == "torrent_set_location":
                        moves.append(params)
                        destination = Path(params["location"])
                        for file in torrent["files"]:
                            source = Path(torrent["download_dir"]) / file["name"]
                            target = destination / file["name"]
                            target.parent.mkdir(parents=True, exist_ok=True)
                            source.rename(target)
                        torrent["download_dir"] = str(destination)
                        self.send_error(500, "fixture response lost after move")
                        return
                    elif method == "torrent_start":
                        starts.append(params)
                        # Deliberately leave a paused/stalled seed to test no early deletion.
                        torrent["status"] = 0
                        result = {}
                    elif method == "torrent_remove":
                        calls.append(params)
                        live.clear()
                        # The RPC succeeds, but its response is lost. Recovery must
                        # observe absence rather than repeating a destructive RPC.
                        self.send_error(500, "fixture response lost after detach")
                        return
                    elif method == "session_get":
                        result = {"seed_ratio_limit": 1, "seed_ratio_limited": True}
                    else:
                        result = {}
                    body = json.dumps({"jsonrpc": "2.0", "id": payload.get("id"), "result": result}).encode()
                    self.send_response(200)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
            rpc = ThreadingHTTPServer(("127.0.0.1", 0), RPC)
            threading.Thread(target=rpc.serve_forever, daemon=True).start()
            self.addCleanup(rpc.server_close)
            self.addCleanup(rpc.shutdown)
            with socket.socket() as sock:
                sock.bind(("127.0.0.1", 0))
                port = sock.getsockname()[1]
            token = "fixture-token-not-a-real-credential-123456"
            (root / "token").write_text(token)
            config = dict(listen_host="127.0.0.1", listen_port=port,
                          token_file=str(root / "token"), state_file=str(root / "state.json"),
                          transmission_url=f"http://127.0.0.1:{rpc.server_port}/rpc",
                          source=str(root / "Downloads"), movies=str(root / "Movies"),
                          tv=str(root / "TV"), poll_seconds=5, paused=True, background_subtitles=False)
            (root / "config.json").write_text(json.dumps(config))
            db = sqlite3.connect(root / "state.sqlite3")
            db.executescript((ROOT / "server/schema.sql").read_text())
            db.executescript((ROOT / "server/profiles_schema.sql").read_text())
            db.execute("PRAGMA user_version=10")
            asset = dict(id="asset-fixture", path=str(media), complete=True, available=True,
                         torrent_hash=torrent_hash, file_index=0, size=media.stat().st_size,
                         role="main", status="matched", entity_id="fixture")
            db.execute("INSERT INTO library_assets VALUES(?,?)", (asset["id"], json.dumps(asset)))
            db.execute("INSERT INTO media VALUES(?,?,?,?,?)", (torrent_hash+":0", torrent_hash, 0, media.name, media.stat().st_size))
            db.execute("INSERT INTO profile_playback(profile_id,media_id,watched) VALUES(?,?,1)", ("profile-jon",torrent_hash+":0"))
            db.commit()
            db.close()
            process = None
            log = open(root / "server.log", "w+")
            def request(method, path, body=None):
                req = urllib.request.Request(f"http://127.0.0.1:{port}"+path,
                      data=None if body is None else json.dumps(body).encode(), method=method,
                      headers={"Authorization":"Bearer "+token,"Content-Type":"application/json"})
                with urllib.request.urlopen(req, timeout=5) as response:
                    return json.load(response)
            def start():
                nonlocal process
                process = subprocess.Popen([str(ROOT / "target/debug/torrbaby"), "serve", "--config", str(root / "config.json")], stdout=log, stderr=log)
                for _ in range(100):
                    if process.poll() is not None:
                        log.seek(0)
                        self.fail(log.read())
                    try:
                        request("GET", "/v1/profiles")
                        return
                    except (OSError, urllib.error.URLError):
                        time.sleep(.05)
                self.fail("Fixture daemon did not start")
            def stop():
                if process and process.poll() is None:
                    process.terminate()
                    process.wait(timeout=10)
            try:
                start()
                # Durable derived caches for this asset and an unrelated neighbor.
                for asset_id, cache_id in [("asset-fixture", "b"*64), ("asset-neighbor", "c"*64)]:
                    subtitles = root / "subtitles"
                    record = dict(id=cache_id, asset_id=asset_id, input=str(media), revision="fixture",
                                  track=0, source="en", target="zh-Hans", state="ready", updated=time.time(), error=None)
                    (subtitles / (cache_id+".json")).write_text(json.dumps(record))
                    (subtitles / (cache_id+".vtt")).write_text("WEBVTT\n")
                    (subtitles / (cache_id+".progress")).write_text("fixture")
                    virtual = root / "virtual-media-v1"
                    (virtual / cache_id).mkdir()
                    (virtual / cache_id / "fixture-index").write_text("fixture")
                    (virtual / (cache_id+".json")).write_text(json.dumps(dict(id=cache_id,asset=asset_id,
                        input=str(media),revision="fixture",state="ready",error=None,captions=[])))
                route = "/v1/library/assets/asset-fixture/manage-files"
                preview = request("GET", route)
                self.assertEqual(preview["allowed_actions"], ["delete"] if action == "delete" else ["delete", "junk"])
                self.assertEqual(len(preview["files"]), 2)
                with self.assertRaises(urllib.error.HTTPError) as error:
                    request("POST", route, {"action":action, "confirmation_token":"stale"})
                self.assertEqual(error.exception.code, 400)
                error.exception.close()
                self.assertTrue(media.exists())
                confirmation = {"action":action, "confirmation_token":preview["confirmation_token"]}
                operation = request("POST", route, confirmation)
                self.assertEqual(request("POST", route, confirmation)["id"],operation["id"])
                self.assertFalse(any(a["id"] == "asset-fixture" for t in request("GET", "/v1/library")["titles"] for a in t["assets"]))
                stop()
                start()
                if action == "junk":
                    for _ in range(160):
                        state = next(o for o in request("GET", "/v1/library/operations")["operations"] if o["id"] == operation["id"])
                        if state["phase"] == "seeding":
                            break
                        time.sleep(.2)
                    self.assertEqual(state["phase"], "seeding", state)
                    destination = Path(state["destination"])
                    self.assertEqual((destination / media.name).read_bytes(), b"disposable fixture")
                    self.assertTrue((destination / subtitle.name).exists())
                    self.assertFalse(media.exists())
                    self.assertFalse(subtitle.exists())
                    self.assertEqual(len(moves), 1)
                    self.assertEqual(starts, [{"ids":[torrent_hash]}])
                    stop()
                    start()
                    time.sleep(6)
                    state = next(o for o in request("GET", "/v1/library/operations")["operations"] if o["id"] == operation["id"])
                    self.assertEqual(state["phase"], "seeding")
                    visible_torrent = next(t for t in request("GET", "/v1/torrents") if t["hash_string"] == torrent_hash)
                    self.assertEqual(visible_torrent["file_operation"]["action"], "junk")
                    self.assertEqual(visible_torrent["file_operation"]["phase"], "seeding")
                    with self.assertRaises(urllib.error.HTTPError) as conflict:
                        request("POST", "/v1/torrents/" + torrent_hash + "/remove", {"delete_data":True,"confirm_hash":torrent_hash})
                    self.assertEqual(conflict.exception.code, 400)
                    conflict.exception.close()
                    self.assertTrue((destination / media.name).exists())
                    self.assertFalse(calls)
                    self.assertFalse(any(a["id"] == "asset-fixture" for t in request("GET", "/v1/library")["titles"] for a in t["assets"]))
                    torrent["is_finished"] = True
                for _ in range(160):
                    operations = request("GET", "/v1/library/operations")["operations"]
                    state = next(o for o in operations if o["id"] == operation["id"])
                    if state["phase"] == "done":
                        break
                    time.sleep(.2)
                self.assertEqual(state["phase"], "done", state)
                self.assertEqual(calls, [{"ids":[torrent_hash],"delete_local_data":False}])
                self.assertFalse(media.exists())
                self.assertFalse(subtitle.exists())
                if action == "junk":
                    self.assertFalse(Path(state["destination"]).exists())
                self.assertEqual(neighbor.read_bytes(), b"keep me")
                for ext in ("json", "vtt", "progress"):
                    self.assertFalse((root / "subtitles" / ("b"*64+"."+ext)).exists())
                    self.assertTrue((root / "subtitles" / ("c"*64+"."+ext)).exists())
                self.assertFalse((root / "virtual-media-v1" / ("b"*64)).exists())
                self.assertFalse((root / "virtual-media-v1" / ("b"*64+".json")).exists())
                self.assertTrue((root / "virtual-media-v1" / ("c"*64)).exists())
                self.assertEqual(request("POST", route, confirmation)["id"],operation["id"])
                self.assertTrue(request("GET", "/v1/playback")[0]["watched"])
                request("POST", "/v1/torrents", {"magnet":"magnet:?xt=urn:btih:"+torrent_hash})
                # Re-add archives only the finished removal; old confirmations stay harmless.
                self.assertEqual(request("POST", route, confirmation)["id"], operation["id"])
                self.assertEqual(request("GET", "/v1/library/operations")["operations"], [])
                self.assertFalse(any(a["id"] == "asset-fixture" for t in request("GET", "/v1/library")["titles"] for a in t["assets"]))
                self.assertTrue(request("GET", "/v1/playback")[0]["watched"])
                self.assertEqual(len(live), 1)
            finally:
                stop()
                log.close()

if __name__ == "__main__":
    unittest.main()
