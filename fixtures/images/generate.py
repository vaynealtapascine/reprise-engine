"""Reproduce original CC0 image fixtures; Pillow 12.3.0 is used for JPEG."""
from pathlib import Path
import binascii
import struct
import zlib
from PIL import Image

ROOT = Path(__file__).resolve().parent


def chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", binascii.crc32(kind + data) & 0xFFFFFFFF)


def png(width, height, data=b"", extra=b""):
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
            + extra + chunk(b"IDAT", data) + chunk(b"IEND", b""))


pixel = zlib.compress(b"\0\xff\0\0\xff", 9)
(ROOT / "red-1x1.png").write_bytes(png(1, 1, pixel))
(ROOT / "header-65535.png").write_bytes(png(65535, 65535))
(ROOT / "density.png").write_bytes(png(1, 1, pixel, chunk(b"pHYs", struct.pack(">IIB", 2835, 2835, 1))))
Image.new("RGB", (2, 1), (255, 0, 0)).save(ROOT / "red-2x1.jpg", format="JPEG", quality=90,
    subsampling=0, dpi=(72, 72), optimize=False, progressive=False)
