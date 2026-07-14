#!/usr/bin/env python3
"""Generate the YumeDock application icon using only the Python standard library.

The mark is a rounded-square tile (deep slate, matching the app's top bar)
holding a soft white "dock + bar" silhouette: a thin top pill over a rounded
dock with three dots. It aligns with PRODUCT.md -- quiet, precise, polished,
and deliberately not a clone of any platform logo.

Outputs assets/yumedock.ico with 16/32/48/64/128/256 px entries. PNG and ICO
are written by hand (struct + zlib) so no Pillow or other third-party image
dependency is required -- this runs identically locally and in CI.
"""

from __future__ import annotations

import math
import struct
import zlib
from pathlib import Path

SIZES = (16, 32, 48, 64, 128, 256)


def _clamp(v: float) -> int:
    return 0 if v < 0 else (255 if v > 255 else int(v + 0.5))


def _rgba(r: float, g: float, b: float, a: float = 255.0) -> tuple[int, int, int, int]:
    return (_clamp(r), _clamp(g), _clamp(b), _clamp(a))


def _in_rounded_rect(px: float, py: float, x: float, y: float, w: float, h: float, r: float) -> bool:
    """True if (px,py) is inside a rounded rectangle."""
    if px < x or px > x + w or py < y or py > y + h:
        return False
    # Distance from the nearest corner center.
    cx = min(max(px, x + r), x + w - r)
    cy = min(max(py, y + r), y + h - r)
    dx = px - cx
    dy = py - cy
    return dx * dx + dy * dy <= r * r


def _in_ellipse(px: float, py: float, cx: float, cy: float, rx: float, ry: float) -> bool:
    dx = (px - cx) / rx
    dy = (py - cy) / ry
    return dx * dx + dy * dy <= 1.0


def _in_rounded_bar(px: float, py: float, cx: float, cy: float, half_w: float, half_h: float, r: float) -> bool:
    """A horizontally-centered rounded bar (capsule)."""
    return _in_rounded_rect(px, py, cx - half_w, cy - half_h, half_w * 2, half_h * 2, r)


def render(size: int) -> bytes:
    """Render the icon at `size` px and return a BGRA pixel buffer."""
    s = float(size)
    px = bytearray(size * size * 4)

    # Tile: rounded square filling ~92% of the canvas with a small margin.
    margin = s * 0.04
    tile_r = s * 0.22

    # Colors (RGBA floats for easy blending).
    tile_top = (0x1a, 0x22, 0x30)      # slate
    tile_bottom = (0x0c, 0x11, 0x19)   # near-black
    fg = (0xF5, 0xF7, 0xFB)            # soft white
    fg_dim = (0xC8, 0xD2, 0xE0)        # dimmer accent for dock dots

    for y in range(size):
        for x in range(size):
            fx = x + 0.5
            fy = y + 0.5
            i = (y * size + x) * 4

            if not _in_rounded_rect(fx, fy, margin, margin, s - 2 * margin, s - 2 * margin, tile_r):
                # Fully transparent outside the tile.
                px[i : i + 4] = b"\x00\x00\x00\x00"
                continue

            # Vertical gradient over the tile.
            t = (fy - margin) / (s - 2 * margin)
            base = (
                tile_top[0] + (tile_bottom[0] - tile_top[0]) * t,
                tile_top[1] + (tile_bottom[1] - tile_top[1]) * t,
                tile_top[2] + (tile_bottom[2] - tile_top[2]) * t,
            )

            # --- Draw the silhouette in normalized [0,1] then scale. ---
            nx = fx / s
            ny = fy / s

            color = base
            alpha = 255.0

            # Top status bar pill.
            bar = _in_rounded_bar(nx, ny, cx=0.5, cy=0.30, half_w=0.26, half_h=0.045, r=0.045)
            # Dock body.
            dock = _in_rounded_rect(
                px=nx * s, py=ny * s,
                x=(0.5 - 0.30) * s, y=(0.52) * s,
                w=0.60 * s, h=0.20 * s, r=0.085 * s,
            )
            # Three dock dots (app icons).
            dot_r = 0.035
            dot_y = 0.62
            dots = (
                _in_ellipse(nx, ny, 0.38, dot_y, dot_r, dot_r),
                _in_ellipse(nx, ny, 0.50, dot_y, dot_r, dot_r),
                _in_ellipse(nx, ny, 0.62, dot_y, dot_r, dot_r),
            )

            if bar:
                color = (fg[0], fg[1], fg[2])
            elif dock:
                color = (fg[0] * 0.96, fg[1] * 0.97, fg[2])
            elif any(dots):
                color = (fg_dim[0], fg_dim[1], fg_dim[2])

            px[i : i + 4] = struct.pack("BBBB", _clamp(color[2]), _clamp(color[1]), _clamp(color[0]), _clamp(alpha))

    return bytes(px)


def write_png(path: Path, size: int, buf: bytes) -> None:
    """Write `buf` (BGRA) as a PNG file (RGBA)."""
    path.write_bytes(encode_png(size, buf))


def encode_png(size: int, buf: bytes) -> bytes:
    """Return the PNG encoding of a BGRA `buf` as bytes (RGBA output)."""

    def chunk(tag: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    # Convert BGRA buffer to RGBA scanlines, each filtered with type 0 (None).
    raw = bytearray()
    for y in range(size):
        raw.append(0)  # filter type: None
        row = bytearray()
        for x in range(size):
            i = (y * size + x) * 4
            b, g, r, a = buf[i], buf[i + 1], buf[i + 2], buf[i + 3]
            row += struct.pack("BBBB", r, g, b, a)
        raw += row

    sig = b"\x89PNG\r\n\x1a\n"
    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)  # 8-bit, RGBA
    compressed = zlib.compress(bytes(raw), 9)
    return sig + chunk(b"IHDR", ihdr) + chunk(b"IDAT", compressed) + chunk(b"IEND", b"")


def _png_for(size: int) -> bytes:
    buf = render(size)
    return encode_png(size, buf)


def write_ico(path: Path, sizes: tuple[int, ...]) -> None:
    """Assemble a multi-resolution ICO from PNG-encoded entries."""
    entries: list[tuple[int, int, bytes]] = []
    for size in sizes:
        png = _png_for(size)
        entries.append((size, size, png))

    # ICONDIR header: reserved(2)=0, type(2)=1, count(2)=N.
    count = len(entries)
    header = struct.pack("<HHH", 0, 1, count)

    offset = 6 + count * 16  # header + each 16-byte directory entry
    directory = b""
    images = b""
    for (w, h, png) in entries:
        w_byte = 0 if w >= 256 else w
        h_byte = 0 if h >= 256 else h
        directory += struct.pack(
            "<BBBBHHII",
            w_byte,       # width (0 => 256)
            h_byte,       # height (0 => 256)
            0,            # palette count (0 = no palette)
            0,            # reserved
            1,            # color planes
            32,           # bits per pixel
            len(png),     # image size
            offset,       # image offset
        )
        images += png
        offset += len(png)

    path.write_bytes(header + directory + images)


def main() -> None:
    out = Path(__file__).resolve().parent / "yumedock.ico"
    write_ico(out, SIZES)
    print(f"wrote {out} ({out.stat().st_size} bytes) sizes={SIZES}")


if __name__ == "__main__":
    main()
