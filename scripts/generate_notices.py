#!/usr/bin/env python3
"""Build-time license notices; not required by the deployed Rust server."""
import json
import pathlib
import subprocess
import sys

root = pathlib.Path(__file__).resolve().parents[1]
metadata = json.loads(subprocess.check_output([
    'cargo', 'metadata', '--locked', '--offline', '--format-version', '1',
    '--filter-platform', 'aarch64-apple-darwin', '--manifest-path', str(root / 'Cargo.toml'),
]))
parts = ['Torrbaby Server — third-party notices\n\nTorrbaby source code is MIT licensed. Dependencies retain their respective licenses.\n']
for package in sorted(metadata['packages'], key=lambda p: (p['name'], p['version'])):
    if pathlib.Path(package['manifest_path']).resolve() == root / 'Cargo.toml':
        continue
    folder = pathlib.Path(package['manifest_path']).parent
    parts.append(f"\n{'=' * 72}\n{package['name']} {package['version']}\nLicense: {package.get('license') or 'See license file'}\n{package.get('repository') or ''}\n")
    licenses = sorted(p for p in folder.iterdir() if p.is_file() and p.name.upper().startswith(('LICENSE', 'LICENCE', 'COPYING', 'NOTICE')))
    if package.get('license_file'):
        licenses.append(folder / package['license_file'])
    # A package published without its license file: the reviewed text kept in scripts/licenses.
    reviewed = root / 'scripts' / 'licenses' / f"{package['name']}.txt"
    if not licenses and reviewed.is_file():
        licenses.append(reviewed)
    if not licenses:
        raise SystemExit(f"Review missing license text: {package['name']}")
    for path in dict.fromkeys(licenses):
        parts.append(f'\n--- {path.name} ---\n{path.read_text(errors="replace")}\n')
parts.append(f"\n{'=' * 72}\nFFmpeg (bundled as the separate programs torrbaby-ffmpeg and torrbaby-ffprobe)\nLicense: LGPL-2.1-or-later; built without GPL or non-free components.\nhttps://ffmpeg.org/\nThe exact source release and configure options are in scripts/build_ffmpeg.sh; the license text is FFMPEG_LICENSE.txt beside this file.\n")
pathlib.Path(sys.argv[1]).write_text(''.join(parts))
