"""Platform-specific compiler settings used by the release workflow."""

import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
platform, target = os.environ["BILI_PLATFORM"], os.environ["BILI_TARGET"]
env = os.environ.copy()
features = "vendored-openssl" if platform.startswith("linux-") else ""
if features:
    env["CC"] = "musl-gcc"
if platform.startswith("windows-"):
    env["RUSTFLAGS"] = "-C target-feature=+crt-static"
command = ["cargo", "build", "--release", "--locked", "-p", "bili-cli", "--target", target]
if features:
    command += ["--features", features]
subprocess.run(command, cwd=ROOT, env=env, check=True)
suffix = ".exe" if platform.startswith("windows-") else ""
binary = ROOT / f"target/{target}/release/bili{suffix}"
ffmpeg = ROOT / f"build/ffmpeg-bin/ffmpeg{suffix}"
if platform.startswith("macos-"):
    for executable in [binary, ffmpeg]:
        subprocess.run(["codesign", "--force", "--sign", "-", str(executable)], check=True)
    for executable in [binary, ffmpeg]:
        dependencies = subprocess.check_output(["otool", "-L", str(executable)], text=True)
        for line in dependencies.splitlines()[1:]:
            dependency = line.strip().split(" ", 1)[0]
            if not dependency.startswith(("/usr/lib/", "/System/Library/")):
                raise RuntimeError(f"Non-system shared library: {dependency}")
if platform.startswith("linux-"):
    for executable in [binary, ffmpeg]:
        description = subprocess.check_output(["file", str(executable)], text=True)
        if "statically linked" not in description and "static-pie linked" not in description:
            raise RuntimeError(f"Not a static executable: {description}")
subprocess.run([sys.executable, "scripts/release/package.py", "--platform", platform,
                "--target", target, "--binary", str(binary), "--features", features], cwd=ROOT, check=True)
