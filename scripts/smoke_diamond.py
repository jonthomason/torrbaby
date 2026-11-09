#!/usr/bin/env python3
"""Opt-in live RPC test: creates and removes only unique, tiny test torrents."""
import argparse
import base64
import hashlib
import json
from pathlib import Path
import shutil
import sys
import tempfile
import time
import uuid
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import torrbaby as tb


def bencode(x):
    if isinstance(x, int):
        return b"i" + str(x).encode() + b"e"
    if isinstance(x, bytes):
        return str(len(x)).encode() + b":" + x
    if isinstance(x, str):
        return bencode(x.encode())
    if isinstance(x, list):
        return b"l" + b"".join(bencode(y) for y in x) + b"e"
    return b"d" + b"".join(bencode(k) + bencode(x[k]) for k in sorted(x)) + b"e"


p = argparse.ArgumentParser(description=__doc__)
p.add_argument("--config", required=True)
a = p.parse_args()
c = json.loads(Path(a.config).expanduser().read_text())
rpc = tb.RPC(c)
base = Path(tempfile.mkdtemp(prefix=".torrbaby-smoke-", dir=Path(c["source"]).parent.parent))
added = []
report = []
try:
    test_config = dict(c, state_file=str(base / "state.json"), paused=False)
    for key in ("source", "movies", "tv"):
        root = base / key
        root.mkdir()
        test_config[key] = str(root)
    class LivePlex:
        def scan(self, roots):
            return tb.Plex(c).scan([c[k] for k in ("source", "movies", "tv")])
    s = tb.Service(test_config, rpc, LivePlex())
    for television in (False, True):
        name = "Torrbaby-Smoke-" + uuid.uuid4().hex + (".S01" if television else ".2026.mp4")
        payload = b"Torrbaby isolated integration fixture\n"
        if television:
            root = base / "source" / name
            root.mkdir()
            (root / "Episode.S01E01.mkv").write_bytes(payload)
            (root / "notes.txt").write_bytes(payload)
            data = payload * 2
            info = {"name": name, "piece length": 16384, "pieces": hashlib.sha1(data).digest(), "private": 1,
                    "files": [{"length": len(payload), "path": [n]} for n in ("Episode.S01E01.mkv", "notes.txt")]}
        else:
            (base / "source" / name).write_bytes(payload)
            info = {"name": name, "length": len(payload), "piece length": 16384, "pieces": hashlib.sha1(payload).digest(), "private": 1}
        hash_ = hashlib.sha1(bencode(info)).hexdigest()
        result = rpc.call("torrent_add", metainfo=base64.b64encode(bencode({"info": info})).decode(), download_dir=str(base / "source"), paused=True)
        assert result.get("torrent_added", {}).get("hash_string") == hash_, result
        added.append(hash_)
        rpc.call("torrent_verify", ids=[hash_])
        for _ in range(40):
            t = s.torrent(hash_)
            if tb.complete(t):
                break
            time.sleep(.25)
        assert tb.complete(t), t
        rpc.call("torrent_start", ids=[hash_])
        for _ in range(40):
            s.tick()
            if s.state["jobs"].get(hash_, {}).get("phase") == "done":
                break
            time.sleep(.25)
        job = s.state["jobs"][hash_]
        assert job["phase"] == "done", job
        t = s.torrent(hash_)
        assert t["status"] == 6, t["status"]
        assert not s.state["scans"], s.state
        category = "tv" if television else "movies"
        assert t["download_dir"] == str(base / category)
        renamed = name + "-Renamed" if television else name.replace(".mp4", "-Renamed.mp4")
        s.rename(hash_, name, renamed)
        s.flush_scans()
        assert not s.state["scans"]
        rpc.call("torrent_verify", ids=[hash_])
        for _ in range(40):
            t = s.torrent(hash_)
            if tb.complete(t):
                break
            time.sleep(.25)
        assert tb.complete(t), t
        assert (base / category / renamed).exists()
        report.append({"category": category, "move": "verified", "seeding_after_move": True, "rename": "verified", "hash_recheck": "passed", "plex_scan": "accepted"})
    print(json.dumps(report, indent=2))
finally:
    for hash_ in added:
        rpc.call("torrent_remove", ids=[hash_], delete_local_data=False)
    shutil.rmtree(base)
