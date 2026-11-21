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
import shlex
import subprocess
import tempfile
import urllib.parse
import uuid


def atomic(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_bytes(data)
    os.replace(tmp, path)


DIAMOND_BASE = "https://torrbaby.diamond.abt.jonthomason.com/updates/ios"
DIAMOND_DATA = "Library/Application Support/Torrbaby/updates/ios"


def publish_on_diamond(directory, tag, info, build, digest):
    """The build goes to Diamond's data folder (server/updates.rs); current.json is written last,
    so no device is offered a build whose files are not all there."""
    manifest = {"items": [{"assets": [{"kind": "software-package", "url": DIAMOND_BASE + "/" + tag + "/Torrbaby.ipa"}], "metadata": {"bundle-identifier": info["CFBundleIdentifier"], "bundle-version": info["CFBundleShortVersionString"], "kind": "software", "title": "Torrbaby " + info["CFBundleShortVersionString"] + " (" + build + ")"}}]}
    current = {"build": build, "version": info["CFBundleShortVersionString"], "sha256": digest,
               "size": (directory / "Torrbaby.ipa").stat().st_size, "manifest": DIAMOND_BASE + "/" + tag + "/manifest.plist"}
    with tempfile.TemporaryDirectory(prefix="torrbaby-diamond-") as tmp:
        staged = Path(tmp) / tag
        staged.mkdir()
        subprocess.run(["cp", str(directory / "Torrbaby.ipa"), str(staged / "Torrbaby.ipa")], check=True)
        (staged / "manifest.plist").write_bytes(plistlib.dumps(manifest))
        (Path(tmp) / "current.json").write_text(json.dumps(current, indent=2))
        # current.json arrives under another name and replaces the old one only once the build is in.
        (Path(tmp) / "current.json").rename(Path(tmp) / "current.json.new")
        packed = subprocess.run(["tar", "-C", tmp, "-cf", "-", tag, "current.json.new"], check=True, capture_output=True).stdout
        folder = shlex.quote(DIAMOND_DATA)
        subprocess.run(["ssh", "diamond", "mkdir -p " + folder + " && cd " + folder + " && tar -xf - && mv current.json.new current.json"
                        # The five newest builds are kept; older ones go.
                        + " && ls -d */ | sort -t- -k1,1nr | tail -n +6 | xargs rm -rf --"], input=packed, check=True)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", required=True, type=Path)
    parser.add_argument("--publish-dir", required=True, type=Path)
    parser.add_argument("--base-url", required=True, help="HTTPS URL serving publish-dir")
    parser.add_argument("--device", action="append", default=[], help="Required registered device UDID; repeat as needed")
    parser.add_argument("--diamond", action="store_true", help="Also publish the build on Diamond, where the app finds it (Install N)")
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
    atomic(destination / "current.json", json.dumps(metadata, indent=2).encode())
    atomic(destination / "manifest.plist", plistlib.dumps(manifest))
    write_page(destination, base)
    if args.diamond:
        publish_on_diamond(directory, tag, info, build, digest)
    print(json.dumps({"install_page": base + "/index.html", "sha256": digest, "build": build, "profile_expires": metadata["profile_expires"], "devices": len(devices)}, indent=2))


def write_page(destination, base):
    """The install page from what is published: the iPhone/iPad build (current.json), Android
    (android.json, phones and TVs) and the Mac and tvOS simulator downloads."""
    metadata = json.loads((destination / "current.json").read_text())
    build, version = metadata["build"], metadata["version"]
    install = "itms-services://?action=download-manifest&url=" + urllib.parse.quote(base + "/" + metadata["artifact"].rsplit("/", 1)[0] + "/manifest.plist", safe="")
    # The page's colour fills the whole screen: on Android the area past the content and the
    # overscroll showed white when only the body was coloured.
    html = '''<!doctype html><html lang="en"><meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover"><meta name="theme-color" content="#101820"><title>Torrbaby</title><style>html{background:#101820;min-height:100%}body{background:#101820;color:#eef5f3;font:17px -apple-system,BlinkMacSystemFont,Roboto,sans-serif;max-width:520px;margin:0 auto;padding:12vh 24px 24px;min-height:100vh;box-sizing:border-box}h1{font-size:44px;letter-spacing:-2px}p{line-height:1.6;color:#afbdc5}.button{display:block;background:#70e8c2;color:#10221c;text-align:center;padding:17px;border-radius:14px;font-weight:650;text-decoration:none;margin:28px 0}small{color:#8fa0ac}a{color:#70e8c2}</style><h1>torrbaby</h1><p>A home for your downloads.</p><a class="button" href="''' + escape(install, quote=True) + '''">Install on iPhone or iPad</a><p>Build ''' + escape(build) + ''' · ''' + escape(version) + '''</p><p>Open this page in Safari on a registered device with Tailscale connected. This is a development-signed build; Developer Mode may be required.</p><p>After installing, choose Scan connection code. On an already linked Mac, iPhone or iPad, open Settings → Link a device to show the QR code. Credentials are never included in this download.</p><small>MIT licensed · <a href="https://github.com/jonthomason/torrbaby">Source code</a></small></html>'''
    android_metadata = destination / "android.json"
    if android_metadata.exists():
        android = json.loads(android_metadata.read_text())
        artifact = android["artifact"]
        if not artifact.startswith("android/") or ".." in artifact or not (destination / artifact).is_file():
            raise ValueError("Invalid Android artifact")
        android_link = '<a class="button" href="' + escape(base + "/" + artifact, quote=True) + '">Download for Android</a><p>Android build ' + escape(str(android["build"])) + ' · phones, tablets and TVs</p>'
        html = html.replace("<small>", android_link + "<small>", 1)
    for platform, label in [("macos", "Download for Mac"), ("tvos-simulator", "Download tvOS simulator build")]:
        release_file = destination / (platform + ".json")
        if release_file.exists():
            release = json.loads(release_file.read_text())
            artifact = release["artifact"]
            if not artifact.startswith(platform + "/") or ".." in artifact or not (destination / artifact).is_file():
                raise ValueError("Invalid " + platform + " artifact")
            link = '<a class="button" href="' + escape(base + "/" + artifact, quote=True) + '">' + label + '</a><p>Build ' + escape(str(release["build"])) + '</p>'
            html = html.replace("<small>", link + "<small>", 1)
    atomic(destination / "index.html", html.encode())


if __name__ == "__main__":
    main()
