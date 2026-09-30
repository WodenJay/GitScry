#!/usr/bin/env python3
"""Package unmodified ORT archives for ort-sys's single-library interface.

Run on the remote musl runner only. GNU ar's MRI mode preserves all object
members, including equal member names from different libraries. No source
files or binary object code are patched.
"""
import pathlib
import subprocess
import sys


def main():
    build = pathlib.Path(sys.argv[1]).resolve()
    output = pathlib.Path(sys.argv[2]).resolve()
    required = {
        f"libonnxruntime_{name}.a" for name in
        ("common", "flatbuffers", "framework", "graph", "lora", "mlas",
         "optimizer", "providers", "session", "util")
    }
    core = sorted(build.glob("libonnxruntime_*.a"))
    if not required.issubset({archive.name for archive in core}):
        raise RuntimeError("Incomplete ONNX Runtime static build")
    # ORT uses protobuf-lite. Including both implementations would introduce
    # duplicate symbols; protoc is a build-time generator, not a runtime library.
    excluded = {"libprotobuf.a", "libprotoc.a"}
    dependencies = sorted(
        archive for archive in build.rglob("lib*.a")
        if archive not in core and archive.name not in excluded
    )
    required_dependencies = {"libre2.a", "libmodel_package.a", "libprotobuf-lite.a"}
    if not required_dependencies.issubset({archive.name for archive in dependencies}):
        raise RuntimeError("Missing ONNX Runtime dependency archives")
    output.mkdir(parents=True, exist_ok=True)
    destination = output / "libonnxruntime.a"
    archives = core + dependencies
    if any(any(c.isspace() for c in str(path)) for path in archives + [destination]):
        raise RuntimeError("MRI archive paths must not contain whitespace")
    instructions = [f"create {destination}"]
    instructions += [f"addlib {archive}" for archive in archives]
    instructions += ["save", "end", ""]
    subprocess.run(["ar", "-M"], input="\n".join(instructions), text=True, check=True)
    subprocess.run(["ar", "s", str(destination)], check=True)
    (output / "archive-inputs.txt").write_text(
        "\n".join(path.relative_to(build).as_posix() for path in archives) + "\n",
        encoding="utf-8",
    )
    print(f"Packed {len(archives)} unchanged static archives into {destination}")


if __name__ == "__main__":
    main()
