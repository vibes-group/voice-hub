#!/usr/bin/env python3
"""Download the reviewed WebRTC SDK, verifying its age and SHA256 before extraction."""

import argparse
import datetime
import hashlib
import json
from pathlib import Path, PurePosixPath
import shutil
import tempfile
import urllib.request
import zipfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", choices=["windows-x86_64", "linux-x86_64"], required=True)
    parser.add_argument("--github-env", type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    manifest = json.loads((root / "src-gpui/native-artifacts.json").read_text())
    asset = manifest["assets"][args.platform]
    published = datetime.datetime.fromisoformat(asset["published"].replace("Z", "+00:00"))
    if datetime.datetime.now(datetime.timezone.utc) - published < datetime.timedelta(days=7):
        raise SystemExit("WebRTC SDK is younger than seven days")
    destination = root / "src-gpui/.native" / manifest["tag"]
    sdk = destination / asset["directory"]
    marker = destination / (asset["name"] + ".sha256")
    if not (sdk.is_dir() and marker.is_file() and marker.read_text() == asset["sha256"]):
        destination.mkdir(parents=True, exist_ok=True)
        url = f"https://github.com/{manifest['repository']}/releases/download/{manifest['tag']}/{asset['name']}"
        with tempfile.TemporaryDirectory(prefix="voice-hub-webrtc-") as temp:
            archive = Path(temp) / "sdk.zip"
            digest = hashlib.sha256()
            req = urllib.request.Request(url, headers={"User-Agent": "voice-hub-native-sdk"})
            with urllib.request.urlopen(req, timeout=120) as response, archive.open("wb") as output:
                while chunk := response.read(1024 * 1024):
                    digest.update(chunk)
                    output.write(chunk)
            if digest.hexdigest() != asset["sha256"]:
                raise SystemExit("WebRTC SDK checksum mismatch")
            unpacked = Path(temp) / "unpacked"
            with zipfile.ZipFile(archive) as zipped:
                for entry in zipped.infolist():
                    path = PurePosixPath(entry.filename)
                    if path.is_absolute() or ".." in path.parts or "\\" in entry.filename:
                        raise SystemExit("Unsafe WebRTC archive entry")
                zipped.extractall(unpacked)
            source = unpacked / asset["directory"]
            if not (source / "include").is_dir() or not (source / "lib").is_dir():
                raise SystemExit("Unexpected WebRTC SDK layout")
            if sdk.exists():
                shutil.rmtree(sdk)
            shutil.move(str(source), sdk)
            marker.write_text(asset["sha256"])
    if args.github_env:
        with args.github_env.open("a", encoding="utf-8") as output:
            output.write(f"LK_CUSTOM_WEBRTC={sdk}\n")
    print(f"Verified {asset['name']} ({asset['sha256']})")


if __name__ == "__main__":
    main()
