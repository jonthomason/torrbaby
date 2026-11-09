#!/usr/bin/env python3
"""Rescan missing subtitles without deleting artifacts or interrupting active work.
Run on the server; defaults to a dry run. No media files are changed.
"""
import argparse
import collections
import datetime
import json
import pathlib
import sqlite3
import time


def revision(path):
    m = pathlib.Path(path).stat()
    return f'{m.st_dev}:{m.st_ino}:{m.st_size}:{m.st_mtime_ns // 1000000000}:{m.st_mtime_ns % 1000000000}:{m.st_ctime_ns // 1000000000}:{m.st_ctime_ns % 1000000000}'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--data-dir', type=pathlib.Path, default=pathlib.Path.home() / 'Library/Application Support/Torrbaby')
    parser.add_argument('--apply', action='store_true')
    args = parser.parse_args()
    root = args.data_dir
    db = sqlite3.connect('file:' + str(root / 'state.sqlite3') + '?mode=rw', uri=True, timeout=15)
    counts = collections.Counter()
    if args.apply:
        backup = root / 'backups' / ('subtitle-rescan-' + datetime.datetime.now().strftime('%Y%m%d-%H%M%S') + '.sqlite3')
        backup.parent.mkdir(exist_ok=True)
        with sqlite3.connect(backup) as dest:
            db.backup(dest)
        print('Database backup:', backup.name)
    db.execute('BEGIN IMMEDIATE' if args.apply else 'BEGIN')
    try:
        jobs = collections.defaultdict(list)
        for path in (root / 'subtitles').glob('*.json'):
            try:
                j = json.loads(path.read_text())
                jobs[j['asset_id']].append(j)
            except (OSError, ValueError, KeyError):
                continue
        assets = [json.loads(r[0]) for r in db.execute('SELECT document FROM library_assets')]
        for asset in assets:
            if not asset.get('available') or not asset.get('complete'):
                counts['incomplete_or_unavailable'] += 1
                continue
            if pathlib.Path(asset.get('path', '')).suffix.lower() not in {'.mkv', '.webm', '.mp4', '.m4v', '.mov'}:
                continue
            asset_jobs = jobs[asset['id']]
            if any(j.get('state') in {'queued', 'running'} for j in asset_jobs):
                counts['active_preserved'] += 1
                continue
            translated = False
            for j in asset_jobs:
                if j.get('state') != 'ready' or not j.get('target'):
                    continue
                try:
                    translated = j.get('revision') == revision(j['input']) and (root / 'subtitles' / (j['id'] + '.vtt')).is_file()
                except (OSError, KeyError):
                    continue
                if translated:
                    break
            if translated:
                counts['completed_translations_preserved'] += 1
                continue
            try:
                rev = revision(asset['path'])
            except OSError:
                counts['unreadable'] += 1
                continue
            row = db.execute('SELECT document FROM media_inventory WHERE asset_id=?', (asset['id'],)).fetchone()
            info = json.loads(row[0]) if row else {}
            info.update(state='pending', preparation_state='pending', retry_after=0, jobs=[])
            info.pop('error', None)
            counts['rescan_and_retry'] += 1
            counts['mkv_rescan' if pathlib.Path(asset['path']).suffix.lower() == '.mkv' else 'other_container_rescan'] += 1
            if args.apply:
                db.execute('INSERT INTO media_inventory(asset_id,revision,document,updated) VALUES(?,?,?,?) ON CONFLICT(asset_id) DO UPDATE SET revision=excluded.revision,document=excluded.document,updated=excluded.updated', (asset['id'], rev, json.dumps(info), time.time()))
        db.commit() if args.apply else db.rollback()
    except BaseException:
        db.rollback()
        raise
    print(json.dumps({'applied': args.apply, 'counts': dict(counts)}, indent=2))


if __name__ == '__main__':
    main()
