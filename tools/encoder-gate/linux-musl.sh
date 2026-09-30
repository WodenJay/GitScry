#!/usr/bin/env sh
# Run ONLY on the remote Actions runner, inside its Alpine container.
set -eu
cd "$(dirname "$0")"
apk add --no-cache build-base cmake ninja git python3 linux-headers curl
mkdir -p .native
cd .native
# Upstream 1.28.0 includes the Abseil stacktrace fix; no local source patches.
git init onnxruntime
git -C onnxruntime remote add origin https://github.com/microsoft/onnxruntime.git
git -C onnxruntime fetch --depth 1 origin da9b5e364c465de65c49d91e696cd6485270757f
git -C onnxruntime checkout --detach FETCH_HEAD
git -C onnxruntime submodule update --init --recursive
python3 onnxruntime/tools/ci_build/build.py \
  --build_dir "$PWD/ort" --config Release --update --build \
  --skip_tests --skip_submodule_sync --allow_running_as_root \
  --parallel 2 --cmake_generator Ninja \
  --cmake_extra_defines \
    onnxruntime_BUILD_SHARED_LIB=OFF \
    onnxruntime_BUILD_FOR_NATIVE_MACHINE=OFF \
    onnxruntime_USE_AVX=OFF onnxruntime_USE_AVX2=OFF onnxruntime_USE_AVX512=OFF
# Never accept a silently patched runtime checkout.
git -C onnxruntime diff --exit-code
export ORT_LIB_LOCATION="$PWD/ort"
export ORT_LIB_PROFILE=Release
export ORT_PREFER_DYNAMIC_LINK=0
export ORT_CXX_STDLIB=static=stdc++
cd ..
rustup component add rustfmt clippy
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features --jobs 1 -- -D warnings
cargo build --locked --release --jobs 1
python3 acquire-model.py .native/model.onnx
binary=target/release/gitscry-encoder-gate
# Static musl proof: reject both ELF interpreter and shared-library requirements.
readelf -l "$binary" > .native/elf-program-headers.txt
readelf -d "$binary" > .native/elf-dynamic.txt
if test -n "$(sed -n '/INTERP/p' .native/elf-program-headers.txt)"; then
  echo 'FAIL: ELF interpreter present' >&2; exit 1
fi
if test -n "$(sed -n '/NEEDED/p' .native/elf-dynamic.txt)"; then
  echo 'FAIL: shared dependency present' >&2; exit 1
fi
# Copy to a fresh directory to exercise the executable, not a build-tree loader.
mkdir -p .native/installed
cp "$binary" .native/installed/gitscry-encoder-gate
.native/installed/gitscry-encoder-gate .native/model.onnx
