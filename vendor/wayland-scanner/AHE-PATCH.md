# AHE compatibility patch

This is `wayland-scanner` 0.31.10. Its `quick-xml` dependency is advanced from
0.39 to the API-compatible 0.41 series to avoid RUSTSEC-2026-0194 and
RUSTSEC-2026-0195. The one changed API call now explicitly selects XML 1.0,
which is the behavior used for Wayland protocol descriptions.

Remove this patch once an upstream wayland-rs release carries the fixed
dependency.
