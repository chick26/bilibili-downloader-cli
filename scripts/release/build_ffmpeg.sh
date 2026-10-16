#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
root="$PWD"
platform="${1:?platform required}"
version="$("${PYTHON:-python3}" -c 'import json; print(json.load(open("scripts/release/ffmpeg.json"))["version"])')"
cd "build/ffmpeg-$version"
extra=(--disable-gpl --disable-nonfree)
case "$platform" in
  linux-*) extra+=(--cc=musl-gcc --extra-ldflags=-static --enable-cross-compile) ;;
  windows-*) extra+=(--target-os=mingw32 --arch=x86_64 --cc=gcc --extra-ldflags=-static --disable-pthreads --enable-w32threads) ;;
  macos-*) export MACOSX_DEPLOYMENT_TARGET=12.0 ;;
  *) echo "Unsupported platform: $platform" >&2; exit 1 ;;
esac
# Only remuxing and chapter metadata are needed; no external codec libraries.
./configure --prefix=/usr/local --disable-autodetect --disable-everything \
  --disable-doc --disable-debug --disable-shared --enable-static \
  --disable-network --disable-x86asm --enable-ffmpeg --enable-protocol=file \
  --enable-demuxer=mov,ffmetadata --enable-muxer=mp4,mov \
  --enable-parsers --enable-bsf=aac_adtstoasc,extract_extradata,av1_frame_merge \
  "${extra[@]}"
program=ffmpeg
if [[ "$platform" == windows-* ]]; then
  program=ffmpeg.exe
fi
make -j"${JOBS:-4}" "$program"
mkdir -p "$root/build/ffmpeg-bin"
if [[ "$platform" == windows-* ]]; then
  cp ffmpeg.exe "$root/build/ffmpeg-bin/ffmpeg.exe"
else
  cp ffmpeg "$root/build/ffmpeg-bin/ffmpeg"
fi
