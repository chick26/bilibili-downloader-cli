import hashlib
import json
from pathlib import Path
import shutil
import tomllib

root = Path(__file__).resolve().parents[2]
output = root / "dist"
version = tomllib.loads((root / "crates/bili-cli/Cargo.toml").read_text())["package"]["version"]
spec = json.loads((root / "scripts/release/ffmpeg.json").read_text())
source = root / f"build/ffmpeg-{spec['version']}.tar.xz"
shutil.copy2(source, output / source.name)
archives = sorted(file for file in output.iterdir() if file.name.endswith((".tar.gz", ".zip", ".tar.xz")))
if len(archives) != 6:
    raise RuntimeError(f"Expected five packages and FFmpeg source, got {len(archives)}")
checksums = {file.name: hashlib.sha256(file.read_bytes()).hexdigest() for file in archives}
(output / "SHA256SUMS").write_text("".join(f"{checksum}  {name}\n" for name, checksum in checksums.items()))
template = (root / "homebrew/bili.rb.in").read_text()
template = template.replace("@VERSION@", version)
for platform in ["macos-arm64", "macos-x86_64", "linux-arm64", "linux-x86_64"]:
    template = template.replace(f"@{platform.upper().replace('-', '_')}_SHA256@", checksums[f"bili-v{version}-{platform}.tar.gz"])
if "@" in template:
    raise RuntimeError("Unresolved formula template placeholder")
(output / "bili.rb").write_text(template)
print("Assembled five portable packages, FFmpeg source, SHA256SUMS and Homebrew formula")
