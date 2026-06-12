#!/usr/bin/env python3
"""Rasterize static/favicon.svg rects to PNG, or pack PNGs into favicon.ico."""

from __future__ import annotations

import re
import struct
import sys
import zlib
from pathlib import Path


def hex_to_rgb(value: str) -> tuple[int, int, int]:
    value = value.lstrip("#")
    return tuple(int(value[i : i + 2], 16) for i in (0, 2, 4))


def parse_svg_rects(svg: str) -> tuple[tuple[int, int, int], list[tuple[int, int, int, int, tuple[int, int, int]]]]:
    bg_match = re.search(r'<rect width="32" height="32" fill="(#[0-9a-fA-F]+)"', svg)
    if not bg_match:
        raise ValueError("expected 32x32 background rect in favicon.svg")
    bg = hex_to_rgb(bg_match.group(1))
    rects: list[tuple[int, int, int, int, tuple[int, int, int]]] = []
    for match in re.finditer(
        r'<rect x="(\d+)" y="(\d+)" width="(\d+)" height="(\d+)" fill="(#[0-9a-fA-F]+)"',
        svg,
    ):
        x, y, w, h, color = match.groups()
        rects.append((int(x), int(y), int(w), int(h), hex_to_rgb(color)))
    return bg, rects


def render_rects(
    bg: tuple[int, int, int],
    rects: list[tuple[int, int, int, int, tuple[int, int, int]]],
    size: int,
) -> list[list[tuple[int, int, int]]]:
    scale = size / 32
    img = [[bg for _ in range(size)] for _ in range(size)]
    for x, y, w, h, color in rects:
        if x == 0 and y == 0 and w == 32 and h == 32:
            continue
        x0 = int(round(x * scale))
        y0 = int(round(y * scale))
        x1 = int(round((x + w) * scale))
        y1 = int(round((y + h) * scale))
        for py in range(y0, min(y1, size)):
            for px in range(x0, min(x1, size)):
                img[py][px] = color
    return img


def write_png(path: Path, img: list[list[tuple[int, int, int]]]) -> bytes:
    height = len(img)
    width = len(img[0])
    raw = bytearray()
    for row in img:
        raw.append(0)
        for r, g, b in row:
            raw.extend((r, g, b))
    compressed = zlib.compress(bytes(raw), 9)

    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    data = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDATA", compressed) + chunk(b"IEND", b"")
    path.write_bytes(data)
    return data


def write_ico(path: Path, png_files: list[Path]) -> None:
    images: list[tuple[bytes, int]] = []
    for png_path in png_files:
        png = png_path.read_bytes()
        if png[12:16] != b"IHDR":
            raise ValueError(f"{png_path} is not a PNG")
        width, height = struct.unpack(">II", png[16:24])
        if width != height:
            raise ValueError(f"{png_path} must be square")
        images.append((png, width))

    count = len(images)
    header = struct.pack("<HHH", 0, 1, count)
    offset = 6 + 16 * count
    entries = bytearray()
    blob = bytearray()
    for png, size in images:
        w = size if size < 256 else 0
        entries.extend(struct.pack("<BBBBHHII", w, w, 0, 0, 1, 32, len(png), offset + len(blob)))
        blob.extend(png)
    path.write_bytes(header + bytes(entries) + bytes(blob))


def rasterize_svg(svg_path: Path, out_path: Path, size: int) -> None:
    bg, rects = parse_svg_rects(svg_path.read_text())
    write_png(out_path, render_rects(bg, rects, size))


def main(argv: list[str]) -> int:
    if len(argv) >= 2 and argv[1] == "--ico":
        if len(argv) < 4:
            print("usage: rasterize_favicon.py --ico out.ico in16.png in32.png ...", file=sys.stderr)
            return 1
        write_ico(Path(argv[2]), [Path(p) for p in argv[3:]])
        return 0

    if len(argv) != 4:
        print("usage: rasterize_favicon.py favicon.svg out.png size", file=sys.stderr)
        return 1

    rasterize_svg(Path(argv[1]), Path(argv[2]), int(argv[3]))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
