#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
target="x86_64-unknown-linux-gnu.2.17"
target_dir="${AHE_PORTABLE_TARGET_DIR:-$root/target-portable}"
binary="$target_dir/x86_64-unknown-linux-gnu/release/aurora-hak-explorer"

command -v cargo-zigbuild >/dev/null || {
  echo "cargo-zigbuild is required: cargo install cargo-zigbuild" >&2
  exit 1
}

(cd "$root" && cargo zigbuild --release --locked --target "$target" --target-dir "$target_dir")
"$root/packaging/check-linux-portability.sh" "$binary"
echo "$binary"
