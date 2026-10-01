#!/bin/sh
set -eu

repository_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
output_dir="$repository_root/target/gitscry-installers"
mkdir -p "$output_dir"
cp "$repository_root/scripts/gitscry-installer.sh" "$output_dir/gitscry-installer.sh"
cp "$repository_root/scripts/gitscry-installer.ps1" "$output_dir/gitscry-installer.ps1"
