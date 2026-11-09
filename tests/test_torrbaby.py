import copy
import json
from pathlib import Path
import shutil
import tempfile
import threading
import time
import unittest
import urllib.error
import urllib.request
from unittest.mock import patch

import torrbaby as tb

HASH = "a" * 40


def torrent(root, name="Example.2025.mkv"):
    return {"hash_string": HASH, "name": name, "download_dir": str(root), "metadata_percent_complete": 1,
            "percent_done": 1, "left_until_done": 0, "error": 0, "status": 6,
            "files": [{"name": name, "length": 4, "bytes_completed": 4}]}


class RPC:
    def __init__(self, t):
        self.items = [t]
        self.calls = []

    def torrents(self, hashes=None):
        return copy.deepcopy([t for t in self.items if hashes is None or t["hash_string"] in hashes])

    def call(self, method, **params):
        self.calls.append((method, params))
        t = self.items[0]
        if method == "torrent_set_location":
            for top in {Path(f["name"]).parts[0] for f in t["files"]}:
                shutil.move(str(Path(t["download_dir"]) / top), str(Path(params["location"]) / top))
            t["download_dir"] = params["location"]
        if method == "torrent_rename_path":
            root = Path(t["download_dir"])
            old = params["path"]
            new = str(Path(old).parent / params["name"])
            (root / old).rename(root / new)
            for f in t["files"]:
                if f["name"] == old or f["name"].startswith(old + "/"):
                    f["name"] = new + f["name"][len(old):]
        return {}


class Plex:
    def __init__(self):
        self.calls = []
        self.fail = False

    def scan(self, roots):
        self.calls.append(roots)
        if self.fail:
            raise RuntimeError("Plex offline")
        return ["1", "2"]


class SorterTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name).resolve()
        self.config = {"source": str(self.root / "Downloads"), "movies": str(self.root / "Movies"), "tv": str(self.root / "TV"), "state_file": str(self.root / "state.json"), "paused": False, "poll_seconds": 30}
        for key in ("source", "movies", "tv"):
            Path(self.config[key]).mkdir()
        self.t = torrent(self.config["source"])
        (Path(self.config["source"]) / self.t["name"]).write_bytes(b"test")
        self.rpc = RPC(self.t)
        self.plex = Plex()
        self.s = tb.Service(self.config, self.rpc, self.plex)

    def tearDown(self):
        self.tmp.cleanup()

    def test_classification(self):
        for name in ["Show.S01.mkv", "Show.S01E03.mkv", "Show.S01.E03.mkv", "Show.Season.2.mkv", "Show.2x03.mkv", "Show.S01-S03.mkv"]:
            self.assertEqual(tb.classify(torrent("/tmp", name)), "tv", name)
        for name in ["Scream.2025.mkv", "Movie.2025.mkv", "Movie.S1000.mkv", "Movie.S01abc.mkv"]:
            self.assertEqual(tb.classify(torrent("/tmp", name)), "movies", name)
        self.assertIsNone(tb.classify(torrent("/tmp", "archive.zip")))
        t = torrent("/tmp", "Collection/Show.S02E03.mkv")
        t["name"] = "Collection"
        self.assertEqual(tb.classify(t), "tv")

    def test_no_partial_or_metadata_or_verifying_moves(self):
        for update in [{"metadata_percent_complete": 0}, {"status": 2}, {"error": 1}, {"percent_done": .99}, {"left_until_done": 1}, {"files": [{"name": "Example.2025.mkv", "length": 4, "bytes_completed": 3}]}]:
            t = dict(self.t, **update)
            self.assertIsNone(self.s.plan(t))

    def test_move_then_scan_and_restart_idempotence(self):
        self.s.tick()
        self.assertEqual(self.rpc.calls[0], ("torrent_set_location", {"ids": [HASH], "location": self.config["movies"], "move": True}))
        self.assertFalse(self.plex.calls)
        restarted = tb.Service(self.config, self.rpc, self.plex)
        restarted.tick()
        self.assertEqual(restarted.state["jobs"][HASH]["phase"], "done")
        self.assertEqual(len(self.plex.calls), 1)
        self.assertFalse((Path(self.config["source"]) / self.t["name"]).exists())
        restarted.tick()
        self.assertEqual(len(self.rpc.calls), 1)
        self.assertEqual(len(self.plex.calls), 1)

    def test_plex_failure_retries_without_repeat_move(self):
        self.plex.fail = True
        self.s.tick()
        self.s.tick()
        self.assertEqual(self.s.state["scan_error"], "Plex offline")
        restarted = tb.Service(self.config, self.rpc, self.plex)
        self.plex.fail = False
        restarted.tick()
        self.assertFalse(restarted.state["scans"])
        self.assertEqual(len(self.rpc.calls), 1)

    def test_no_overwrite_collision(self):
        (Path(self.config["movies"]) / self.t["name"]).write_bytes(b"keep")
        self.s.tick()
        self.assertEqual(self.s.state["jobs"][HASH]["phase"], "blocked")
        self.assertFalse(self.rpc.calls)
        self.assertEqual((Path(self.config["movies"]) / self.t["name"]).read_bytes(), b"keep")

    def test_missing_volume_does_not_create_destination(self):
        Path(self.config["movies"]).rmdir()
        self.s.tick()
        self.assertFalse(self.rpc.calls)
        self.assertFalse(Path(self.config["movies"]).exists())

    def test_shared_files_not_moved(self):
        other = copy.deepcopy(self.t)
        other["hash_string"] = "b" * 40
        self.rpc.items.append(other)
        self.s.tick()
        self.assertFalse(self.rpc.calls)
        self.assertIn("shared", self.s.state["jobs"][HASH]["error"])

    def test_external_move_not_repeated(self):
        self.s.tick()
        self.rpc.items[0]["download_dir"] = "/outside"
        self.s.state["jobs"][HASH]["attempted"] = 0
        self.s.tick()
        self.assertEqual(len(self.rpc.calls), 1)
        self.assertIn("changed", self.s.state["jobs"][HASH]["error"])

    def test_pause_and_settings_survive_restart(self):
        self.s.update_settings({"paused": True, "poll_seconds": 5})
        restarted = tb.Service(self.config, self.rpc, self.plex)
        restarted.tick()
        self.assertFalse(self.rpc.calls)
        self.assertEqual(restarted.settings()["poll_seconds"], 5)
        with self.assertRaises(ValueError):
            self.s.update_settings({"poll_seconds": 0})
        with self.assertRaises(ValueError):
            self.s.update_settings({"movies": self.config["source"]})

    def test_traversal_and_symlink_rejected(self):
        for name in ["../escape.mkv", "/escape.mkv", "dir/../../escape.mkv", "./escape.mkv"]:
            with self.assertRaises(ValueError):
                tb.checked_path(self.config["source"], name)
        (Path(self.config["movies"]) / "link").symlink_to(self.config["source"], target_is_directory=True)
        with self.assertRaises(ValueError):
            tb.checked_path(self.config["movies"], "link/file.mkv")

    def test_rename_via_rpc_preserves_extension(self):
        with self.assertRaises(ValueError):
            self.s.rename(HASH, self.t["name"], "Example")
        self.s.rename(HASH, self.t["name"], "Example (2025).mkv")
        self.assertEqual(self.rpc.calls[-1][0], "torrent_rename_path")
        self.assertTrue((Path(self.config["source"]) / "Example (2025).mkv").exists())
        self.assertTrue(self.s.state["scans"])

    def test_delete_explicit_confirmation_and_default_keep_data(self):
        with self.assertRaises(ValueError):
            self.s.action(HASH, "remove", {"delete_data": True})
        self.assertFalse(self.rpc.calls)
        self.s.action(HASH, "remove", {})
        self.assertFalse(self.rpc.calls[-1][1]["delete_local_data"])
        with self.assertRaises(ValueError):
            self.s.torrent("")

    def test_folder_payload_moves_entire_root(self):
        old = Path(self.config["source"]) / self.t["name"]
        old.unlink()
        root = Path(self.config["source"]) / "Show.S02"
        root.mkdir()
        (root / "Show.S02E01.mkv").write_bytes(b"test")
        (root / "notes.txt").write_bytes(b"note")
        self.t["name"] = root.name
        self.t["files"] = [{"name": root.name + "/" + n, "length": 4, "bytes_completed": 4} for n in ["Show.S02E01.mkv", "notes.txt"]]
        self.s.tick()
        self.s.tick()
        self.assertEqual(self.s.state["jobs"][HASH]["phase"], "done")
        self.assertTrue((Path(self.config["tv"]) / root.name / "notes.txt").exists())

    def test_rpc_timeout_after_success_recovers_without_duplicate(self):
        original = self.rpc.call
        def timeout(*args, **kwargs):
            original(*args, **kwargs)
            raise TimeoutError("unknown result")
        self.rpc.call = timeout
        self.s.tick()
        self.s.tick()
        self.assertEqual(self.s.state["jobs"][HASH]["phase"], "done")
        self.assertEqual(len(self.rpc.calls), 1)

    def test_http_auth_and_remote_controls(self):
        server = tb.Server(("127.0.0.1", 0), tb.Handler)
        server.service = self.s
        server.token = "secret" * 8
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        url = "http://127.0.0.1:" + str(server.server_port)
        try:
            with self.assertRaises(urllib.error.HTTPError) as e:
                urllib.request.urlopen(url + "/v1/status")
            self.assertEqual(e.exception.code, 401)
            e.exception.close()
            headers = {"Authorization": "Bearer " + server.token, "Content-Type": "application/json"}
            req = urllib.request.Request(url + "/v1/settings", b'{"paused":true}', headers, method="PATCH")
            with urllib.request.urlopen(req) as r:
                self.assertTrue(json.load(r)["paused"])
            req = urllib.request.Request(url + "/v1/status", headers=dict(headers, Origin="https://example.com"))
            with self.assertRaises(urllib.error.HTTPError) as e:
                urllib.request.urlopen(req)
            self.assertEqual(e.exception.code, 403)
            e.exception.close()
        finally:
            server.shutdown()
            server.server_close()
            thread.join()

    def test_signed_stream_ranges_head_and_tampering(self):
        server = tb.Server(("127.0.0.1", 0), tb.Handler)
        server.service, server.token = self.s, "secret" * 8
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        base = "http://127.0.0.1:" + str(server.server_port)
        try:
            req = urllib.request.Request(base + "/v1/torrents/" + HASH + "/play", b'{"file_index":0}', {"Authorization": "Bearer " + server.token}, method="POST")
            with urllib.request.urlopen(req) as r:
                link = json.load(r)["path"]
            with urllib.request.urlopen(urllib.request.Request(base + link, headers={"Range": "bytes=1-2"})) as r:
                self.assertEqual(r.status, 206)
                self.assertEqual(r.headers["Content-Range"], "bytes 1-2/4")
                self.assertEqual(r.read(), b"es")
            with urllib.request.urlopen(urllib.request.Request(base + link, method="HEAD")) as r:
                self.assertEqual(r.headers["Content-Length"], "4")
                self.assertEqual(r.read(), b"")
            for url, headers, code in [(base + link.replace("sig=", "sig=x"), {}, 401), (base + link, {"Range": "bytes=10-20"}, 416)]:
                with self.assertRaises(urllib.error.HTTPError) as e:
                    urllib.request.urlopen(urllib.request.Request(url, headers=headers))
                self.assertEqual(e.exception.code, code)
                e.exception.close()
            expires = int(time.time()) - 1
            sig = tb.stream_signature(server.token, HASH, 0, expires)
            with self.assertRaises(urllib.error.HTTPError) as e:
                urllib.request.urlopen(base + "/v1/stream/%s/0?expires=%s&sig=%s" % (HASH, expires, sig))
            self.assertEqual(e.exception.code, 401)
            e.exception.close()
        finally:
            server.shutdown()
            server.server_close()
            thread.join()


class ProtocolTests(unittest.TestCase):
    def test_byte_ranges(self):
        for header, expected in [(None, (0, 9)), ("bytes=0-1", (0, 1)), ("bytes=5-", (5, 9)), ("bytes=-3", (7, 9)), ("bytes=0-100", (0, 9)), ("bytes=-100", (0, 9))]:
            self.assertEqual(tb.byte_range(header, 10), expected)
        for header in ["bytes=", "bytes=-", "bytes=-0", "bytes=10-", "bytes=5-1", "bytes=0-1,4-5"]:
            with self.assertRaises(ValueError):
                tb.byte_range(header, 10)
    def test_rpc_409_retry_and_json_error(self):
        class Response:
            def __enter__(self):
                return self
            def __exit__(self, *_):
                pass
            def read(self):
                return b'{"jsonrpc":"2.0","result":{"ok":true},"id":1}'
        error = urllib.error.HTTPError("http://localhost", 409, "conflict", {"X-Transmission-Session-Id": "session"}, None)
        with patch("urllib.request.urlopen", side_effect=[error, Response()]) as mock:
            rpc = tb.RPC({"transmission_url": "http://localhost"})
            self.assertTrue(rpc.call("session_get")["ok"])
            self.assertEqual(mock.call_count, 2)
            self.assertEqual(rpc.headers["X-Transmission-Session-Id"], "session")
            error.close()
        class ErrorResponse(Response):
            def read(self):
                return b'{"error":{"message":"bad request"}}'
        with patch("urllib.request.urlopen", return_value=ErrorResponse()):
            with self.assertRaises(RuntimeError):
                rpc.call("session_get")


if __name__ == "__main__":
    unittest.main()
