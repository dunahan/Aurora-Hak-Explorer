# Aurora Hak Explorer winit patch

This is the upstream `winit` 0.30.13 crate with a focused Linux/X11 XDND
compatibility patch.

KDE/Dolphin uses the ICCCM `INCR` selection protocol when a drag contains a
large `text/uri-list` (for example, tens of thousands of selected files).
Upstream 0.30.13 only reads an immediately available selection property, so
those drops never reach the application. The AHE patch receives the bounded
incremental chunks through `PropertyNotify`, assembles the URI list, and emits
the normal winit file-drop events once complete. It also records an outstanding
selection request so repeated pointer-position events cannot restart a large
transfer before it finishes.

The normal immediate XDND path and all non-X11 platforms remain unchanged.
