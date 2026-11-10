#!/usr/bin/env python3
"""Package a signed universal app and publish an HTTPS OTA install directory."""
import argparse
from datetime import datetime, timezone
import hashlib
from html import escape
import json
import os
from pathlib import Path
import plistlib
import subprocess
import tempfile
import urllib.parse
import uuid


def atomic(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_bytes(data)
    os.replace(tmp, path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", required=True, type=Path)
    parser.add_argument("--publish-dir", required=True, type=Path)
    parser.add_argument("--base-url", required=True, help="HTTPS URL serving publish-dir")
    parser.add_argument("--device", action="append", default=[], help="Required registered device UDID; repeat as needed")
    args = parser.parse_args()
    if urllib.parse.urlsplit(args.base_url).scheme != "https":
        parser.error("OTA requires HTTPS")
    app = args.app.resolve()
    info = plistlib.loads((app / "Info.plist").read_bytes())
    if info.get("CFBundleIdentifier") != "com.jonthomason.torrbaby.ios":
        parser.error("Only com.jonthomason.torrbaby.ios may be published to Torrbaby OTA")
    if set(info.get("UIDeviceFamily", [])) != {1, 2}:
        parser.error("Expected a universal iPhone/iPad app")
    subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], check=True)
    profile = plistlib.loads(subprocess.check_output(["security", "cms", "-D", "-i", str(app / "embedded.mobileprovision")]))
    if profile["ExpirationDate"].replace(tzinfo=timezone.utc) <= datetime.now(timezone.utc):
        parser.error("Provisioning profile has expired")
    devices = set(profile.get("ProvisionedDevices", []))
    if not devices or set(args.device) - devices:
        parser.error("Provisioning profile does not cover the requested devices")
    entitlements = plistlib.loads(subprocess.check_output(["codesign", "-d", "--entitlements", ":-", str(app)], stderr=subprocess.DEVNULL))
    if not entitlements.get("application-identifier", "").endswith("." + info["CFBundleIdentifier"]):
        parser.error("Signed application identity mismatch")
    destination = args.publish_dir.expanduser().resolve()
    build = info["CFBundleVersion"]
    tag = build + "-" + uuid.uuid4().hex[:8]
    directory = destination / "builds" / tag
    directory.mkdir(parents=True)
    ipa = directory / "Torrbaby.ipa"
    with tempfile.TemporaryDirectory(prefix="torrbaby-ipa-") as tmp:
        payload = Path(tmp) / "Payload"
        payload.mkdir()
        subprocess.run(["ditto", str(app), str(payload / "Torrbaby.app")], check=True)
        subprocess.run(["ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", str(payload), str(ipa)], check=True)
    digest = hashlib.sha256(ipa.read_bytes()).hexdigest()
    base = args.base_url.rstrip("/")
    manifest = {"items": [{"assets": [{"kind": "software-package", "url": base + "/builds/" + tag + "/Torrbaby.ipa"}], "metadata": {"bundle-identifier": info["CFBundleIdentifier"], "bundle-version": info["CFBundleShortVersionString"], "kind": "software", "title": "Torrbaby " + info["CFBundleShortVersionString"] + " (" + build + ")"}}]}
    atomic(directory / "manifest.plist", plistlib.dumps(manifest))
    metadata = {"version": info["CFBundleShortVersionString"], "build": build, "sha256": digest, "device_count": len(devices), "profile_expires": profile["ExpirationDate"].isoformat(), "artifact": "builds/" + tag + "/Torrbaby.ipa"}
    atomic(directory / "build.json", json.dumps(metadata, indent=2).encode())
    install = "itms-services://?action=download-manifest&url=" + urllib.parse.quote(base + "/builds/" + tag + "/manifest.plist", safe="")
    html = '''<!doctype html><html lang="en"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Torrbaby</title><style>body{background:#101820;color:#eef5f3;font:17px -apple-system,BlinkMacSystemFont,sans-serif;max-width:520px;margin:12vh auto;padding:24px}h1{font-size:44px;letter-spacing:-2px}p{line-height:1.6;color:#afbdc5}.button{display:block;background:#70e8c2;color:#10221c;text-align:center;padding:17px;border-radius:14px;font-weight:650;text-decoration:none;margin:28px 0}small{color:#8fa0ac}a{color:#70e8c2}</style><h1>torrbaby</h1><p>A home for your downloads.</p><a class="button" href="''' + escape(install, quote=True) + '''">Install on iPhone or iPad</a><p>Build ''' + escape(build) + ''' · ''' + escape(info["CFBundleShortVersionString"]) + '''</p><p>Open this page in Safari on a registered device with Tailscale connected. This is a development-signed build; Developer Mode may be required.</p><p>After installing, choose Scan connection code. On an already linked Mac, iPhone or iPad, open Settings → Link a device to show the QR code. Credentials are never included in this download.</p><small>MIT licensed · <a href="https://github.com/jonthomason/torrbaby">Source code</a></small></html>'''
    android_metadata = destination / "android.json"
    if android_metadata.exists():
        android = json.loads(android_metadata.read_text())
        artifact = android["artifact"]
        if not artifact.startswith("android/") or ".." in artifact or not (destination / artifact).is_file():
            raise ValueError("Invalid Android artifact")
        android_link = '<a class="button" href="' + escape(base + "/" + artifact, quote=True) + '">Download for Android TV</a><p>Android build ' + escape(str(android["build"])) + ' · ' + escape(android["version"]) + '</p>'
        html = html.replace("<small>", android_link + "<small>", 1)
    atomic(destination / "current.json", json.dumps(metadata, indent=2).encode())
    atomic(destination / "manifest.plist", plistlib.dumps(manifest))
    atomic(destination / "index.html", html.encode())
    print(json.dumps({"install_page": base + "/index.html", "sha256": digest, "build": build, "profile_expires": metadata["profile_expires"], "devices": len(devices)}, indent=2))


if __name__ == "__main__":
    main()
