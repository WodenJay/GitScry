# Bundle precompiled CPU ONNX Runtime instead of static musl builds

GitScry's semantic retrieval roadmap originally required Linux musl and a statically embedded ONNX Runtime in each platform's executable; the archived runtime experiment demonstrated feasibility but also exposed substantial cross-platform source-build, linking and maintenance work. We instead target Windows x64, macOS ARM64 and Linux x64 glibc with a pinned, verified precompiled CPU ORT bundled in each official release, preserving the fixed MiniLM resources and local/offline retrieval design. This deliberately trades single-file portability for maintaining a small runtime distribution contract rather than a cross-platform C++ build system; the concrete runtime version and minimum glibc/CPU/OS requirements remain verification gates, not assumed results.

## Consequences

- The executable and runtime form one installation/update unit. Runtime location, package layout, installers and failure-safe version-consistent updates belong to the distribution task, not the runtime/encoder verification experiment.
- ORT loads only for semantic operations. Non-semantic commands work without it; explicit semantic requests fail clearly rather than silently falling back. Full semantic support is guaranteed for official release packages, not bare `cargo install` executables; there is no selectable/system runtime, runtime downloader or source-build fallback.
- ORT version participates in the encoder fingerprint. A change invalidates old semantic readiness and requires `index --semantic`, without discarding ordinary cache or reacquiring unchanged shared model resources. Cross-platform parity tolerance is not a runtime-version compatibility exemption.
- Ordinary CI uses Rust logic, compact fixtures/parity, cache lifecycle and a small single-platform real-inference smoke. Full three-platform runtime/package verification runs for the initial switch, releases and inference/distribution-affecting changes; ORT is not built from source.
- Linux and macOS verification uses matching native GitHub Actions runners (Linux x64 glibc, macOS ARM64), recording run URLs and tested artifact/environment identities. Local Linux/macOS simulation, emulation, local containers or cross-compilation do not substitute for platform evidence.

## Roadmap ownership

- [Verify CPU runtime and encoder parity](https://github.com/WodenJay/GitScry/issues/111) verifies only precompiled runtime loading, real inference and embedding parity with the fixed MiniLM on three platforms, and records compatibility evidence.
- [Bundle CPU runtime and update full releases](https://github.com/WodenJay/GitScry/issues/121) implements package layout, runtime location, installation, updates and version consistency.
- [Validate release workflow and performance](https://github.com/WodenJay/GitScry/issues/116) owns final integrated acceptance; [Explore local semantic retrieval for Git history](https://github.com/WodenJay/GitScry/issues/109) remains the canonical feature specification.

This records an accepted roadmap decision, not a claim that the current executable, installer or CI already implements it. The earlier static experiments remain historical evidence and do not satisfy the new dynamic-package release gate.
