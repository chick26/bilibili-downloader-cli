"""Fetch and verify the exact source shipped alongside the portable packages."""

import hashlib
import json
from pathlib import Path
import tarfile
import urllib.request

ROOT = Path(__file__).resolve().parents[2]


def main():
    spec = json.loads((ROOT / "scripts/release/ffmpeg.json").read_text())
    build = ROOT / "build"
    build.mkdir(exist_ok=True)
    archive = build / f"ffmpeg-{spec['version']}.tar.xz"
    if not archive.exists():
        with urllib.request.urlopen(spec["url"], timeout=120) as response:
            archive.write_bytes(response.read())
    if hashlib.sha256(archive.read_bytes()).hexdigest() != spec["sha256"]:
        raise SystemExit("FFmpeg source checksum mismatch")
    source = build / f"ffmpeg-{spec['version']}"
    if not source.exists():
        with tarfile.open(archive) as tar:
            tar.extractall(build, filter="data")
    print(f"Verified FFmpeg {spec['version']} source")


if __name__ == "__main__":
    main()
