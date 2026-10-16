"""Create a portable archive with FFmpeg, upstream notices and dependency licenses."""

import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[2]


def dependency_licenses(destination, target, features):
    command = ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", target]
    if features:
        command += ["--features", features]
    metadata = json.loads(subprocess.check_output(command, cwd=ROOT))
    supplement = ROOT / "scripts/release/licenses"
    packages = []
    for package in metadata["packages"]:
        if package["source"] is None:
            continue
        source = Path(package["manifest_path"]).parent
        files = set()
        if package.get("license_file"):
            files.add(source / package["license_file"])
        for candidate in source.rglob("*"):
            if candidate.is_file() and candidate.name.upper().startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE", "AUTHORS")):
                files.add(candidate)
        if not files:
            original = supplement / f"{package['name']}-{package['version']}.txt"
            if not original.is_file():
                raise RuntimeError(f"No license text found for {package['name']} {package['version']}")
            output = destination / "rust" / f"{package['name']}-{package['version']}" / "LICENSE"
            output.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(original, output)
        name = f"{package['name']}-{package['version']}"
        for file in sorted(files):
            relative = file.relative_to(source)
            output = destination / "rust" / name / relative
            output.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(file, output)
        packages.append({"name": package["name"], "version": package["version"], "license": package["license"], "repository": package["repository"]})
    (destination / "rust-dependencies.json").write_text(json.dumps(packages, indent=2) + "\n", encoding="utf-8")
    shutil.copyfile(supplement / "sources.json", destination / "rust-license-sources.json")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--platform", required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--features", default="")
    args = parser.parse_args()
    version = tomllib.loads((ROOT / "crates/bili-cli/Cargo.toml").read_text())["package"]["version"]
    spec = json.loads((ROOT / "scripts/release/ffmpeg.json").read_text())
    name = f"bili-v{version}-{args.platform}"
    output = ROOT / "dist"
    output.mkdir(exist_ok=True)
    bundle = output / name
    if bundle.exists():
        shutil.rmtree(bundle)
    bundle.mkdir()
    suffix = ".exe" if args.platform.startswith("windows-") else ""
    shutil.copy2(args.binary, bundle / f"bili{suffix}")
    shutil.copy2(ROOT / f"build/ffmpeg-bin/ffmpeg{suffix}", bundle / f"ffmpeg{suffix}")
    for file in ["LICENSE", "UPSTREAM.md"]:
        shutil.copy2(ROOT / file, bundle / file)
    shutil.copy2(ROOT / "docs/INSTALL.md", bundle / "README.md")
    licenses = bundle / "licenses"
    licenses.mkdir()
    source = ROOT / f"build/ffmpeg-{spec['version']}"
    shutil.copy2(source / "COPYING.LGPLv2.1", licenses / "FFmpeg-LGPL-2.1.txt")
    shutil.copy2(source / "LICENSE.md", licenses / "FFmpeg-LICENSE.md")
    shutil.copy2(ROOT / "scripts/release/build_ffmpeg.sh", licenses / "build_ffmpeg.sh")
    shutil.copy2(ROOT / "scripts/release/ffmpeg.json", licenses / "ffmpeg-source.json")
    buildconf = subprocess.check_output([str(bundle / f"ffmpeg{suffix}"), "-buildconf"], stderr=subprocess.STDOUT, text=True)
    (licenses / "ffmpeg-build.txt").write_text(buildconf, encoding="utf-8")
    (licenses / "FFmpeg-NOTICE.txt").write_text(
        "This package includes the unmodified FFmpeg command-line program, built from official source.\n"
        "FFmpeg is Copyright (c) 2000-2026 the FFmpeg developers, licensed under LGPL-2.1-or-later.\n"
        "bili invokes FFmpeg as a separate process. No FFmpeg library is linked into bili.\n"
        "GPL, nonfree and external codec components are disabled.\n"
        f"Exact corresponding source: https://github.com/chick26/bilibili-downloader-cli/releases/download/v{version}/ffmpeg-{spec['version']}.tar.xz\n"
        "The source archive is unchanged. The included build script and build configuration reproduce this component.\n"
        "You may rebuild and replace the adjacent ffmpeg executable, or select your own with --ffmpeg.\n",
        encoding="utf-8",
    )
    dependency_licenses(licenses, args.target, args.features)
    if suffix:
        archive = output / f"{name}.zip"
        with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as zip:
            for file in sorted(bundle.rglob("*")):
                if file.is_file():
                    zip.write(file, file.relative_to(output))
    else:
        archive = output / f"{name}.tar.gz"
        with tarfile.open(archive, "w:gz") as tar:
            tar.add(bundle, arcname=name)
    checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
    archive.with_name(archive.name + ".sha256").write_text(f"{checksum}  {archive.name}\n")
    print(archive.name)


if __name__ == "__main__":
    main()
