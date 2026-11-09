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
    if package.get('source') is None:
        continue
    folder = pathlib.Path(package['manifest_path']).parent
    parts.append(f"\n{'=' * 72}\n{package['name']} {package['version']}\nLicense: {package.get('license') or 'See license file'}\n{package.get('repository') or ''}\n")
    licenses = sorted(p for p in folder.iterdir() if p.is_file() and p.name.upper().startswith(('LICENSE', 'LICENCE', 'COPYING', 'NOTICE')))
    if package.get('license_file'):
        licenses.append(folder / package['license_file'])
    if not licenses:
        raise SystemExit(f"Review missing license text: {package['name']}")
    for path in dict.fromkeys(licenses):
        parts.append(f'\n--- {path.name} ---\n{path.read_text(errors="replace")}\n')
pathlib.Path(sys.argv[1]).write_text(''.join(parts))
