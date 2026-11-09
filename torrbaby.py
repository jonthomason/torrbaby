#!/usr/bin/env python3
"""Torrbaby: a dependency-free Transmission organizer and private control API."""
import argparse
import base64
import fcntl
import hmac
import hashlib
import json
import logging
from logging.handlers import RotatingFileHandler
import os
import mimetypes
from pathlib import Path, PurePosixPath
import plistlib
import re
import signal
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
import xml.etree.ElementTree as ET
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

VERSION = "0.1.0"
TV = re.compile(r"(?<![a-z0-9])(?:s\d{1,2}(?:[ ._-]*e\d{1,3})?(?!\d|[a-df-z])|season[ ._-]*\d{1,2}\b|\d{1,2}x\d{2}\b)", re.I)
VIDEO = {".mkv", ".mp4", ".m4v", ".avi", ".mov", ".mpg", ".mpeg", ".ts", ".m2ts", ".wmv", ".webm"}
FIELDS = ["hash_string", "name", "status", "error", "error_string", "percent_done", "metadata_percent_complete", "left_until_done", "download_dir", "files", "file_stats", "rate_download", "rate_upload", "eta", "upload_ratio", "added_date", "activity_date", "peers_connected", "peers_sending_to_us", "downloaded_ever", "uploaded_ever", "labels", "is_stalled"]
LOG = logging.getLogger("torrbaby")


def expanded(value):
    return Path(value).expanduser().absolute()


def atomic_json(path, data):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(path.name + ".tmp")
    fd = os.open(str(tmp), os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as f:
        json.dump(data, f, indent=2, ensure_ascii=False)
        f.flush()
        os.fsync(f.fileno())
    os.replace(str(tmp), str(path))


def checked_path(root, relative):
    p = PurePosixPath(relative)
    if not relative or p.is_absolute() or any(x in ("..", ".") for x in relative.split("/")) or "\x00" in relative:
        raise ValueError("Unsafe torrent path")
    root = Path(root)
    if root.is_symlink() or root.resolve() != root.absolute():
        raise ValueError("Symlinked library root")
    result = root.joinpath(*p.parts)
    for parent in [result] + list(result.parents):
        if parent == root:
            break
        if parent.is_symlink():
            raise ValueError("Symlink in torrent path")
    return result


def classify(torrent):
    names = [torrent["name"]] + [f["name"] for f in torrent["files"]]
    if not any(Path(f["name"]).suffix.lower() in VIDEO for f in torrent["files"]):
        return None
    return "tv" if any(TV.search(n) for n in names) else "movies"


def complete(t):
    # Require the whole payload, not merely the wanted subset or magnet metadata.
    return (t["metadata_percent_complete"] == 1 and t["percent_done"] == 1
            and t["left_until_done"] == 0 and t["error"] == 0
            and t["status"] in (0, 5, 6) and bool(t["files"])
            and all(f["bytes_completed"] == f["length"] for f in t["files"]))


def validate_config(c):
    for key in ("source", "movies", "tv"):
        if not Path(c[key]).is_absolute():
            raise ValueError(key + " must be absolute")
    paths = [Path(c[k]) for k in ("source", "movies", "tv")]
    for a in paths:
        for b in paths:
            if a != b and (a in b.parents or b in a.parents):
                raise ValueError("Library roots must not contain one another")
    if len(set(paths)) != 3:
        raise ValueError("Source and destination roots must differ")
    if type(c["poll_seconds"]) not in (int, float) or not 5 <= c["poll_seconds"] <= 3600:
        raise ValueError("poll_seconds must be between 5 and 3600")
    if type(c["paused"]) is not bool:
        raise ValueError("paused must be boolean")


class RPC:
    def __init__(self, config):
        self.url = config["transmission_url"]
        self.headers = {"Content-Type": "application/json"}
        auth = config.get("transmission_auth_file")
        if auth:
            value = expanded(auth).read_text().strip().encode()
            self.headers["Authorization"] = "Basic " + base64.b64encode(value).decode()
        self.lock = threading.Lock()

    def call(self, method, **params):
        data = json.dumps(dict(jsonrpc="2.0", method=method, params=params, id=1)).encode()
        with self.lock:
            for _ in range(2):
                try:
                    with urllib.request.urlopen(urllib.request.Request(self.url, data, self.headers), timeout=20) as r:
                        result = json.load(r)
                    if "error" in result:
                        raise RuntimeError("Transmission: " + str(result["error"]))
                    return result["result"]
                except urllib.error.HTTPError as e:
                    if e.code != 409:
                        raise RuntimeError("Transmission HTTP " + str(e.code)) from None
                    self.headers["X-Transmission-Session-Id"] = e.headers["X-Transmission-Session-Id"]
        raise RuntimeError("Transmission session negotiation failed")

    def torrents(self, hashes=None):
        params = {"fields": FIELDS}
        if hashes is not None:
            if not hashes:
                raise ValueError("Explicit torrent hash required")
            params["ids"] = hashes
        return self.call("torrent_get", **params)["torrents"]


class Plex:
    def __init__(self, config):
        self.config = config

    def request(self, path):
        c = self.config
        if c.get("plex_token_file"):
            token = expanded(c["plex_token_file"]).read_text().strip()
        else:
            with expanded(c["plex_preferences"]).open("rb") as f:
                token = plistlib.load(f)["PlexOnlineToken"]
        req = urllib.request.Request(c["plex_url"].rstrip("/") + path, headers={"X-Plex-Token": token})
        try:
            with urllib.request.urlopen(req, timeout=20) as r:
                return r.read()
        except urllib.error.HTTPError as e:
            raise RuntimeError("Plex HTTP " + str(e.code)) from None

    def scan(self, roots):
        sections = ET.fromstring(self.request("/library/sections"))
        ids = set()
        for root in roots:
            matched = False
            for section in sections.findall("Directory"):
                for loc in section.findall("Location"):
                    if Path(loc.attrib["path"]) == Path(root):
                        ids.add(section.attrib["key"])
                        matched = True
            if not matched:
                raise RuntimeError("No Plex library configured for " + root)
        for key in sorted(ids):
            self.request("/library/sections/" + urllib.parse.quote(key, safe="") + "/refresh")
        return sorted(ids)


class Service:
    def __init__(self, config, rpc=None, plex=None):
        validate_config(config)
        self.config = config.copy()
        self.path = expanded(config["state_file"])
        self.state = json.loads(self.path.read_text()) if self.path.exists() else {"jobs": {}, "events": [], "settings": {}, "scans": []}
        self.config.update(self.state.get("settings", {}))
        validate_config(self.config)
        self.rpc = rpc or RPC(config)
        self.plex = plex or Plex(config)
        self.lock = threading.RLock()
        self.wake = threading.Event()
        self.stop = threading.Event()
        self.last_poll = None
        self.last_error = None
        self.started = time.time()

    def save(self):
        atomic_json(self.path, self.state)

    def event(self, kind, **data):
        item = dict(time=time.time(), kind=kind, **data)
        self.state["events"].append(item)
        self.state["events"] = self.state["events"][-1000:]
        LOG.info("%s", json.dumps(item, ensure_ascii=False))
        self.save()

    def torrent(self, hash_):
        if not re.fullmatch(r"[0-9a-fA-F]{40}", hash_):
            raise ValueError("Use a full 40-character torrent hash")
        items = self.rpc.torrents([hash_])
        if len(items) != 1:
            raise ValueError("Torrent not found")
        return items[0]

    def update_settings(self, patch):
        allowed = {"paused", "poll_seconds", "source", "movies", "tv"}
        if not patch or set(patch) - allowed:
            raise ValueError("Allowed settings: " + ", ".join(sorted(allowed)))
        c = dict(self.config, **patch)
        validate_config(c)
        for key in ("source", "movies", "tv"):
            if key in patch and not Path(patch[key]).is_dir():
                raise ValueError("Directory does not exist: " + patch[key])
        self.config = c
        self.state["settings"].update(patch)
        self.event("settings_changed", settings=patch)
        self.wake.set()
        return self.settings()

    def settings(self):
        return {k: self.config[k] for k in ("paused", "poll_seconds", "source", "movies", "tv")}

    def queue_scan(self, roots, reason):
        self.state["scans"].append({"roots": roots, "reason": reason, "created": time.time()})
        self.save()

    def flush_scans(self):
        if not self.state["scans"]:
            return
        roots = sorted({r for s in self.state["scans"] for r in s["roots"]})
        try:
            ids = self.plex.scan(roots)
        except Exception as e:
            self.state["scan_error"] = str(e)
            self.save()
            return
        self.state["scans"] = []
        self.state["scan_error"] = None
        self.event("plex_scan_requested", sections=ids)

    def plan(self, t):
        if Path(t["download_dir"]) != Path(self.config["source"]) or not complete(t):
            return None
        kind = classify(t)
        if kind is None:
            return None
        source, dest = Path(t["download_dir"]), Path(self.config[kind])
        if not source.is_dir() or not dest.is_dir():
            raise ValueError("Library volume/directory is unavailable")
        tops = sorted({PurePosixPath(f["name"]).parts[0] for f in t["files"]})
        for f in t["files"]:
            p = checked_path(source, f["name"])
            checked_path(dest, f["name"])
            if not p.is_file() or p.stat().st_size != f["length"]:
                raise ValueError("Source file missing or size differs: " + f["name"])
        for top in tops:
            if os.path.lexists(str(checked_path(dest, top))):
                raise ValueError("Destination already exists: " + top)
        return {"hash": t["hash_string"], "name": t["name"], "category": kind,
                "source": str(source), "destination": str(dest), "files": t["files"],
                "phase": "moving", "created": time.time(), "attempted": 0}

    def moved(self, job, t):
        if Path(t["download_dir"]) != Path(job["destination"]) or not complete(t):
            return False
        for f in job["files"]:
            target = checked_path(job["destination"], f["name"])
            if not target.is_file() or target.stat().st_size != f["length"]:
                return False
            if checked_path(job["source"], f["name"]).exists():
                return False
        return True

    def advance(self, job, t):
        if self.moved(job, t):
            # Persist the scan obligation and completion together before any scan call.
            job.update(phase="done", finished=time.time(), error=None)
            self.queue_scan([job["source"], job["destination"]], job["hash"])
            self.event("move_verified", hash=job["hash"], destination=job["destination"])
            return
        if time.time() - job["attempted"] < 300:
            return
        if Path(t["download_dir"]) != Path(job["source"]):
            raise ValueError("Move awaiting verification; torrent location changed")
        # Never resend an ambiguous move into existing data.
        for f in job["files"]:
            source = checked_path(job["source"], f["name"])
            if not source.is_file() or source.stat().st_size != f["length"]:
                raise ValueError("Move awaiting verification; source changed")
            top = PurePosixPath(f["name"]).parts[0]
            if os.path.lexists(str(checked_path(job["destination"], top))):
                raise ValueError("Move awaiting verification; destination exists")
        if not complete(t):
            raise ValueError("Torrent no longer complete; move deferred")
        job["attempted"] = time.time()
        self.event("move_requested", hash=job["hash"], destination=job["destination"])
        self.rpc.call("torrent_set_location", ids=[job["hash"]], location=job["destination"], move=True)

    def tick(self):
        with self.lock:
            if self.config["paused"]:
                return
            try:
                torrents = self.rpc.torrents()
                self.last_poll = time.time()
                self.last_error = None
                for t in torrents:
                    h = t["hash_string"]
                    job = self.state["jobs"].get(h)
                    if job and job["phase"] in ("done", "ignored"):
                        continue
                    try:
                        if not job or job["phase"] == "blocked":
                            plan = self.plan(t)
                            if plan is None:
                                continue
                            # Avoid moving files that another torrent also references.
                            own = {str(checked_path(plan["source"], f["name"])) for f in t["files"]}
                            others = {str(checked_path(o["download_dir"], f["name"])) for o in torrents if o["hash_string"] != h for f in o["files"]}
                            if own & others:
                                raise ValueError("Source files are shared by another torrent")
                            job = self.state["jobs"][h] = plan
                            self.save()
                        self.advance(job, t)
                    except Exception as e:
                        if not job:
                            job = self.state["jobs"][h] = {"hash": h, "name": t["name"], "phase": "blocked"}
                        if job.get("error") != str(e):
                            job["error"] = str(e)
                            self.event("job_error", hash=h, error=str(e))
                self.flush_scans()
            except Exception as e:
                self.last_error = str(e)
                LOG.exception("Reconciliation failed")

    def run(self):
        while not self.stop.is_set():
            self.wake.clear()
            self.tick()
            self.wake.wait(self.config["poll_seconds"])

    def status(self):
        return {"version": VERSION, "uptime_seconds": int(time.time() - self.started),
                "settings": self.settings(), "last_poll": self.last_poll, "last_error": self.last_error,
                "pending_scans": len(self.state["scans"]), "scan_error": self.state.get("scan_error"),
                "jobs": self.state["jobs"]}

    def rename(self, hash_, path, name):
        t = self.torrent(hash_)
        if not complete(t):
            raise ValueError("Rename requires a complete torrent")
        if Path(t["download_dir"]) not in [Path(self.config[k]) for k in ("source", "movies", "tv")]:
            raise ValueError("Torrent is outside managed library roots")
        job = self.state["jobs"].get(hash_)
        if job and job["phase"] == "moving":
            raise ValueError("Wait for the current move to finish")
        if not name or name in (".", "..") or any(c in name for c in "/\\\x00"):
            raise ValueError("New name must be a single path component")
        root = Path(t["download_dir"])
        old = checked_path(root, path)
        if not any(f["name"] == path or f["name"].startswith(path + "/") for f in t["files"]):
            raise ValueError("Path is not owned by this torrent")
        if not old.exists():
            raise ValueError("Path does not exist")
        if old.is_file() and old.suffix.lower() != Path(name).suffix.lower():
            raise ValueError("Keep the file extension")
        new_rel = str(PurePosixPath(path).parent / name)
        new = checked_path(root, new_rel)
        if os.path.lexists(str(new)):
            raise ValueError("Rename destination already exists")
        for other in self.rpc.torrents():
            if other["hash_string"] == hash_:
                continue
            for f in other["files"]:
                p = checked_path(other["download_dir"], f["name"])
                if p == old or old in p.parents:
                    raise ValueError("Path is shared by another torrent")
        self.event("rename_requested", hash=hash_, path=path, name=name)
        # Queue scan before the request: even an ambiguous timeout will be reconciled.
        self.queue_scan([str(root)], "rename:" + hash_)
        self.rpc.call("torrent_rename_path", ids=[hash_], path=path, name=name)
        refreshed = self.torrent(hash_)
        if not new.exists() or old.exists() or not any(f["name"] == new_rel or f["name"].startswith(new_rel + "/") for f in refreshed["files"]):
            raise RuntimeError("Rename requested but verification failed; inspect torrent before retrying")
        self.event("rename_verified", hash=hash_, path=path, name=name)
        self.wake.set()
        return refreshed

    def action(self, hash_, action, body):
        t = self.torrent(hash_)
        methods = {"start": "torrent_start", "stop": "torrent_stop", "verify": "torrent_verify", "reannounce": "torrent_reannounce"}
        if action in methods:
            self.event("torrent_" + action, hash=hash_)
            return self.rpc.call(methods[action], ids=[hash_])
        if action == "remove":
            delete = body.get("delete_data", False)
            if type(delete) is not bool or (delete and body.get("confirm_hash") != hash_):
                raise ValueError("Deleting data requires delete_data=true and confirm_hash matching the hash")
            if delete:
                root = Path(t["download_dir"])
                if root not in [Path(self.config[k]) for k in ("source", "movies", "tv")]:
                    raise ValueError("Data deletion is restricted to managed roots")
                owned = {str(checked_path(root, f["name"])) for f in t["files"]}
                for other in self.rpc.torrents():
                    if other["hash_string"] != hash_ and owned & {str(checked_path(other["download_dir"], f["name"])) for f in other["files"]}:
                        raise ValueError("Cannot delete shared torrent files")
                self.queue_scan([str(root)], "remove:" + hash_)
            self.event("torrent_remove", hash=hash_, delete_data=delete)
            result = self.rpc.call("torrent_remove", ids=[hash_], delete_local_data=delete)
            if hash_ in self.state["jobs"]:
                self.state["jobs"][hash_]["phase"] = "ignored"
                self.save()
            self.wake.set()
            return result
        if action == "rename":
            return self.rename(hash_, body["path"], body["name"])
        if action in ("retry", "ignore"):
            job = self.state["jobs"].get(hash_)
            if action == "ignore":
                if job and job["phase"] == "moving":
                    raise ValueError("Cannot cancel an in-flight Transmission move")
                self.state["jobs"][hash_] = {"hash": hash_, "name": t["name"], "phase": "ignored"}
            elif job and job["phase"] == "moving":
                job["attempted"] = 0
                job.pop("error", None)
            else:
                self.state["jobs"].pop(hash_, None)
            self.event("job_" + action, hash=hash_)
            self.wake.set()
            return {"accepted": True}
        raise ValueError("Unknown action")

    def add(self, body):
        if set(body) - {"magnet", "metainfo", "paused"}:
            raise ValueError("Allowed fields: magnet, metainfo, paused")
        if bool(body.get("magnet")) == bool(body.get("metainfo")):
            raise ValueError("Provide either a magnet link or base64 torrent metainfo")
        params = {"download_dir": self.config["source"], "paused": body.get("paused", False)}
        if type(params["paused"]) is not bool:
            raise ValueError("paused must be boolean")
        if body.get("magnet"):
            if not isinstance(body["magnet"], str) or not body["magnet"].startswith("magnet:?"):
                raise ValueError("Expected a magnet link")
            params["filename"] = body["magnet"]
        else:
            base64.b64decode(body["metainfo"], validate=True)
            params["metainfo"] = body["metainfo"]
        result = self.rpc.call("torrent_add", **params)
        self.event("torrent_added", result=result)
        self.wake.set()
        return result

    def media_file(self, hash_, index):
        t = self.torrent(hash_)
        if type(index) is not int or not 0 <= index < len(t["files"]):
            raise ValueError("Invalid file index")
        f = t["files"][index]
        if f["bytes_completed"] != f["length"] or not f["length"] or t["metadata_percent_complete"] != 1:
            raise ValueError("This file is not completely downloaded")
        if Path(t["download_dir"]) not in [Path(self.config[k]) for k in ("source", "movies", "tv")]:
            raise ValueError("Streaming is limited to managed roots")
        if Path(f["name"]).suffix.lower() not in VIDEO:
            raise ValueError("This is not a recognized video file")
        path = checked_path(t["download_dir"], f["name"])
        if not path.is_file() or path.stat().st_size != f["length"]:
            raise ValueError("File is unavailable or has changed")
        try:
            with path.open("rb"):
                pass
        except PermissionError:
            raise ValueError("Torrbaby cannot read this volume. On the server, allow its Python process access to removable volumes in macOS Privacy & Security settings.") from None
        return path


def lookup_names(t):
    """Opt-in online suggestions, never executed by the sorter."""
    kind = classify(t)
    name = t["name"]
    if Path(name).suffix.lower() in VIDEO:
        name = str(Path(name).with_suffix(""))
    marker = TV.search(name)
    year_match = re.search(r"(?<!\d)((?:19|20)\d{2})(?!\d)", name)
    cut = marker.start() if marker else year_match.start() if year_match else len(name)
    query = re.sub(r"[._]+", " ", name[:cut]).strip(" -()[]")
    if not query or kind is None:
        return {"query": query, "candidates": [], "note": "No reliable title query"}
    if kind == "tv":
        url = "https://api.tvmaze.com/search/shows?" + urllib.parse.urlencode({"q": query})
    else:
        url = "https://itunes.apple.com/search?" + urllib.parse.urlencode({"term": query, "entity": "movie", "country": "US", "limit": 8})
    with urllib.request.urlopen(urllib.request.Request(url, headers={"User-Agent": "Torrbaby/" + VERSION}), timeout=15) as r:
        data = json.load(r)
    candidates = []
    for item in (data[:8] if kind == "tv" else data.get("results", [])):
        show = item["show"] if kind == "tv" else item
        title = show.get("name") if kind == "tv" else show.get("trackName")
        year = (show.get("premiered") or "")[:4] if kind == "tv" else (show.get("releaseDate") or "")[:4]
        source_url = show.get("url") if kind == "tv" else show.get("trackViewUrl")
        if not title:
            continue
        safe_title = re.sub(r'[/\\:\x00]', " - ", title).strip()
        suffix = (" - " + marker.group().upper().replace(".", "").replace(" ", "")) if marker else ""
        proposed = safe_title + (" (" + year + ")" if year else "") + suffix
        roots = sorted({PurePosixPath(f["name"]).parts[0] for f in t["files"]})
        if len(roots) == 1 and len(t["files"]) == 1 and roots[0] == t["files"][0]["name"]:
            proposed += Path(roots[0]).suffix
        candidates.append({"title": title, "year": year, "source_url": source_url,
                           "proposed_root_name": proposed if len(roots) == 1 else None,
                           "path": roots[0] if len(roots) == 1 else None})
    return {"query": query, "provider": "TVmaze (CC BY-SA)" if kind == "tv" else "Apple iTunes Search",
            "candidates": candidates, "automatic": False,
            "note": "Review title, year and episode range; renaming a root does not rename its children."}


class Handler(BaseHTTPRequestHandler):
    server_version = "Torrbaby/" + VERSION

    def log_message(self, format_, *args):
        # Do not log Authorization headers or request bodies.
        LOG.info("http %s %s %s", self.client_address[0], self.command, urllib.parse.urlsplit(self.path).path)

    def reply(self, code, payload):
        data = json.dumps(payload, ensure_ascii=False).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if urllib.parse.urlsplit(self.path).path.startswith("/v1/stream/"):
            return self.stream(False)
        self.dispatch("GET")

    def do_HEAD(self):
        if urllib.parse.urlsplit(self.path).path.startswith("/v1/stream/"):
            return self.stream(True)
        self.reply(404, {"error": "Not found"})

    def stream(self, head):
        try:
            url = urllib.parse.urlsplit(self.path)
            parts = url.path.strip("/").split("/")
            if len(parts) != 4:
                raise ValueError("Invalid stream path")
            query = urllib.parse.parse_qs(url.query)
            expires = int(query.get("expires", ["0"])[0])
            sig = query.get("sig", [""])[0]
            expected = stream_signature(self.server.token, parts[2], int(parts[3]), expires)
            if expires < time.time() or not hmac.compare_digest(sig.encode(), expected.encode()):
                return self.reply(401, {"error": "Invalid or expired playback URL"})
            with self.server.service.lock:
                path = self.server.service.media_file(parts[2], int(parts[3]))
                file = path.open("rb")
            with file:
                size = os.fstat(file.fileno()).st_size
                try:
                    start, end = byte_range(self.headers.get("Range"), size)
                except ValueError:
                    self.send_response(416)
                    self.send_header("Content-Range", "bytes */%s" % size)
                    self.send_header("Content-Length", "0")
                    self.end_headers()
                    return
                partial = bool(self.headers.get("Range"))
                self.send_response(206 if partial else 200)
                self.send_header("Content-Type", mimetypes.guess_type(str(path))[0] or "application/octet-stream")
                self.send_header("Accept-Ranges", "bytes")
                self.send_header("Content-Length", str(end - start + 1))
                self.send_header("Cache-Control", "private, no-store")
                if partial:
                    self.send_header("Content-Range", "bytes %s-%s/%s" % (start, end, size))
                self.end_headers()
                if not head:
                    file.seek(start)
                    remaining = end - start + 1
                    while remaining:
                        chunk = file.read(min(1024 * 1024, remaining))
                        if not chunk:
                            break
                        self.wfile.write(chunk)
                        remaining -= len(chunk)
        except (BrokenPipeError, ConnectionResetError, TimeoutError):
            pass
        except (ValueError, KeyError) as e:
            self.reply(400, {"error": str(e)})
        except Exception:
            LOG.exception("Streaming failed")
            self.reply(502, {"error": "Media unavailable"})

    def do_POST(self):
        self.dispatch("POST")

    def do_PATCH(self):
        self.dispatch("PATCH")

    def dispatch(self, method):
        if not hmac.compare_digest(self.headers.get("Authorization", "").encode(), ("Bearer " + self.server.token).encode()):
            return self.reply(401, {"error": "Unauthorized"})
        if self.headers.get("Origin"):
            return self.reply(403, {"error": "Browser cross-origin requests are disabled"})
        try:
            length = int(self.headers.get("Content-Length", "0"))
            if not 0 <= length <= 65536:
                return self.reply(413, {"error": "Request too large"})
            self.connection.settimeout(10)
            body = json.loads(self.rfile.read(length)) if length else {}
            if not isinstance(body, dict):
                raise ValueError("JSON body must be an object")
            parts = urllib.parse.urlsplit(self.path).path.strip("/").split("/")
            s = self.server.service
            with s.lock:
                if method == "GET" and parts == ["v1", "status"]:
                    result = s.status()
                elif method == "GET" and parts == ["v1", "settings"]:
                    result = s.settings()
                elif method == "PATCH" and parts == ["v1", "settings"]:
                    result = s.update_settings(body)
                elif method == "GET" and parts == ["v1", "events"]:
                    result = s.state["events"]
                elif method == "GET" and parts == ["v1", "torrents"]:
                    result = s.rpc.torrents()
                elif method == "POST" and parts == ["v1", "torrents"]:
                    result = s.add(body)
                elif method == "POST" and parts == ["v1", "reconcile"]:
                    s.wake.set()
                    result = {"accepted": True}
                elif method == "POST" and parts == ["v1", "plex", "scan"]:
                    s.queue_scan([s.config[k] for k in ("source", "movies", "tv")], "manual")
                    s.flush_scans()
                    result = {"pending_scans": len(s.state["scans"]), "error": s.state.get("scan_error")}
                elif len(parts) in (3, 4) and parts[:2] == ["v1", "torrents"]:
                    if method == "GET" and len(parts) == 3:
                        result = s.torrent(parts[2])
                    elif method == "POST" and len(parts) == 4 and parts[3] == "lookup":
                        result = lookup_names(s.torrent(parts[2]))
                    elif method == "POST" and len(parts) == 4 and parts[3] == "play":
                        index = body.get("file_index")
                        path = s.media_file(parts[2], index)
                        expires = int(time.time()) + 21600
                        sig = stream_signature(self.server.token, parts[2], index, expires)
                        result = {"path": "/v1/stream/%s/%s?expires=%s&sig=%s" % (parts[2], index, expires, sig),
                                  "expires": expires, "name": path.name}
                    elif method == "POST" and len(parts) == 4:
                        result = s.action(parts[2], parts[3], body)
                    else:
                        return self.reply(404, {"error": "Not found"})
                else:
                    return self.reply(404, {"error": "Not found"})
            self.reply(200, result)
        except (ValueError, KeyError, TypeError) as e:
            self.reply(400, {"error": str(e)})
        except Exception as e:
            LOG.exception("API request failed")
            self.reply(502, {"error": str(e)})


class Server(ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True

    def get_request(self):
        sock, addr = super().get_request()
        sock.settimeout(10)
        return sock, addr


def stream_signature(token, hash_, index, expires):
    message = "%s:%s:%s" % (hash_, index, expires)
    return hmac.new(token.encode(), message.encode(), hashlib.sha256).hexdigest()


def byte_range(header, size):
    if size <= 0:
        raise ValueError("Empty media")
    if not header:
        return 0, size - 1
    match = re.fullmatch(r"bytes=(\d*)-(\d*)", header.strip())
    if not match or not any(match.groups()):
        raise ValueError("Unsupported byte range")
    first, last = match.groups()
    if first:
        start = int(first)
        end = min(int(last), size - 1) if last else size - 1
    else:
        start, end = max(0, size - int(last)), size - 1
    if start > end or start >= size:
        raise ValueError("Unsatisfiable byte range")
    return start, end


def serve(config_path):
    config = json.loads(expanded(config_path).read_text())
    token_path = expanded(config["token_file"])
    token = token_path.read_text().strip()
    if len(token) < 32:
        raise ValueError("API token must be at least 32 characters")
    state_path = expanded(config["state_file"])
    state_path.parent.mkdir(parents=True, exist_ok=True)
    with state_path.with_suffix(".lock").open("w") as lockfile:
        fcntl.flock(lockfile, fcntl.LOCK_EX | fcntl.LOCK_NB)
        handler = RotatingFileHandler(state_path.parent / "torrbaby.log", maxBytes=2_000_000, backupCount=3)
        logging.basicConfig(level=logging.INFO, handlers=[handler], format="%(asctime)s %(levelname)s %(message)s")
        service = Service(config)
        server = Server((config["listen_host"], config["listen_port"]), Handler)
        server.token, server.service = token, service
        worker = threading.Thread(target=service.run, daemon=True)
        worker.start()
        def shutdown(*_):
            service.stop.set()
            service.wake.set()
            threading.Thread(target=server.shutdown, daemon=True).start()
        signal.signal(signal.SIGTERM, shutdown)
        signal.signal(signal.SIGINT, shutdown)
        LOG.info("Torrbaby %s listening on %s:%s", VERSION, config["listen_host"], config["listen_port"])
        try:
            server.serve_forever(poll_interval=0.5)
        finally:
            shutdown()
            worker.join(timeout=25)
            server.server_close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    subs = parser.add_subparsers(dest="command", required=True)
    p = subs.add_parser("serve")
    p.add_argument("--config", required=True)
    p = subs.add_parser("request", help="Call the same authenticated API used by apps")
    p.add_argument("method", choices=["GET", "POST", "PATCH"])
    p.add_argument("path")
    p.add_argument("--data", default="{}", help="JSON body")
    p.add_argument("--url", default=os.environ.get("TORRBABY_URL", "http://127.0.0.1:8765"))
    p.add_argument("--token-file", default=os.environ.get("TORRBABY_TOKEN_FILE", "~/.config/torrbaby/api.token"))
    args = parser.parse_args()
    if args.command == "serve":
        serve(args.config)
    else:
        token = expanded(args.token_file).read_text().strip()
        headers = {"Authorization": "Bearer " + token, "Content-Type": "application/json"}
        data = None if args.method == "GET" else json.dumps(json.loads(args.data)).encode()
        req = urllib.request.Request(args.url.rstrip("/") + "/" + args.path.lstrip("/"), data=data, headers=headers, method=args.method)
        try:
            with urllib.request.urlopen(req, timeout=90) as r:
                print(json.dumps(json.load(r), indent=2, ensure_ascii=False))
        except urllib.error.HTTPError as e:
            print(e.read().decode())
            raise SystemExit(1)


if __name__ == "__main__":
    main()
