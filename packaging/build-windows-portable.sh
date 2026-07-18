#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$root/Cargo.toml" | head -1)"
binary="${1:-$root/target/x86_64-pc-windows-msvc/release/aurora-hak-explorer.exe}"
output="${2:-$root/build/Aurora-Hak-Explorer-${version}-Windows-x86_64.zip}"

test -f "$binary"
mkdir -p "$root/build"
package="$(mktemp -d "$root/build/Windows-${version}.XXXXXX")"
trap 'find "$package" -depth -delete' EXIT

install -Dm755 "$binary" "$package/Aurora-Hak-Explorer.exe"
install -Dm644 "$root/CHANGELOG.md" "$package/CHANGELOG.md"

rm -f "$output"
(cd "$package" && zip -q -9 -r "$output" .)
echo "$output"
