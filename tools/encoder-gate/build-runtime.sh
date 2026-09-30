#!/usr/bin/env sh
# Internal source-build recipe. Called from .native on the remote musl runner.
set -eu
# Upstream 1.28.0 includes the Abseil stacktrace fix; no source patches.
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
    onnxruntime_BUILD_UNIT_TESTS=OFF \
    onnxruntime_BUILD_FOR_NATIVE_MACHINE=OFF \
    onnxruntime_USE_AVX=OFF onnxruntime_USE_AVX2=OFF onnxruntime_USE_AVX512=OFF
# re2 is EXCLUDE_FROM_ALL and split static libraries do not pull it in.
cmake --build ort/Release --target re2 --parallel 2
# Never accept a silently patched runtime checkout.
git -C onnxruntime diff --exit-code
python3 ../pack-runtime.py ort/Release ort-packed
