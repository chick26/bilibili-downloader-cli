import os
from pathlib import Path
import subprocess
import sys

root = Path(__file__).resolve().parents[2]
platform = os.environ["BILI_PLATFORM"]
archives = list((root / "dist").glob(f"bili-v*-{platform}.zip" if platform.startswith("windows-") else f"bili-v*-{platform}.tar.gz"))
archive, = archives
subprocess.run([sys.executable, "scripts/release/verify_package.py", str(archive)], cwd=root, check=True)
