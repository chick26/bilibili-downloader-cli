"""Smoke-test extracted release archives with an empty environment and synthetic media."""

import argparse
import base64
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parents[2]


def run(command, env, **kwargs):
    return subprocess.run([str(item) for item in command], env=env, check=True, capture_output=True, text=True, encoding="utf-8", **kwargs)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("archive", type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="bili-package-test-") as temporary:
        temp = Path(temporary)
        if args.archive.suffix == ".zip":
            with zipfile.ZipFile(args.archive) as zip:
                zip.extractall(temp)
        else:
            with tarfile.open(args.archive) as tar:
                tar.extractall(temp, filter="data")
        bundle, = temp.iterdir()
        suffix = ".exe" if os.name == "nt" else ""
        bili, ffmpeg = bundle / f"bili{suffix}", bundle / f"ffmpeg{suffix}"
        env = os.environ.copy()
        env["PATH"] = str(Path(os.environ.get("SystemRoot", "C:/Windows")) / "System32") if os.name == "nt" else "/usr/bin:/bin"
        env["HOME"] = str(temp / "home")
        env.pop("BILI_DATA_DIR", None)
        run([bili, "--version"], env)
        run([bili, "--help"], env)
        state = temp / "state"
        for command in [["auth", "status"], ["tasks", "list"]]:
            response = run([bili, "--data-dir", state, *command, "--json"], env)
            assert json.loads(response.stdout)["ok"]
        config = run([ffmpeg, "-buildconf"], env)
        assert "--enable-gpl" not in config.stderr + config.stdout
        assert "--enable-nonfree" not in config.stderr + config.stdout
        for name in ["audio", "video"]:
            (temp / f"{name}.mp4").write_bytes(base64.decodebytes((ROOT / f"scripts/release/fixtures/{name}.base64").read_bytes()))
        metadata = temp / "chapters.ini"
        metadata.write_text(";FFMETADATA1\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=1000\ntitle=Release smoke test\n")
        output = temp / "merged.mp4"
        run([ffmpeg, "-nostdin", "-i", temp / "video.mp4", "-i", temp / "audio.mp4", "-i", metadata,
             "-map_metadata", "2", "-c", "copy", "-map", "0:v:0", "-map", "1:a:0", "-y", output], env)
        inspected = run([ffmpeg, "-nostdin", "-i", output, "-c", "copy", "-y", temp / "remuxed.mp4"], env)
        assert "Video: h264" in inspected.stderr and "Audio: aac" in inspected.stderr
        assert "Release smoke test" in inspected.stderr
        assert output.stat().st_size > 0
        assert (bundle / "licenses/FFmpeg-LGPL-2.1.txt").is_file()
        assert (bundle / "licenses/rust-dependencies.json").is_file()
    print(f"Portable package passed: {args.archive.name}")


if __name__ == "__main__":
    main()
