#!/usr/bin/env python3
"""Regenerate the RGBA8 reference files for crates/rustic-codecs/tests/pngsuite.

Uses Pillow as an independent decoder. 16-bit samples are scaled with
round(v * 255 / 65535), the PNG specification's recommended mapping, because
Pillow truncates them; Pillow's own tRNS handling decides alpha. Run from the
repository root: python3 tools/pngsuite_reference.py
"""
import pathlib
import sys

from PIL import Image

ROOT = pathlib.Path("crates/rustic-codecs/tests/pngsuite")


def scale16(v):
    return (v * 255 + 32767) // 65535


def pixels(im):
    if hasattr(im, "get_flattened_data"):
        return im.get_flattened_data()
    return im.getdata()


def reference(path):
    im = Image.open(path)
    im.load()
    if im.mode in ("I;16", "I;16B", "I"):
        trns = im.info.get("transparency")
        out = bytearray()
        for v in pixels(im):
            g = scale16(v)
            out += bytes([g, g, g, 0 if trns is not None and v == trns else 255])
        return bytes(out)
    depth = path.read_bytes()[24]
    trns = im.info.get("transparency")
    if im.mode == "L" and depth < 8 and isinstance(trns, int):
        # Pillow compares tRNS with samples already scaled to 8 bits; the
        # PNG specification compares the original low-depth sample.
        top = (1 << depth) - 1
        out = bytearray()
        for v in pixels(im):
            original = (v * top + 127) // 255
            out += bytes([v, v, v, 0 if original == trns else 255])
        return bytes(out)
    return im.convert("RGBA").tobytes()


def main():
    for png in sorted(ROOT.glob("*.png")):
        png.with_suffix(".rgba").write_bytes(reference(png))
    return 0


if __name__ == "__main__":
    sys.exit(main())
