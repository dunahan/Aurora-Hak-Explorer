#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")" && pwd)"
zig="${ZIG:-zig}"
platform="${1:-all}"

sources=(
    "$root"/_MathLib/*.cpp
    "$root"/_NmcLib/*.cpp
    "$root"/_NwnLib/*.cpp
    "$root"/nwnmdlcomp/nwnmdlcomp.cpp
)
common=(
    c++ -std=gnu++98 -O2 -s
    -Wno-c++11-narrowing -Wno-deprecated-declarations
    -Wno-unused-command-line-argument
    -Wno-unused-value -Wno-nontrivial-memcall
    -Wno-mismatched-new-delete -Wno-missing-exception-spec
    -Wno-nullability-completeness
    # NWNTools maps the original 32-bit Aurora binary layout with offsetof on
    # inherited layout structs. This is why these helpers deliberately target
    # 32-bit, and the resulting layout is covered by byte-for-byte tests.
    -Wno-invalid-offsetof
    -I"$root" -I"$root/_MathLib" -I"$root/_NmcLib" -I"$root/_NwnLib"
)

build_linux() {
    mkdir -p "$root/../linux"
    "$zig" "${common[@]}" -target x86-linux-musl -static \
        "${sources[@]}" -o "$root/../linux/nwnmdlcomp"
}

build_windows() {
    mkdir -p "$root/../windows"
    "$zig" "${common[@]}" -target x86-windows-gnu -fms-extensions \
        -D_CRT_SECURE_NO_WARNINGS "${sources[@]}" \
        -o "$root/../windows/nwnmdlcomp.exe"
}

case "$platform" in
    linux) build_linux ;;
    windows) build_windows ;;
    all)
        build_linux
        build_windows
        ;;
    *)
        echo "usage: $0 [linux|windows|all]" >&2
        exit 2
        ;;
esac
