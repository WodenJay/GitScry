# Installing GitScry

## Official release

Official release installers support Windows x86-64, macOS Apple Silicon, and Linux x86-64 with GNU libc (glibc). They verify the release archive's SHA-256 checksum and install the executable together with its pinned ONNX Runtime files.
Linux x86-64 packages are built and validated on Ubuntu 22.04 (glibc 2.35); compatibility with older glibc releases is not established.

```powershell
# Windows x86-64
powershell -ExecutionPolicy Bypass -c "irm https://github.com/WodenJay/GitScry/releases/latest/download/gitscry-installer.ps1 | iex"
```

```sh
# macOS Apple Silicon or Linux x86-64 (glibc)
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/WodenJay/GitScry/releases/latest/download/gitscry-installer.sh | sh
```

By default, the installers place GitScry in `~/.gitscry/bin` (Windows: `%USERPROFILE%\.gitscry\bin`). The Windows installer adds this location to the user PATH. On Linux and macOS, add `~/.gitscry/bin` to your shell PATH if it is not already there. Set `GITSCRY_INSTALL_DIR` to use another installation directory.

The platform archives are also available on the [GitHub Releases page](https://github.com/WodenJay/GitScry/releases/latest). Extract the complete archive, keeping its `runtime/` directory beside `gitscry` (`gitscry.exe` on Windows); the runtime files and manifest must not be moved independently.

## Cargo installation

```sh
cargo install gitscry
```

`cargo install` builds GitScry from source and does not bundle ONNX Runtime. Non-semantic commands work normally, but semantic indexing requires a prepared native runtime.

## Building from source with semantic indexing

From a GitScry checkout, prepare the verified runtime for the current supported native platform, then build or run the application:

```sh
cargo run --manifest-path xtask/Cargo.toml -- prepare-runtime
cargo build --release
```

The preparation command downloads only the pinned ONNX Runtime package for the current platform, verifies its archive and every packaged file, and places the runtime under `runtime/`. It does not download the semantic model; model resources are handled by GitScry separately.
