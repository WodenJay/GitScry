# Install GitScry

Use this guide to install the current GitScry release without administrator access. Choose one method for the environment where GitScry will run.

## Prebuilt release

Use the prebuilt installer when Rust is not already available.

### Linux or macOS

Run the public shell installer:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/WodenJay/GitScry/releases/latest/download/gitscry-installer.sh | sh
```

The installer selects the supported binary for the machine, verifies its SHA-256 checksum, installs it under `~/.gitscry/bin`, and adds that directory to the user's PATH when possible. Open a new shell before verifying the command.

### Windows

Run PowerShell as the current user and execute:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/WodenJay/GitScry/releases/latest/download/gitscry-installer.ps1 | iex"
```

The installer selects the x86-64 Windows binary, verifies its SHA-256 checksum, installs it under the user's `.gitscry\bin` directory, and updates the user-level PATH. Open a new PowerShell window before verifying the command.

## Cargo

Use Cargo when Rust 1.89 or newer is installed:

```sh
cargo install gitscry --locked
```

This is the standard source-installation path and works independently of the prebuilt target matrix.

## Verify the installation

Run:

```sh
gitscry --version
```

For the first public release, the output should report `gitscry 0.1.0`. If the command is not found, open a new shell and confirm that the user-level `.gitscry/bin` directory is on PATH. Do not use `sudo` or an administrator shell to work around a PATH problem.

## Upgrade

Run `gitscry update` to install the latest stable release, or use `gitscry upgrade` as its exact alias. The command downloads the official binary for supported Apple Silicon macOS, x86-64 musl Linux, or x86-64 MSVC Windows targets, verifies its published SHA-256 checksum, and replaces the executable that is actually running. A Cargo-built installation becomes the official prebuilt binary after a successful update. Updates do not modify repositories, configuration, or the rebuildable cache, and never request elevation. There is no separate uninstaller.

## Manual artifact verification

If an archive is downloaded instead of using an installer, download the matching `.sha256` file from the same GitHub Release and verify the archive before unpacking it. On Linux, use `sha256sum -c`; on macOS, use `shasum -a 256 -c`; on Windows, use PowerShell's `Get-FileHash -Algorithm SHA256` and compare the result with the published checksum.
