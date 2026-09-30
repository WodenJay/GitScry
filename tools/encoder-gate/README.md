# CPU runtime feasibility gate (#111)

This is the **first**, executable portability gate from
[#109](https://github.com/WodenJay/GitScry/issues/109), not a completed encoder.
Do not proceed to cache/search integration or claim encoder parity until this
route passes. No product dependencies, commands, or initialization have changed.

## Pinned candidate

- Unmodified ONNX Runtime **1.28.0**, commit
  `da9b5e364c465de65c49d91e696cd6485270757f`, CPU-only, split static archives.
  Includes the upstream Abseil stacktrace fix (#28405); no extra compatibility
  dependency or source patch. This candidate was approved after 1.22.1 failed.
- Rust `ort` and `ort-sys` **2.0.0-rc.13**, default features disabled, explicit
  `std` and `api-27`; no vendor binary downloads, shared-library loading, or
  accelerator providers. Their Rust 1.88 minimum fits the project's Rust 1.89.
- Rust 1.89.0, Alpine 3.22 on GitHub's Ubuntu 22.04 runner. The container is used
  **only remotely**; do not run Linux containers, WSL, or a Linux emulator locally.
- FP32 `Qdrant/all-MiniLM-L6-v2-onnx` revision
  `8f518e882455312b086101e60691f5e6e2f05c3c`; model size **90,387,630** bytes,
  SHA-256 `bbd7b466f6d58e646fdc2bd5fd67b2f5e93c0b687011bd4548c420f7bd46f0c5`.

The probe executes `[CLS] hello [SEP]` (`101, 7592, 102`) in a release binary,
checks `[1, 3, 384]` FP32 finite output, mean pools the three non-padding tokens,
and L2-normalizes. **This does not establish tokenizer/document/query parity.**
The model is verified during explicit acquisition and again before Rust loading.
Acquisition uses Python 3.11+ standard-library code, not pip or a global install.

## Reproduce

Manually dispatch `.github/workflows/encoder-gate.yml` on the intended revision:

```powershell
gh workflow run encoder-gate.yml --repo WodenJay/GitScry --ref YOUR_BRANCH
```

The workflow must be registered on the default branch first. During development,
a temporary manual entry in the already registered `ci.yml` can dispatch the
same job on the task branch; it must be removed before merging. This does not
add a push/PR trigger to the expensive job. The final workflow remains manual.

`linux-musl.sh` builds upstream source without edits and with shared output,
native-machine compilation and global AVX/AVX2/AVX-512 disabled. Upstream MLAS
still includes CPU-dispatched specialized kernels; their presence does not mean
the baseline executable requires x86-64-v3. The script builds Rust with one Cargo
job and a static C++ standard library, rejects ELF `INTERP` and `NEEDED` entries,
and runs a copied executable outside its build output directory.
`onnxruntime_BUILD_UNIT_TESTS=OFF` disables upstream test/performance programs;
`--skip_tests` alone only skips their execution. Our release inference and ELF
dependency checks remain mandatory. No ORT source is modified.

`pack-runtime.py` combines the unmodified runtime and dependency object members
into one indexed `libonnxruntime.a` using GNU ar MRI mode. This uses ort-sys's
single-library interface rather than depending on its assumptions about split
archive directories. Only protobuf-lite is packaged; protobuf and protoc are
excluded. The input archive list is preserved as evidence.

Actions caches this native archive under the exact build/packaging script hashes,
without broad restore keys. Even on a cache hit the Rust release binary is rebuilt,
ELF dependencies checked, the model verified, and inference rerun. To force a fresh
source build, delete the corresponding Actions cache; no caching occurs locally.

On failure, preserve the Actions log/artifact and stop for a design decision.
Do not patch upstream, use GNU results as musl proof, change runtime/model, or
remove a release target. The previously tested 1.22.1 source includes
`execinfo.h`, absent on stock recent Alpine. The approved 1.28.0 candidate
removes that dependency upstream, but must still pass the complete gate.

## Remaining gates

- Actual successful static-musl build, link, and inference.
- Windows x64 and macOS ARM64 static release inference and native dependency
  checks; existing CPU/OS minimums and installation/update behavior.
- Reusable encoder at `src/analysis/retrieval/embedding/` with a narrow local
  interface, exact reference title/body/path protocol, 64/32/256 budgets,
  special tokens, dynamic padding, masked mean pooling, and 384-dimensional
  normalized vectors with correct input association.
- Reproducible Python reference token/vector fixtures (ordinary, Unicode and
  token boundaries, padding, and 256-token boundary), explicit numeric tolerance,
  and model-free normal tests. Extra reference-generation Python packages, if
  needed, belong in a tool-local uv project/venv, never the global environment.
- Full repository tests, format and warning-free Clippy; measured encoder costs.

## Evidence

### Previous candidate: 1.22.1 — failed
[36742848765](https://github.com/WodenJay/GitScry/actions/runs/36742848765)
on task-branch commit `44aa037` took **31m50s** and failed compiling the
unmodified upstream 1.22.1 source, before Rust linking or model acquisition:

```text
onnxruntime/core/platform/posix/stacktrace.cc:7:10:
fatal error: execinfo.h: No such file or directory
```

`Release`/`NDEBUG` does not eliminate this unconditional include. No source
patch, compatibility header, alternate backend, glibc artifact, or target removal
was attempted. This establishes failure of this pinned stock-Alpine route,
**not impossibility of every supported musl build configuration**. Any next route
requires a design decision before dependent implementation proceeds.

Evidence artifact: `encoder-gate-musl-36742848765` (14-day retention), containing
the full build log and CMake configuration. The exact source, environment,
commands, failure and run URL are recorded here so they outlive that artifact.
The task-branch bootstrap in `ci.yml` has been removed; existing automatic CI is
byte-for-byte unchanged from the starting commit.

Local checks: existing full suite **218 passed**, formatting check and
all-target/all-feature Clippy with one Cargo job and warnings denied passed.
The isolated probe passed formatting, typechecking and Clippy using `DOCS_RS=1`.
That mode supplies no native runtime and is **not** link/inference evidence.
No model/vector fixtures or production encoder were integrated because the
first portability gate failed. #111 remains open.

### Approved candidate: 1.28.0 — retrying library-only configuration

Manual run [36748881801](https://github.com/WodenJay/GitScry/actions/runs/36748881801)
on `64096b5` took **50m46s**. The former `execinfo.h` failure disappeared and
runtime static archives were produced, but upstream `onnxruntime_perf_test`
failed: `strings_helper.h:18:77: error: 'int64_t' was not declared in this scope`.
Rust linking and our inference probe had not yet run, so this is not a gate pass.
The retry uses the documented `onnxruntime_BUILD_UNIT_TESTS=OFF` option, not a
source patch or a different runtime/model. Evidence artifact:
`encoder-gate-musl-36748881801` (14-day retention).

Manual run [36755445118](https://github.com/WodenJay/GitScry/actions/runs/36755445118)
on `c801bed` took **48m42s**. All 1165 native build steps completed, followed by
successful probe Clippy. Release linking failed in ort-sys:
`error: could not find native static library \`onnx\`, perhaps an -L flag is missing?`.
The archive exists at `Release/_deps/onnx-build/libonnx.a`; the binding's first
matching split-library layout instead searches `ort/_deps/onnx-build/Release`.
The retry packages unmodified object members for the supported single-library
interface; no runtime/model version or source changes. Evidence artifact:
`encoder-gate-musl-36755445118` (14-day retention).
The temporary manual bootstrap is restored on the task branch for these retries
and will again be removed before merging.

Manual run [36762175611](https://github.com/WodenJay/GitScry/actions/runs/36762175611)
on `c4124ea` packaged **79** native archives successfully, then failed because
rustc does not search GCC's private static C++ library directory:
`error: could not find native static library \`stdc++\`, perhaps an -L flag is missing?`.
The completed native archive was cached. The retry explicitly discovers
`libstdc++.a` with the compiler and adds its directory to Rust's native search
path. The identical build recipe is now private in `build-runtime.sh`; a one-time
exact-key cache migration avoids rebuilding it and will be removed afterward.
