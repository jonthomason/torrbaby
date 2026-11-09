"""SQLite persistence for the organizer, device registry and shared playback."""
import json
import math
import os
from pathlib import Path
import sqlite3
import threading
import time
import uuid


class Database:
    def __init__(self, path, legacy_state=None):
        self.path = Path(path)
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.lock = threading.RLock()
        # Create privately before SQLite creates its journal sidecars.
        fd = os.open(str(self.path), os.O_CREAT | os.O_RDWR, 0o600)
        os.close(fd)
        self.conn = sqlite3.connect(str(self.path), timeout=10, check_same_thread=False)
        self.conn.row_factory = sqlite3.Row
        self.conn.execute("PRAGMA journal_mode=WAL")
        self.conn.execute("PRAGMA synchronous=FULL")
        self.conn.execute("PRAGMA foreign_keys=ON")
        self.conn.executescript('''
        CREATE TABLE IF NOT EXISTS organizer_state (id INTEGER PRIMARY KEY CHECK(id=1), document TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS clients (id TEXT PRIMARY KEY, name TEXT NOT NULL, platform TEXT NOT NULL, created REAL NOT NULL, last_seen REAL NOT NULL);
        CREATE TABLE IF NOT EXISTS media (id TEXT PRIMARY KEY, torrent_hash TEXT NOT NULL, file_index INTEGER NOT NULL, name TEXT NOT NULL, length INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS playback (media_id TEXT PRIMARY KEY REFERENCES media(id), position REAL NOT NULL DEFAULT 0, duration REAL NOT NULL DEFAULT 0, watched INTEGER NOT NULL DEFAULT 0, last_played REAL NOT NULL DEFAULT 0, client_id TEXT REFERENCES clients(id), active_session TEXT);
        CREATE TABLE IF NOT EXISTS playback_sessions (id TEXT PRIMARY KEY, media_id TEXT NOT NULL REFERENCES media(id), client_id TEXT NOT NULL REFERENCES clients(id), started REAL NOT NULL, sequence INTEGER NOT NULL DEFAULT -1, ended INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE IF NOT EXISTS transfer_rates (time INTEGER PRIMARY KEY, download REAL NOT NULL, upload REAL NOT NULL);
        CREATE TABLE IF NOT EXISTS metadata (hash TEXT PRIMARY KEY, document TEXT NOT NULL);
        PRAGMA user_version=3;
        ''')
        self.conn.commit()
        self.path.chmod(0o600)
        if not self.conn.execute("SELECT 1 FROM organizer_state WHERE id=1").fetchone():
            if legacy_state and Path(legacy_state).exists():
                state = json.loads(Path(legacy_state).read_text())
            else:
                state = {"jobs": {}, "events": [], "settings": {}, "scans": []}
            self.save_state(state)

    def close(self):
        with self.lock:
            self.conn.close()

    def save_state(self, state):
        with self.lock, self.conn:
            self.conn.execute("INSERT OR REPLACE INTO organizer_state VALUES(1,?)", (json.dumps(state, ensure_ascii=False),))

    def load_state(self):
        with self.lock:
            return json.loads(self.conn.execute("SELECT document FROM organizer_state WHERE id=1").fetchone()[0])

    def register(self, client_id, name, platform):
        try:
            client_id = str(uuid.UUID(client_id))
        except (ValueError, TypeError, AttributeError):
            raise ValueError("client_id must be a UUID") from None
        if not isinstance(name, str) or not name.strip() or len(name) > 80:
            raise ValueError("Client name must contain 1–80 characters")
        if platform not in ("macOS", "iOS", "iPadOS", "tvOS", "CLI", "other"):
            raise ValueError("Unknown client platform")
        now = time.time()
        with self.lock, self.conn:
            self.conn.execute("INSERT INTO clients VALUES(?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET last_seen=excluded.last_seen, platform=excluded.platform", (client_id, name.strip(), platform, now, now))
            return dict(self.conn.execute("SELECT * FROM clients WHERE id=?", (client_id,)).fetchone())

    def clients(self):
        with self.lock:
            return [dict(r) for r in self.conn.execute("SELECT * FROM clients ORDER BY last_seen DESC")]

    def rename_client(self, client_id, name):
        if not isinstance(name, str) or not name.strip() or len(name) > 80:
            raise ValueError("Client name must contain 1–80 characters")
        with self.lock, self.conn:
            cursor = self.conn.execute("UPDATE clients SET name=? WHERE id=?", (name.strip(), client_id))
            if not cursor.rowcount:
                raise ValueError("Client not found")
            return dict(self.conn.execute("SELECT * FROM clients WHERE id=?", (client_id,)).fetchone())

    def begin(self, hash_, index, name, length, client_id):
        media_id = hash_ + ":" + str(index)
        session = str(uuid.uuid4())
        now = time.time()
        with self.lock, self.conn:
            if not self.conn.execute("SELECT 1 FROM clients WHERE id=?", (client_id,)).fetchone():
                raise ValueError("Register the client before starting playback")
            self.conn.execute("INSERT INTO media VALUES(?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET name=excluded.name,length=excluded.length", (media_id, hash_, index, name, length))
            self.conn.execute("INSERT OR IGNORE INTO playback(media_id) VALUES(?)", (media_id,))
            old = self.conn.execute("SELECT * FROM playback WHERE media_id=?", (media_id,)).fetchone()
            resume = 0 if old["watched"] else old["position"]
            self.conn.execute("INSERT INTO playback_sessions(id,media_id,client_id,started) VALUES(?,?,?,?)", (session, media_id, client_id, now))
            self.conn.execute("UPDATE playback SET active_session=?,client_id=?,last_played=? WHERE media_id=?", (session, client_id, now, media_id))
            self.conn.execute("UPDATE clients SET last_seen=? WHERE id=?", (now, client_id))
            # Old sessions remain for rejecting delayed updates, bounded to 90 days.
            self.conn.execute("DELETE FROM playback_sessions WHERE started<? AND id NOT IN (SELECT active_session FROM playback WHERE active_session IS NOT NULL)", (now - 90 * 86400,))
        return {"session_id": session, "media_id": media_id, "resume_position": resume}

    def progress(self, session_id, sequence, position, duration, ended=False):
        if type(sequence) is not int or sequence < 0:
            raise ValueError("sequence must be a nonnegative integer")
        if any(type(x) not in (int, float) or not math.isfinite(x) or x < 0 for x in (position, duration)) or duration == 0 or position > duration + 2:
            raise ValueError("Invalid playback position or duration")
        if type(ended) is not bool:
            raise ValueError("ended must be boolean")
        with self.lock, self.conn:
            session = self.conn.execute("SELECT * FROM playback_sessions WHERE id=?", (session_id,)).fetchone()
            if not session:
                raise ValueError("Playback session not found")
            state = self.conn.execute("SELECT * FROM playback WHERE media_id=?", (session["media_id"],)).fetchone()
            if state["active_session"] != session_id or sequence <= session["sequence"] or session["ended"]:
                return {"accepted": False, "reason": "superseded_or_stale"}
            now = time.time()
            watched = bool(state["watched"]) or position >= duration * .95
            self.conn.execute("UPDATE playback SET position=?,duration=?,watched=?,last_played=?,client_id=? WHERE media_id=?", (min(position, duration), duration, int(watched), now, session["client_id"], session["media_id"]))
            self.conn.execute("UPDATE playback_sessions SET sequence=?,ended=? WHERE id=?", (sequence, int(ended), session_id))
            self.conn.execute("UPDATE clients SET last_seen=? WHERE id=?", (now, session["client_id"]))
        return {"accepted": True, "watched": watched}

    def history(self):
        with self.lock:
            rows = self.conn.execute("SELECT m.*,p.position,p.duration,p.watched,p.last_played,p.client_id,c.name AS client_name FROM playback p JOIN media m ON m.id=p.media_id LEFT JOIN clients c ON c.id=p.client_id ORDER BY p.last_played DESC")
            return [dict(dict(r), watched=bool(r["watched"])) for r in rows]

    def mark(self, hash_, index, name, length, watched):
        if type(watched) is not bool:
            raise ValueError("watched must be boolean")
        media_id = hash_ + ":" + str(index)
        with self.lock, self.conn:
            self.conn.execute("INSERT INTO media VALUES(?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET name=excluded.name,length=excluded.length", (media_id, hash_, index, name, length))
            self.conn.execute("INSERT OR IGNORE INTO playback(media_id) VALUES(?)", (media_id,))
            # Explicit user intent invalidates previous in-flight progress reports.
            self.conn.execute("UPDATE playback SET watched=?,position=0,active_session=NULL WHERE media_id=?", (int(watched), media_id))
        return {"media_id": media_id, "watched": watched}

    def sample_rates(self, download, upload, timestamp=None):
        now = int(time.time() if timestamp is None else timestamp)
        with self.lock, self.conn:
            self.conn.execute("INSERT OR REPLACE INTO transfer_rates VALUES(?,?,?)", (now, max(0, download), max(0, upload)))
            self.conn.execute("DELETE FROM transfer_rates WHERE time<?", (now - 25 * 3600,))

    def rates(self):
        with self.lock:
            return [dict(r) for r in self.conn.execute("SELECT CAST(time/60 AS INTEGER)*60 AS time, AVG(download) AS download, AVG(upload) AS upload FROM transfer_rates WHERE time>=? GROUP BY CAST(time/60 AS INTEGER) ORDER BY time", (int(time.time()) - 86400,))]

    def metadata(self):
        with self.lock:
            return [json.loads(r[0]) for r in self.conn.execute("SELECT document FROM metadata")]

    def save_metadata(self, value):
        with self.lock, self.conn:
            old = self.conn.execute("SELECT document FROM metadata WHERE hash=?", (value['hash'],)).fetchone()
            if old and json.loads(old[0]).get('confirmed') and not value.get('confirmed'):
                return
            self.conn.execute("INSERT OR REPLACE INTO metadata VALUES(?,?)", (value['hash'], json.dumps(value, ensure_ascii=False)))

    def refresh_metadata(self, hash_):
        with self.lock, self.conn:
            self.conn.execute("DELETE FROM metadata WHERE hash=?", (hash_,))
