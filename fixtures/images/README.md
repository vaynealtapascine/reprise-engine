# Image test inputs

These original synthetic fixtures are dedicated to the public domain under CC0-1.0.
No source images were used. `generate.py` reproduces them (Python standard library
and Pillow 12.3.0 for JPEG). PNGs use exact integer chunks and zlib level 9.

- red-1x1.png: opaque red pixel, no density, default 96 DPI.
- density.png: same pixel, 2835 pixels/metre (approximately 72 DPI).
- red-2x1.jpg: two red pixels, baseline JPEG, JFIF 72 DPI.
- header-65535.png: valid dimensions and metadata but intentionally empty IDAT;
  layout must read the header without attempting a 4-billion-pixel decode.
