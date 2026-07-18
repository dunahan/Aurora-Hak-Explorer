#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$root/Cargo.toml" | head -1)"
appimage="${1:-$root/build/Aurora-Hak-Explorer-${version}-x86_64.AppImage}"
output="${2:-$root/build/Aurora-Hak-Explorer-${version}-Linux-x86_64.zip}"

test -x "$appimage"
mkdir -p "$root/build"
package="$(mktemp -d "$root/build/Linux-${version}.XXXXXX")"
trap 'find "$package" -depth -delete' EXIT

install -Dm755 "$appimage" "$package/Aurora-Hak-Explorer-${version}-x86_64.AppImage"
install -Dm644 "$root/CHANGELOG.md" "$package/CHANGELOG.md"

rm -f "$output"
(cd "$package" && zip -q -9 -r "$output" .)
echo "$output"
