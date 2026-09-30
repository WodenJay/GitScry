# Offline encoder distribution gate (#111)

The reusable encoder lives at
[`src/analysis/retrieval/embedding/`](../../src/analysis/retrieval/embedding/README.md).
This tool uses that same module, not a second demonstration encoder. Indexing,
search, storage and CLI integration belong to dependent tickets; the product
manifest and existing automatic CI do not acquire or initialize model resources.

## Pinned route

- Unmodified ONNX Runtime **1.28.0**, commit
  `da9b5e364c465de65c49d91e696cd6485270757f`, CPU-only static archives.
  Includes the upstream Abseil stacktrace fix (#28405), with no compatibility
  dependency or source patch. This candidate was approved after 1.22.1 failed.
- Rust `ort` / `ort-sys` **2.0.0-rc.13**, default features disabled, explicit
  `std` / `api-27`, Rust **1.89.0**. No vendor binary downloads, `load-dynamic`,
  accelerator providers or runtime acquisition during inference.
- FP32 `Qdrant/all-MiniLM-L6-v2-onnx`, revision
  `8f518e882455312b086101e60691f5e6e2f05c3c`. Model size **90,387,630** bytes,
  SHA-256 `bbd7b466f6d58e646fdc2bd5fd67b2f5e93c0b687011bd4548c420f7bd46f0c5`.
  All six resources are pinned in the module's `resources.json` and verified
  before tokenizer/session creation. The vendored tokenizer is byte-preserved
  with `.gitattributes`, including on Windows checkouts.
- Python acquisition/build scripts use only the standard library. Independent
  reference generation uses the tool-local uv project, lockfile and `.venv`;
  it does not install into the global environment or download from inference.

## Reproduce (manual only)

After this workflow is registered on the default branch:

```powershell
gh workflow run encoder-gate.yml --repo WodenJay/GitScry --ref YOUR_BRANCH
$log = Join-Path $env:TEMP 'encoder-gate-watch.log'
gh run watch RUN_ID --repo WodenJay/GitScry --exit-status --interval 60 *> $log
Get-Content $log -Tail 10
```

Allow up to 90 minutes for an uncached native build; use a 3600-second watch
command timeout and resume watching if needed. Search the saved logs with `rg`
before reading them in full. The final workflow has **only `workflow_dispatch`**,
no push/PR trigger or reusable automatic entry. Existing `ci.yml` is unchanged.
A temporary task-branch dispatch entry was used during development and removed.

Linux is exercised **only remotely**: GitHub's Ubuntu runner runs the pinned
Alpine 3.22 / Rust 1.89 container. Never use local WSL, Docker or Linux emulation
for this gate. Desktop runners are Windows x64 MSVC and macOS ARM64.

## What passes mean

- Build upstream without edits, with shared output and native-machine compilation
  disabled. Explicitly disable global AVX/AVX2/AVX-512 and accelerator providers.
  CPU-dispatched MLAS kernels do not raise the baseline to x86-64-v3. Windows
  retains Rust's Windows 10 baseline; macOS has an explicit **11.0** deployment
  target checked against the executable's Mach-O metadata. CI does not claim
  testing every historical CPU or physically running an old OS installation.
- Disable upstream test/performance programs with the documented
  `onnxruntime_BUILD_UNIT_TESTS=OFF`; our release tests remain mandatory.
- Build `re2` explicitly; package unmodified runtime/dependency object members
  with GNU ar MRI (musl), MSVC lib (Windows), or libtool (macOS). Require nested
  `model_package`, re2 and protobuf-lite. Exclude full protobuf and protoc.
  Preserve the input archive list. Windows uses static CRT and explicitly selects
  MSVC's linker rather than Git Bash's unrelated `link.exe`.
- Reject ELF `INTERP` / `NEEDED`, non-system Windows DLLs (including shared ORT /
  CRT), and non-system macOS dylibs. Copy the executable into a fresh installation
  directory and run it with verified, explicitly supplied model resources, without
  build-tool/runtime lookup paths. This exercises the single-executable packaging
  route, not an integration test of the product's future installer/updater.
- Compare independent Python token/vector fixtures: ordinary/Unicode/malformed
  input, field budgets, split WordPieces, empty queries and 254/255 query boundaries.
  Reverse/repeat identities over batch boundaries, compare single vs padded batches,
  check unit length and reject corrupt resources without repairing/downloading them.
  Require maximum absolute error **<=3e-5**, cosine distance **<=1e-6**, and norm
  error **<=1e-5**; FP32 parity is deliberately not byte equality.
- Check formatting, warning-free Clippy and encoder unit tests on all targets,
  with one Cargo job. Model-free tests are separately available with
  `cargo test --manifest-path src/analysis/retrieval/embedding/Cargo.toml --locked --no-default-features --jobs 1`.

The harness records executable/model bytes, first index-session startup (including
resource hash verification), a 13-document batch and reference-query durations.
Linux also reports process peak resident memory (`VmHWM`) before the deliberate
corrupt-resource copy. These are CI observations, not portable benchmark guarantees;
resource files remain separate from the executable and are not shipped in Git.

Native archives are cached under exact source-build/packaging script hashes with
no broad restore keys. On a hit, rebuild the Rust release executable and rerun all
resource/dependency/parity checks. Delete the exact cache to force a native rebuild.
Logs, CMake configuration, native dependency inspections and archive inputs are
uploaded as 14-day artifacts; this document records durable results after expiry.
On a runtime portability failure, stop for a design decision rather than patch
upstream, substitute GNU proof for musl, or silently change model/backend/targets.

## Evidence

[Manual run 36785538394](https://github.com/WodenJay/GitScry/actions/runs/36785538394),
commit `02620c095f13608f7763f2b8cdb4e4972821544d`, passed the same reusable encoder's
release inference, format/Clippy/tests, native dependency inspections and independent
vector parity on **Linux musl, Windows x64 and macOS ARM64**. Artifact names:
`encoder-gate-musl-36785538394`, `encoder-gate-windows-2022-36785538394`,
`encoder-gate-macos-14-36785538394`.

[Cost/parity run 36786936218](https://github.com/WodenJay/GitScry/actions/runs/36786936218),
commit `e27c2e520fc8272e72dacc7aedd1d44115a4f83d`, also passed all three targets.
Model size was 90,387,630 bytes on each target. Observed costs:

| Target | Executable bytes | Index startup ms | 13-document batch ms | Reference queries ms | Maximum component error |
| --- | ---: | ---: | ---: | ---: | ---: |
| Linux musl x64 | 40,164,576 | 285.302 | 459.876 | 99.085 | 1.42e-7 |
| Windows x64 | 22,394,368 | 262.379 | 520.211 | 112.842 | 1.42e-7 |
| macOS ARM64 | 32,039,216 | 509.795 | 941.096 | 171.106 | 1.97e-7 |

Linux process peak resident memory was **198,772 kB**. Platform runner hardware
and cold-cache state differ; these timings are not cross-platform benchmarks.
Artifacts: `encoder-gate-musl-36786936218`, `encoder-gate-windows-2022-36786936218`,
`encoder-gate-macos-14-36786936218`.

The existing repository suite passes **218 tests**; model-free encoder tests pass
**8 tests**. Packaging regressions cover nested dependencies, Windows prefixed
protobuf exclusion, allowed system DLLs and rejected shared runtime/CRT sidecars.
One review batch found the missing 32-window query cap; its regression failed,
then passed after adding early rejection with shortening guidance.

### Failed attempts and lessons

| Run | Outcome and correction |
| --- | --- |
| [36742848765](https://github.com/WodenJay/GitScry/actions/runs/36742848765) | ORT 1.22.1, 31m50s: `fatal error: execinfo.h: No such file or directory`. Stopped and obtained approval for upstream-fixed 1.28.0; no compatibility patch. |
| [36748881801](https://github.com/WodenJay/GitScry/actions/runs/36748881801) | 50m46s: optional `onnxruntime_perf_test` failed (`int64_t` missing); disabled upstream test programs with their documented option. |
| [36755445118](https://github.com/WodenJay/GitScry/actions/runs/36755445118) | 48m42s: native build complete, ort-sys split-layout lookup missed `onnx`; used its supported combined-archive interface. |
| [36762175611](https://github.com/WodenJay/GitScry/actions/runs/36762175611) | Static `stdc++` search path missing; discover it through the compiler. |
| [36765627642](https://github.com/WodenJay/GitScry/actions/runs/36765627642) | Nested `model_package` and unbuilt `re2` missing; explicit target and recursive discovery, regression first. |
| [36766373346](https://github.com/WodenJay/GitScry/actions/runs/36766373346) | First successful musl native link/probe, before canonical encoder parity. |
| [36778249285](https://github.com/WodenJay/GitScry/actions/runs/36778249285) | Canonical encoder passed musl/macOS; Windows selected Git Bash linker. Explicit MSVC selection and protobuf-lite-only archive regression. |
| [36781157557](https://github.com/WodenJay/GitScry/actions/runs/36781157557) | Windows checksum test detected Git CRLF conversion (742346 vs 711661 bytes); byte-preserving tokenizer attribute. |
| [36784584164](https://github.com/WodenJay/GitScry/actions/runs/36784584164) | Strict DLL audit omitted Windows system `bcryptprimitives`, `setupapi`, `dxgi`; accept those explicitly while regression still rejects ORT/CRT sidecars. |

No upstream code, model revision or release target changed during configuration/
packaging corrections. No local Linux emulation or global Python install was used.
