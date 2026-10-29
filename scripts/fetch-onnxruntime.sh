#!/usr/bin/env bash
# ONNX Runtime 1.28.2 from the pinned sherpa release, including Windows MT CRT.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
case "${RUNNER_OS:-$(uname -s)}" in
 Linux) platform=linux-x64-shared-lib; hash=3892d184be41027e18165e67f549cd4e4cdd8dcd73ac5579e97afd55e14e30b6; pattern='libonnxruntime.so*' ;;
 Darwin|macOS) platform=osx-arm64-shared-lib; hash=ae77050cdae565496059d96f5ab33d77b397a0864282a4e4a26e3b3b3effb948; pattern='libonnxruntime*.dylib' ;;
 Windows|MINGW*|MSYS*) platform=win-x64-shared-MT-Release-lib; hash=b8eedf41bd6d3779218887b48367bb7a3ece5aaa7667f01f69ee823a12b0a9e7; pattern='onnxruntime.dll' ;;
 *) echo 'Unsupported platform for pinned ONNX Runtime' >&2; exit 1 ;;
esac
stem=sherpa-onnx-v1.13.8-$platform
cache=${XDG_CACHE_HOME:-$HOME/.cache}/smart-voices/onnxruntime
mkdir -p "$cache"
archive=$cache/$stem.tar.bz2
if [ ! -f "$archive" ]; then
 curl -fL --retry 2 "https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.8/$stem.tar.bz2" -o "$archive.partial"
 mv "$archive.partial" "$archive"
fi
if command -v sha256sum >/dev/null; then
 echo "$hash  $archive" | sha256sum -c -
else
 echo "$hash  $archive" | shasum -a 256 -c -
fi
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
tar -xjf "$archive" -C "$tmp"
mkdir -p "$root/target/debug" "$root/target/release"
found=0
while IFS= read -r file; do
 cp -L "$file" "$root/target/debug/"
 cp -L "$file" "$root/target/release/"
 found=1
done < <(find "$tmp/$stem/lib" -name "$pattern")
if [ "$found" != 1 ]; then echo 'ONNX Runtime library missing from verified archive' >&2; exit 1; fi
