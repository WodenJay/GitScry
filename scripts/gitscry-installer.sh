#!/bin/sh
set -eu

REPOSITORY="WodenJay/GitScry"
RELEASE_BASE="https://github.com/${REPOSITORY}/releases/latest/download"

case "$(uname -s):$(uname -m)" in
    Darwin:arm64|Darwin:aarch64)
        target="aarch64-apple-darwin"
        ;;
    Linux:x86_64|Linux:amd64)
        libc_version=$(getconf GNU_LIBC_VERSION 2>/dev/null || true)
        if [ -z "$libc_version" ]; then
            ldd_version=$(ldd --version 2>&1 | head -n 1 || true)
            case "$ldd_version" in
                *glibc*|*GLIBC*|*"GNU libc"*) ;;
                *) echo "GitScry release installers require x86-64 GNU libc (glibc)." >&2; exit 1 ;;
            esac
        fi
        target="x86_64-unknown-linux-gnu"
        ;;
    *)
        echo "GitScry has no release installer for $(uname -s) $(uname -m)." >&2
        exit 1
        ;;
esac

if [ -z "${HOME:-}" ]; then
    echo "HOME must be set to install GitScry." >&2
    exit 1
fi

archive="gitscry-${target}.tar.xz"
install_dir=${GITSCRY_INSTALL_DIR:-"$HOME/.gitscry/bin"}
download_dir=$(mktemp -d "${TMPDIR:-/tmp}/gitscry-install.XXXXXX")
stage_dir=

cleanup() {
    rm -rf "$download_dir"
    if [ -n "$stage_dir" ]; then
        rm -rf "$stage_dir"
    fi
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

download() {
    url=$1
    destination=$2
    if command -v curl >/dev/null 2>&1; then
        curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location \
            "$url" --output "$destination"
    elif command -v wget >/dev/null 2>&1; then
        wget --https-only --quiet --output-document="$destination" "$url"
    else
        echo "Install curl or wget to download GitScry." >&2
        return 1
    fi
}

download_archive() {
    download "$RELEASE_BASE/$archive" "$download_dir/$archive"
    download "$RELEASE_BASE/$archive.sha256" "$download_dir/$archive.sha256"
}

download_archive
expected_sha=$(awk -v asset="$archive" '($2 == asset) || ($2 == ("*" asset)) { print $1; exit }' \
    "$download_dir/$archive.sha256")
case "$expected_sha" in
    *[!0123456789abcdefABCDEF]*|"")
        echo "The release checksum file does not contain a SHA-256 for $archive." >&2
        exit 1
        ;;
esac
if [ "${#expected_sha}" -ne 64 ]; then
    echo "The release checksum for $archive is malformed." >&2
    exit 1
fi
if command -v sha256sum >/dev/null 2>&1; then
    actual_sha=$(sha256sum "$download_dir/$archive" | awk '{ print $1 }')
elif command -v shasum >/dev/null 2>&1; then
    actual_sha=$(shasum -a 256 "$download_dir/$archive" | awk '{ print $1 }')
else
    echo "Install sha256sum or shasum to verify the GitScry download." >&2
    exit 1
fi
if [ "$(printf '%s' "$expected_sha" | tr '[:upper:]' '[:lower:]')" != "$actual_sha" ]; then
    echo "SHA-256 verification failed for $archive." >&2
    exit 1
fi

mkdir "$download_dir/extracted"
tar -xf "$download_dir/$archive" -C "$download_dir/extracted"
package_dir="$download_dir/extracted/gitscry-${target}"
if [ ! -x "$package_dir/gitscry" ] || [ ! -d "$package_dir/runtime" ]; then
    echo "The verified GitScry archive is missing its executable or ONNX Runtime package." >&2
    exit 1
fi

mkdir -p "$install_dir"
stage_dir=$(mktemp -d "$install_dir/.gitscry-install.XXXXXX")
mkdir -p "$install_dir/runtime"
cp -R "$package_dir/runtime/." "$install_dir/runtime/"
cp "$package_dir/gitscry" "$stage_dir/gitscry"
chmod 755 "$stage_dir/gitscry"
# Publish runtime files before replacing the executable, so it never points at an absent runtime.
mv -f "$stage_dir/gitscry" "$install_dir/gitscry"

printf 'GitScry installed to %s/gitscry.\n' "$install_dir"
case ":${PATH:-}:" in
    *":$install_dir:"*) ;;
    *) printf 'Add %s to PATH to run gitscry from any directory.\n' "$install_dir" ;;
esac
