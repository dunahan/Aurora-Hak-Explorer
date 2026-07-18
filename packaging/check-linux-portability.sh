#!/usr/bin/env bash
set -euo pipefail

binary="${1:?usage: check-linux-portability.sh BINARY}"
test -x "$binary"
command -v objdump >/dev/null || {
  echo "objdump is required to verify Linux binary compatibility" >&2
  exit 1
}

max_glibc="$({
  objdump -T "$binary" | sed -n 's/.*GLIBC_\([0-9][0-9.]*\).*/\1/p'
} | sort -Vu | tail -1)"
test -n "$max_glibc" || {
  echo "could not determine the glibc requirement for $binary" >&2
  exit 1
}

maximum_supported="2.17"
newest="$(printf '%s\n%s\n' "$max_glibc" "$maximum_supported" | sort -V | tail -1)"
if [[ "$newest" != "$maximum_supported" ]]; then
  echo "$binary requires glibc $max_glibc; portable releases must require glibc $maximum_supported or older" >&2
  exit 1
fi

echo "$binary requires glibc $max_glibc (portable limit: $maximum_supported)"
