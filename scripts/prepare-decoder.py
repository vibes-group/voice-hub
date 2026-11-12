#!/usr/bin/env python3
"""Prepare the pinned LGPL decoder archive without executing its binaries."""
import argparse
import datetime
import hashlib
import json
from pathlib import Path
import urllib.request


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--github-env', type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    asset = json.loads((root / 'src-gpui/decoder-artifact.json').read_text())
    published = datetime.datetime.fromisoformat(asset['published'].replace('Z', '+00:00'))
    if datetime.datetime.now(datetime.timezone.utc) - published < datetime.timedelta(days=7):
        raise SystemExit('Decoder archive is younger than seven days')
    destination = root / 'src-gpui/.native/decoder' / asset['name']
    destination.parent.mkdir(parents=True, exist_ok=True)
    if not destination.exists() or hashlib.file_digest(destination.open('rb'), 'sha256').hexdigest() != asset['sha256']:
        request = urllib.request.Request(
            f"https://github.com/{asset['repository']}/releases/download/{asset['tag']}/{asset['name']}",
            headers={'User-Agent': 'voice-hub-native-decoder'},
        )
        partial = destination.with_suffix('.part')
        digest = hashlib.sha256()
        try:
            with urllib.request.urlopen(request, timeout=120) as response, partial.open('wb') as output:
                while chunk := response.read(1024 * 1024):
                    digest.update(chunk)
                    output.write(chunk)
            if digest.hexdigest() != asset['sha256']:
                raise SystemExit('Decoder archive checksum mismatch')
            partial.replace(destination)
        finally:
            partial.unlink(missing_ok=True)
    if args.github_env:
        with args.github_env.open('a', encoding='utf-8') as output:
            output.write(f'APP_DECODER_ARCHIVE={destination}\n')
    print(f"Verified {asset['name']} ({asset['sha256']})")


if __name__ == '__main__':
    main()
