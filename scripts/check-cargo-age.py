#!/usr/bin/env python3
"""Reject registry dependencies published less than seven days ago or of unknown age."""

import argparse
import concurrent.futures
import datetime
import json
import os
from pathlib import Path
import sys
import time
import tomllib
import urllib.error
import urllib.request


def index_path(name):
    name = name.lower()
    if len(name) <= 2:
        return f"{len(name)}/{name}"
    if len(name) == 3:
        return f"3/{name[0]}/{name}"
    return f"{name[:2]}/{name[2:4]}/{name}"


def entries(name):
    cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    cached = list((cargo_home / "registry/index").glob(f"index.crates.io-*/.cache/{index_path(name)}"))
    if cached:
        return [json.loads(x) for x in cached[0].read_bytes().split(b"\0") if x.startswith(b"{")]
    req = urllib.request.Request(
        f"https://index.crates.io/{index_path(name)}",
        headers={"User-Agent": "voice-hub-dependency-age-check"},
    )
    for attempt in range(4):
        try:
            with urllib.request.urlopen(req, timeout=30) as response:
                return [json.loads(x) for x in response.read().splitlines()]
        except urllib.error.HTTPError as error:
            if error.code not in (408, 429, 500, 502, 503, 504) or attempt == 3:
                raise
        except (urllib.error.URLError, TimeoutError):
            if attempt == 3:
                raise
        time.sleep(2 ** attempt)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("lockfile", type=Path)
    args = parser.parse_args()
    packages = tomllib.loads(args.lockfile.read_text())["package"]
    cutoff = datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(days=7)
    names = {p["name"] for p in packages if p.get("source", "").startswith("registry+")}
    records = {}
    failures = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        jobs = {pool.submit(entries, name): name for name in names}
        for job in concurrent.futures.as_completed(jobs):
            name = jobs[job]
            try:
                records[name] = {v["vers"]: v for v in job.result()}
            except Exception as error:
                failures.append(f"{name}: cannot verify publication date ({type(error).__name__})")
    checked = 0
    for package in packages:
        if "source" not in package:
            continue
        name, version = package["name"], package["version"]
        if package["source"] != "registry+https://github.com/rust-lang/crates.io-index":
            failures.append(f"{name} {version}: unverified non-crates.io source")
            continue
        record = records.get(name, {}).get(version, {})
        try:
            published = datetime.datetime.fromisoformat(record["pubtime"].replace("Z", "+00:00"))
            if published > cutoff:
                failures.append(f"{name} {version}: too fresh ({published.isoformat()})")
            if record.get("yanked"):
                failures.append(f"{name} {version}: yanked")
        except (KeyError, ValueError):
            failures.append(f"{name} {version}: unknown publication date")
        checked += 1
    for failure in sorted(failures):
        print(failure, file=sys.stderr)
    print(f"Checked {checked} registry versions; cutoff {cutoff.isoformat()}; failures {len(failures)}")
    return bool(failures)


if __name__ == "__main__":
    sys.exit(main())
