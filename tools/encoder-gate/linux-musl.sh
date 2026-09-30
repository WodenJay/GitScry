#!/usr/bin/env sh
# Run ONLY on the remote Actions runner, inside its Alpine container.
set -eu
cd "$(dirname "$0")"
apk add --no-cache build-base cmake ninja git python3 linux-headers curl
mkdir -p .native
cd .native
if ! test -f ort-packed/libonnxruntime.a; then
  sh ../build-runtime.sh
fi
export ORT_LIB_LOCATION="$PWD/ort-packed"
export ORT_PREFER_DYNAMIC_LINK=0
export ORT_CXX_STDLIB=static=stdc++
# rustc does not search GCC's private library directory automatically.
cpp_archive="$(c++ -print-file-name=libstdc++.a)"
test -f "$cpp_archive"
export RUSTFLAGS="${RUSTFLAGS:-} -L native=$(dirname "$cpp_archive")"
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
