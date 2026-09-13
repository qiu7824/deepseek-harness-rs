# Responsive raster surfaces

The `surface` Cargo feature adds a bounded raster painter backed by the existing Unicode text engine. It leaves product palettes, message parsing and data access in the application.

`view::surface` (available with `canvas`) lays out ordinary retained children using local logical rectangles returned by a layout-time callback. The callback receives current physical bounds and DPI. Child IDs must be stable and unique. Native input controls and scroll regions may be positioned above raster content; they remain part of the regular input tree.

`surface::SurfacePainter` shares measurement and glyph rendering. It retains at most 256 text layouts / 4 MiB and 4 MiB of glyph data by default. `RasterSurface::new` rejects non-finite dimensions and buffers larger than 64 MiB. Cloned image frames share immutable BGRA storage. Explicit pixel and logical units are converted at the paint boundary.

Callbacks must not perform network or disk I/O. Store fetched data in application state and request an invalidation after asynchronous changes. Create one painter per window, not one per text node. Raster operations do not replace input, selection or accessibility semantics.
