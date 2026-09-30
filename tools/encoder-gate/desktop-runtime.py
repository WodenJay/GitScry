"""Unmodified pinned CPU ORT source build for remote Windows/macOS runners.

Python standard library only. Windows uses /MT and baseline x64; macOS builds
ARM64 with deployment target 11.0. No vendor prebuilt, source edits, or GPU.
"""
from pathlib import Path
import subprocess
import sys

REVISION = "da9b5e364c465de65c49d91e696cd6485270757f"  # ORT 1.28.0
ROOT = Path(__file__).resolve().parent
NATIVE = ROOT / ".native"
SOURCE = NATIVE / "onnxruntime"
BUILD = NATIVE / "ort"
PACKED = NATIVE / "ort-packed"
WINDOWS = sys.platform == "win32"


def run(*args, **kwargs):
    subprocess.run([str(arg) for arg in args], check=True, **kwargs)


def main():
    if (PACKED / ("onnxruntime.lib" if WINDOWS else "libonnxruntime.a")).exists():
        return
    NATIVE.mkdir(exist_ok=True)
    run("git", "init", SOURCE)
    run("git", "-C", SOURCE, "remote", "add", "origin", "https://github.com/microsoft/onnxruntime.git")
    run("git", "-C", SOURCE, "fetch", "--depth", "1", "origin", REVISION)
    run("git", "-C", SOURCE, "checkout", "--detach", "FETCH_HEAD")
    run("git", "-C", SOURCE, "submodule", "update", "--init", "--recursive")
    options = ["--enable_msvc_static_runtime"] if WINDOWS else []
    definitions = ["onnxruntime_BUILD_SHARED_LIB=OFF", "onnxruntime_BUILD_UNIT_TESTS=OFF",
                   "onnxruntime_BUILD_FOR_NATIVE_MACHINE=OFF", "onnxruntime_USE_AVX=OFF",
                   "onnxruntime_USE_AVX2=OFF", "onnxruntime_USE_AVX512=OFF"]
    if not WINDOWS:
        definitions += ["CMAKE_OSX_ARCHITECTURES=arm64", "CMAKE_OSX_DEPLOYMENT_TARGET=11.0"]
    run(sys.executable, SOURCE / "tools/ci_build/build.py", "--build_dir", BUILD,
        "--config", "Release", "--update", "--build", "--skip_tests", "--skip_submodule_sync",
        "--parallel", "2", "--cmake_generator", "Ninja", *options,
        "--cmake_extra_defines", *definitions)
    run("cmake", "--build", BUILD / "Release", "--target", "re2", "--parallel", "2")
    run("git", "-C", SOURCE, "diff", "--exit-code")
    suffix = ".lib" if WINDOWS else ".a"
    excluded = {"protobuf.lib", "libprotobuf.a", "protoc.lib", "libprotoc.a"}
    archives = sorted(p for p in (BUILD / "Release").rglob(f"*{suffix}") if p.name not in excluded)
    # Do not report a partial source build as a distributable runtime.
    names = {p.name.removeprefix("lib").removesuffix(suffix) for p in archives}
    required = {"onnxruntime_common", "onnxruntime_framework", "onnxruntime_session",
                "onnxruntime_providers", "re2", "model_package", "protobuf-lite"}
    if not required.issubset(names):
        raise RuntimeError(f"Missing static dependencies: {required - names}")
    PACKED.mkdir(exist_ok=True)
    if WINDOWS:
        response = PACKED / "archives.rsp"
        response.write_text("\n".join(f'"{p}"' for p in archives), encoding="utf-8")
        run("lib.exe", "/NOLOGO", f"/OUT:{PACKED / 'onnxruntime.lib'}", f"@{response}")
    else:
        run("libtool", "-static", "-o", PACKED / "libonnxruntime.a", *archives)
    (PACKED / "archive-inputs.txt").write_text("\n".join(p.relative_to(BUILD).as_posix() for p in archives) + "\n")
    print(f"Packed {len(archives)} unchanged static archives")


if __name__ == "__main__":
    main()
