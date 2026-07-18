# AHE standalone model compiler source

This directory contains the NWNTools model compiler sources used to produce
the `nwnmdlcomp` helpers bundled with Aurora Hak Explorer. The code originates
from NWN Explorer/NWNTools by Edward T. Smith and the Open Knights Consortium
and is distributed under the BSD-style license in `LICENSE`.

AHE carries two small portability changes:

- ASCII validation checks each input byte rather than repeatedly checking the
  first byte.
- Ordinary compile/decompile operations do not inspect the Neverwinter Nights
  installation or Windows registry. Required supermodels are staged by AHE.

Build both 32-bit, standalone helpers with Zig 0.16 or newer:

```sh
ZIG=/path/to/zig ./build.sh all
```

The 32-bit target is intentional. NWNTools' legacy in-memory structures match
the original Aurora binary MDL layout only with 32-bit pointers.
