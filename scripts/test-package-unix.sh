#!/bin/sh
set -eu

if [ "$#" -ne 3 ]; then
    echo "usage: $0 <target> <archive> <checksum>" >&2
    exit 2
fi

target=$1
archive=$2
checksum=$3
case "$target" in
    aarch64-apple-darwin) runtime_library=libonnxruntime.1.23.2.dylib ;;
    x86_64-unknown-linux-gnu) runtime_library=libonnxruntime.so.1.23.2 ;;
    *) echo "unsupported Unix package target: $target" >&2; exit 2 ;;
esac

version=$(awk -F '"' '/^version = "/ { print $2; exit }' Cargo.toml)
test -n "$version"
test -f "$archive"
test -f "$checksum"
expected_hash=$(awk 'NR == 1 { print $1 }' "$checksum")
if command -v sha256sum >/dev/null 2>&1; then
    actual_hash=$(sha256sum "$archive" | awk '{ print $1 }')
else
    actual_hash=$(shasum -a 256 "$archive" | awk '{ print $1 }')
fi
if [ "$actual_hash" != "$expected_hash" ]; then
    echo "archive checksum mismatch for $target: expected $expected_hash, got $actual_hash" >&2
    exit 1
fi
printf 'Verified package target=%s platform=%s/%s archive=%s sha256=%s\n' "$target" "$(uname -s)" "$(uname -m)" "$archive" "$actual_hash"

scratch=$(mktemp -d "${TMPDIR:-/tmp}/GitScry package test.XXXXXX")
mock_bin="$scratch/mock-bin"
unpacked="$scratch/unpacked"
smoke_repo="$scratch/smoke repository"
install_dir="$scratch/installed package/bin"
mkdir -p "$mock_bin" "$unpacked" "$smoke_repo"
trap 'rm -rf "$scratch"' EXIT HUP INT TERM

cat > "$mock_bin/curl" <<'MOCK_CURL'
#!/bin/sh
set -eu
output=
url=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --output) output=$2; shift 2 ;;
        --proto) shift 2 ;;
        --*) shift ;;
        *) url=$1; shift ;;
    esac
done
case "$url" in
    *.sha256) source=$GITSCRY_TEST_CHECKSUM ;;
    *) source=$GITSCRY_TEST_ARCHIVE ;;
esac
cp "$source" "$output"
MOCK_CURL
chmod 755 "$mock_bin/curl"

package_root="$unpacked/gitscry-$target"
tar -xf "$archive" -C "$unpacked"
test -x "$package_root/gitscry"
test -f "$package_root/runtime/manifests/$version.json"
find "$package_root/runtime" -type f -name "$runtime_library" -print -quit | grep -q .

export GITSCRY_TEST_ARCHIVE="$archive"
export GITSCRY_TEST_CHECKSUM="$checksum"
export GITSCRY_INSTALL_DIR="$install_dir"
export TMPDIR="$scratch"
PATH="$mock_bin:$PATH" ./scripts/gitscry-installer.sh
test -x "$install_dir/gitscry"
test -f "$install_dir/runtime/manifests/$version.json"
find "$install_dir/runtime" -type f -name "$runtime_library" -print -quit | grep -q .

# Both the archive and the shipped installer must produce an executable package.
git -C "$smoke_repo" init -q -b main
printf 'first packaged inference\n' > "$smoke_repo/fixture.txt"
git -C "$smoke_repo" add fixture.txt
git -C "$smoke_repo" -c user.name=Smoke -c user.email=smoke@example.invalid commit -qm initial
(cd "$smoke_repo" && "$install_dir/gitscry" index --semantic)
printf 'second offline inference\n' >> "$smoke_repo/fixture.txt"
git -C "$smoke_repo" add fixture.txt
git -C "$smoke_repo" -c user.name=Smoke -c user.email=smoke@example.invalid commit -qm second
(cd "$smoke_repo" && HTTP_PROXY=http://127.0.0.1:9 HTTPS_PROXY=http://127.0.0.1:9 ALL_PROXY=http://127.0.0.1:9 http_proxy=http://127.0.0.1:9 https_proxy=http://127.0.0.1:9 all_proxy=http://127.0.0.1:9 "$install_dir/gitscry" index --semantic)
