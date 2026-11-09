#!/usr/bin/env python3
"""Install or update a per-user macOS launchd service. Run on the server."""
import argparse
import json
import os
from pathlib import Path
import plistlib
import secrets
import shutil
import subprocess
import sys
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--media-root", required=True, type=Path)
parser.add_argument("--listen-host", default="127.0.0.1", help="Use a private VPN IP for remote clients")
parser.add_argument("--port", type=int, default=8765)
parser.add_argument("--paused", action="store_true", help="Start a new installation paused")
args = parser.parse_args()
if sys.platform != "darwin":
    parser.error("This installer requires macOS; run torrbaby.py serve directly elsewhere")
media = args.media_root.resolve()
for folder in ("Downloads", "Movies", "TV"):
    if not (media / folder).is_dir():
        parser.error("Required media folder missing: " + str(media / folder))
source = Path(__file__).resolve().parent.parent
home = Path.home()
base = home / "Library/Application Support/Torrbaby"
app = base / "app"
app.mkdir(parents=True, exist_ok=True)
for filename in ("torrbaby.py", "storage.py", "metadata.py", "library.py", "README.md", "LICENSE", "config.example.json"):
    shutil.copy2(str(source / filename), str(app / filename))
config_path = base / "config.json"
if not config_path.exists():
    config = json.loads((source / "config.example.json").read_text())
    config.update(listen_host=args.listen_host, listen_port=args.port, paused=args.paused,
                  token_file=str(base / "api.token"), state_file=str(base / "state.json"),
                  source=str(media / "Downloads"), movies=str(media / "Movies"), tv=str(media / "TV"))
    config_path.write_text(json.dumps(config, indent=2) + "\n")
    config_path.chmod(0o600)
else:
    config = json.loads(config_path.read_text())
token_file = Path(config["token_file"]).expanduser()
if not token_file.exists():
    token_file.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(str(token_file), os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w") as f:
        f.write(secrets.token_urlsafe(48) + "\n")
label = "org.torrbaby.daemon"
plist_path = home / "Library/LaunchAgents" / (label + ".plist")
plist_path.parent.mkdir(parents=True, exist_ok=True)
plist = {"Label": label, "ProgramArguments": [sys.executable, str(app / "torrbaby.py"), "serve", "--config", str(config_path)],
         "RunAtLoad": True, "KeepAlive": True, "ThrottleInterval": 30,
         "StandardOutPath": str(base / "bootstrap.log"), "StandardErrorPath": str(base / "bootstrap.log"),
         "WorkingDirectory": str(app), "Umask": 0o077}
with plist_path.open("wb") as f:
    plistlib.dump(plist, f)
domain = "gui/" + str(os.getuid())
subprocess.run(["launchctl", "bootout", domain + "/" + label], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
for attempt in range(10):
    result = subprocess.run(["launchctl", "bootstrap", domain, str(plist_path)], capture_output=True, text=True)
    if result.returncode == 0:
        break
    time.sleep(1)
else:
    raise RuntimeError("Could not register Torrbaby: " + result.stderr.strip())
print(json.dumps({"app": str(app), "config": str(config_path), "token_file": str(token_file), "launch_agent": str(plist_path), "api": "http://%s:%s" % (config["listen_host"], config["listen_port"])}, indent=2))
