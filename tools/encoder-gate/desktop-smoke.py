"""Release inference + native dependency checks on remote desktop runners."""
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parent
MODULE = ROOT.parents[1] / "src/analysis/retrieval/embedding/Cargo.toml"
WINDOWS = sys.platform == "win32"


def run(*args, **kwargs):
    return subprocess.run([str(arg) for arg in args], check=True, **kwargs)


def main():
    os.chdir(ROOT)
    os.environ["ORT_LIB_LOCATION"] = str(ROOT / ".native/ort-packed")
    os.environ["ORT_PREFER_DYNAMIC_LINK"] = "0"
    if WINDOWS:
        os.environ["RUSTFLAGS"] = "-C target-feature=+crt-static"
    else:
        os.environ["MACOSX_DEPLOYMENT_TARGET"] = "11.0"
    for manifest in [ROOT / "Cargo.toml", MODULE]:
        run("cargo", "fmt", "--manifest-path", manifest, "--all", "--", "--check")
        run("cargo", "clippy", "--manifest-path", manifest, "--locked", "--all-targets", "--all-features", "--jobs", "1", "--", "-D", "warnings")
    run("cargo", "test", "--manifest-path", MODULE, "--locked", "--all-features", "--jobs", "1")
    run("cargo", "build", "--locked", "--release", "--jobs", "1")
    run(sys.executable, ROOT / "acquire-model.py", ROOT / ".native/resources")
    binary = ROOT / "target/release" / ("gitscry-encoder-gate.exe" if WINDOWS else "gitscry-encoder-gate")
    command = ["dumpbin", "/DEPENDENTS", str(binary)] if WINDOWS else ["otool", "-L", str(binary)]
    dependencies = run(*command, capture_output=True, text=True).stdout
    (ROOT / ".native/dependencies.txt").write_text(dependencies)
    print(dependencies)
    if WINDOWS:
        system = {"kernel32.dll", "ntdll.dll", "advapi32.dll", "bcrypt.dll", "crypt32.dll",
                  "ole32.dll", "oleaut32.dll", "userenv.dll", "ws2_32.dll", "shell32.dll",
                  "shlwapi.dll", "secur32.dll", "user32.dll", "gdi32.dll", "dbghelp.dll"}
        libraries = [line.strip().lower() for line in dependencies.splitlines() if line.strip().lower().endswith(".dll")]
        if not libraries or any(lib not in system and not lib.startswith("api-ms-win-") for lib in libraries):
            raise RuntimeError(f"Non-system DLL dependency: {libraries}")
    else:
        libraries = [line.strip().split(" ")[0] for line in dependencies.splitlines()[1:]]
        if not libraries or any(not lib.startswith(("/usr/lib/", "/System/Library/")) for lib in libraries):
            raise RuntimeError(f"Non-system dylib dependency: {libraries}")
        build = run("vtool", "-show-build", binary, capture_output=True, text=True).stdout
        (ROOT / ".native/macos-minimum.txt").write_text(build)
        versions = re.findall(r"minos\s+(\d+)\.(\d+)", build)
        if not versions or any((int(major), int(minor)) > (11, 0) for major, minor in versions):
            raise RuntimeError(f"Raised macOS deployment minimum: {build}")
    installed = ROOT / ".native/installed"
    installed.mkdir(exist_ok=True)
    executable = installed / binary.name
    shutil.copy2(binary, executable)
    # Only the executable and model resources remain visible to runtime lookup.
    environment = os.environ.copy()
    environment.pop("ORT_LIB_LOCATION", None)
    environment["PATH"] = str(Path(os.environ["SystemRoot"]) / "System32") if WINDOWS else "/usr/bin:/bin"
    run(executable, ROOT / ".native/resources", cwd=installed, env=environment)


if __name__ == "__main__":
    main()
