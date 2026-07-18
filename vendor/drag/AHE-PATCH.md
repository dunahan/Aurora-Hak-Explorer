# AHE Windows-only dependency patch

Aurora Hak Explorer uses `drag` 2.1.1 only on Windows. This local package omits
the crate's Linux/BSD and macOS dependency declarations so Cargo does not lock
unused GTK 3 and platform crates for AHE's supported Linux and Windows builds.
The Windows implementation and its dependencies are unchanged.
