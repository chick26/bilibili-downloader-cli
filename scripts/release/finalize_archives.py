"""Add runtime copyright texts to existing packages without changing executable bytes."""

import argparse
import hashlib
from pathlib import Path
import tarfile
import tempfile
import tomllib
import zipfile

from package import ROOT, runtime_licenses


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("directory", type=Path)
    args = parser.parse_args()
    version = tomllib.loads((ROOT / "crates/bili-cli/Cargo.toml").read_text())["package"]["version"]
    archives = sorted(args.directory.glob(f"bili-v{version}-*.tar.gz")) + sorted(args.directory.glob(f"bili-v{version}-*.zip"))
    for archive in archives:
        platform = archive.name.removeprefix(f"bili-v{version}-").removesuffix(".tar.gz").removesuffix(".zip")
        with tempfile.TemporaryDirectory(prefix="bili-notices-") as temporary:
            temp = Path(temporary)
            if archive.suffix == ".zip":
                with zipfile.ZipFile(archive) as zip:
                    zip.extractall(temp)
            else:
                with tarfile.open(archive) as tar:
                    tar.extractall(temp, filter="data")
            bundle, = temp.iterdir()
            binaries = [file for file in bundle.iterdir() if file.name in {"bili", "bili.exe", "ffmpeg", "ffmpeg.exe"}]
            assert len(binaries) == 2
            before = {file.name: hashlib.sha256(file.read_bytes()).hexdigest() for file in binaries}
            runtime_licenses(bundle / "licenses", platform)
            assert before == {file.name: hashlib.sha256(file.read_bytes()).hexdigest() for file in binaries}
            if archive.suffix == ".zip":
                with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as zip:
                    for file in sorted(bundle.rglob("*")):
                        if file.is_file():
                            zip.write(file, file.relative_to(temp))
            else:
                with tarfile.open(archive, "w:gz") as tar:
                    tar.add(bundle, arcname=bundle.name)
        checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
        archive.with_name(archive.name + ".sha256").write_text(f"{checksum}  {archive.name}\n")
        print(f"Runtime notices complete; executable bytes preserved: {platform}")


if __name__ == "__main__":
    main()
