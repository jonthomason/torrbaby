#!/usr/bin/env python3
"""Publish an Android TV build on Diamond, where the TV app finds it (server/updates.rs).

    python3 scripts/publish_android_update.py [--apk Android/app/build/outputs/apk/debug/app-debug.apk]

The APK goes to <data>/updates/android/<build>-<random>/Torrbaby.apk and current.json is replaced
last, so no TV is offered a build that is not all there. The TV installs it only over a build
signed with the same key (the debug key of the Mac that builds them).
"""
import argparse
import glob
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import tempfile
import uuid

DIAMOND_DATA = "Library/Application Support/Torrbaby/updates/android"
# For a first install: Torrbaby's own page (scripts/package_ota.py) and the OTA Installs page,
# which lists Android apps from ~/.cache/miyu/ota/<key>/ (current.json, builds/<tag>/, icon.png).
PAGE = Path("~/.cache/miyu/ota/torrbaby").expanduser()
PAGE_BASE = "https://jons-m5-mbp.bengal-locrian.ts.net/ota/torrbaby"
INSTALLS = Path("~/.cache/miyu/ota/torrbaby-android").expanduser()
ICON = Path(__file__).resolve().parent.parent / "Android/app/src/main/res/drawable-nodpi/torrbaby_icon.png"
PACKAGE = "com.torrbaby.tv.debug"


def badging(apk):
    tools = sorted(glob.glob(os.path.expanduser("~/Library/Android/sdk/build-tools/*/aapt2")))
    if not tools:
        raise SystemExit("aapt2 not found under ~/Library/Android/sdk/build-tools")
    out = subprocess.run([tools[-1], "dump", "badging", str(apk)], check=True, capture_output=True, text=True).stdout
    found = re.search(r"package: name='([^']+)' versionCode='(\d+)' versionName='([^']+)'", out)
    if not found:
        raise SystemExit("Could not read the APK's package and version")
    return found.group(1), int(found.group(2)), found.group(3)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apk", type=Path, default=Path("Android/app/build/outputs/apk/debug/app-debug.apk"))
    args = parser.parse_args()
    package, build, version = badging(args.apk)
    if package != PACKAGE:
        raise SystemExit(f"Expected {PACKAGE}, the package the TVs have, not {package}")
    payload = args.apk.read_bytes()
    tag = f"{build}-{uuid.uuid4().hex[:8]}"
    current = {"build": build, "version": version, "sha256": hashlib.sha256(payload).hexdigest(), "size": len(payload),
               "apk": f"/updates/android/{tag}/Torrbaby.apk"}
    with tempfile.TemporaryDirectory(prefix="torrbaby-android-") as tmp:
        (Path(tmp) / tag).mkdir()
        (Path(tmp) / tag / "Torrbaby.apk").write_bytes(payload)
        (Path(tmp) / "current.json.new").write_text(json.dumps(current, indent=2))
        packed = subprocess.run(["tar", "-C", tmp, "-cf", "-", tag, "current.json.new"], check=True, capture_output=True).stdout
    folder = shlex.quote(DIAMOND_DATA)
    subprocess.run(["ssh", "diamond", "mkdir -p " + folder + " && cd " + folder + " && tar -xf - && mv current.json.new current.json"
                    # The five newest builds are kept; older ones go.
                    + " && ls -d */ | sort -t- -k1,1nr | tail -n +6 | xargs rm -rf --"], input=packed, check=True)
    # And on the install page.
    import sys
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    from package_ota import atomic, write_page
    artifact = f"android/Torrbaby-{version}-build{build}.apk"
    atomic(PAGE / artifact, payload)
    atomic(PAGE / "android.json", json.dumps({"version": version, "build": build, "artifact": artifact, "sha256": current["sha256"]}, indent=2).encode())
    write_page(PAGE, PAGE_BASE)
    atomic(INSTALLS / "builds" / tag / "Torrbaby.apk", payload)
    atomic(INSTALLS / "icon.png", ICON.read_bytes())
    atomic(INSTALLS / "current.json", json.dumps({"version": version, "build": str(build), "size": len(payload),
                                                   "sha256": current["sha256"], "artifact": f"builds/{tag}/Torrbaby.apk"}, indent=2).encode())
    print(json.dumps({k: current[k] for k in ("build", "version", "sha256", "size")}, indent=2))


if __name__ == "__main__":
    main()
