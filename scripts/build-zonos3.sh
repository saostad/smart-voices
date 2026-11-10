#!/usr/bin/env bash
# Build the pinned ZONOS3 ggml server (the runtime of the optional Zonos voice
# model) and put it next to the app binaries, like fetch-onnxruntime.sh does for
# ONNX Runtime. The build is cached under ~/.cache/smart-voices/zonos3.
#
# Linux and Windows builds target x86-64 CPUs with AVX2/FMA/F16C (2013 and
# newer); the app checks the CPU before starting the server. macOS builds use
# Metal on Apple Silicon.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)

zonos_repo=https://github.com/Zyphra/ZONOS3-Mini.git
zonos_commit=b820f6da278862d7c2db3e628d21cbae85983720
ggml_commit=a05c9922613c6a2230dbb1d0de679519ecafb4f0
# Our fixes on top of the pinned source, applied in name order.
patches=("$root"/packaging/zonos3/*.patch)

case "${RUNNER_OS:-$(uname -s)}" in
 Linux) exe=zonos3-server; flags=(-DGGML_NATIVE=OFF -DGGML_AVX=ON -DGGML_AVX2=ON -DGGML_FMA=ON -DGGML_F16C=ON) ;;
 Darwin|macOS) exe=zonos3-server; flags=(-DGGML_NATIVE=OFF -DGGML_METAL=ON -DGGML_METAL_EMBED_LIBRARY=ON) ;;
 Windows|MINGW*|MSYS*) exe=zonos3-server.exe; flags=(-DGGML_NATIVE=OFF -DGGML_AVX=ON -DGGML_AVX2=ON -DGGML_FMA=ON -DGGML_F16C=ON -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded) ;;
 *) echo 'error: unsupported platform for the ZONOS3 runtime' >&2; exit 1 ;;
esac

patch_hash=$( (command -v sha256sum >/dev/null && cat "${patches[@]}" "$0" | sha256sum || cat "${patches[@]}" "$0" | shasum -a 256) | cut -c1-12)
cache=${XDG_CACHE_HOME:-$HOME/.cache}/smart-voices/zonos3
built=$cache/$zonos_commit-$patch_hash/$exe

if [ ! -x "$built" ]; then
 src=$cache/src
 rm -rf "$src"
 mkdir -p "$src"
 git -C "$src" init -q
 git -C "$src" remote add origin "$zonos_repo"
 git -C "$src" fetch -q --depth 1 origin "$zonos_commit"
 git -C "$src" checkout -q FETCH_HEAD
 git -C "$src" submodule update -q --init --depth 1 servers/ggml/ggml
 actual_ggml=$(git -C "$src/servers/ggml/ggml" rev-parse HEAD)
 if [ "$actual_ggml" != "$ggml_commit" ]; then
  echo "error: ZONOS3 $zonos_commit pins ggml $actual_ggml, expected $ggml_commit" >&2
  exit 1
 fi
 for p in "${patches[@]}"; do git -C "$src" apply "$p"; done
 cmake -S "$src/servers/ggml" -B "$src/build" -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF -DGGML_OPENMP=OFF "${flags[@]}"
 cmake --build "$src/build" --config Release --target zonos3-server -j "${ZONOS3_BUILD_JOBS:-4}"
 out=$(find "$src/build" -type f -name "$exe" | head -1)
 if [ -z "$out" ]; then echo "error: the ZONOS3 build produced no $exe" >&2; exit 1; fi
 mkdir -p "$(dirname "$built")"
 cp "$out" "$built.partial"
 mv "$built.partial" "$built"
 rm -rf "$src"
fi

mkdir -p "$root/target/debug" "$root/target/release"
cp "$built" "$root/target/debug/$exe"
cp "$built" "$root/target/release/$exe"
echo "ZONOS3 runtime ready: target/release/$exe"
