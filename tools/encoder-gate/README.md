# CPU runtime feasibility gate (#111)

This is the **first**, executable portability gate from
[#109](https://github.com/WodenJay/GitScry/issues/109), not a completed encoder.
Do not proceed to cache/search integration or claim encoder parity until this
route passes. No product dependencies, commands, or initialization have changed.

## Pinned candidate

- Unmodified ONNX Runtime **1.22.1**, commit
  `89746dc19a0a1ae59ebf4b16df9acab8f99f3925`, CPU-only, split static archives.
  This upstream patch release avoids 1.22.0's broken Eigen archive digest.
- Rust `ort` and `ort-sys` **2.0.0-rc.10**, default features disabled; no vendor
  binary downloads, shared-library loading, or accelerator providers.
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

On failure, preserve the Actions log/artifact and stop for a design decision.
Do not patch upstream, use GNU results as musl proof, change runtime/model, or
remove a release target. In particular, upstream POSIX `stacktrace.cc` includes
`execinfo.h`, absent on stock recent Alpine; this is an unresolved source-build
risk, not something this harness hides with a generated compatibility header.

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

**FAILED / design decision required.** Manual run
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
