#!/usr/bin/env python3
"""Generate Rustic's original TrueType test fonts (Python 3.11+, no dependencies).

The fonts are written from scratch by this script: every outline is a simple
geometric shape defined below, and no third-party font data is copied. They
exist only to exercise the `rustic-text` sfnt parser, face matching and
font-relative CSS units with known metrics. They are released under CC0-1.0
(see crates/rustic-text/fonts/LICENSE.md).

Usage:
    python3 tools/generate_test_fonts.py           # rewrite the fonts
    python3 tools/generate_test_fonts.py --check   # fail if the fonts differ

The output is deterministic (timestamps are zero), so `--check` and the unit
test in tools/tests keep the committed binaries reproducible.
"""

from __future__ import annotations

import argparse
import struct
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FONT_DIR = ROOT / "crates/rustic-text/fonts"

# Glyph flag bits (OpenType `glyf`).
ON_CURVE = 0x01
X_SHORT = 0x02
Y_SHORT = 0x04
REPEAT = 0x08
X_SAME_OR_POSITIVE = 0x10
Y_SAME_OR_POSITIVE = 0x20

# Composite glyph flag bits.
ARG_1_AND_2_ARE_WORDS = 0x0001
ARGS_ARE_XY_VALUES = 0x0002
WE_HAVE_A_SCALE = 0x0008
MORE_COMPONENTS = 0x0020

WATER = 0x6C34  # 水, the CSS `ic` reference glyph.

# usWidthClass for each face's font-stretch.
WIDTH_CLASSES = {75.0: 3, 100.0: 5}


@dataclass(frozen=True)
class FaceSpec:
    """One face: metrics are in font units unless named otherwise."""

    stem: str
    family: str
    subfamily: str
    typographic_family: str | None
    units_per_em: int
    weight: int
    stretch: float
    italic: bool
    os2_version: int
    ascender: int
    descender: int
    x_height: int
    cap_height: int
    zero_advance: int
    x_advance: int
    h_advance: int
    water_advance: int | None
    long_loca: bool
    cmap12: bool
    short_hmetrics: bool


FACES = (
    FaceSpec("RusticTest-Regular", "Rustic Test", "Regular", None, 1000, 400, 100.0, False, 4,
             800, -200, 500, 700, 550, 500, 650, 1000, False, True, True),
    FaceSpec("RusticTest-Bold", "Rustic Test", "Bold", None, 1000, 700, 100.0, False, 4,
             800, -200, 520, 720, 600, 540, 700, 1000, True, True, False),
    # OS/2 version 1 has no sxHeight/sCapHeight: x-height and cap height
    # then come from the `x` and `H` outlines. No 水 glyph: `ic` falls back.
    FaceSpec("RusticTest-Italic", "Rustic Test", "Italic", None, 2048, 400, 100.0, True, 1,
             1638, -410, 983, 1413, 1106, 1024, 1331, None, False, False, False),
    FaceSpec("RusticTestCondensed-Regular", "Rustic Test Condensed", "Regular", "Rustic Test",
             1000, 400, 75.0, False, 4, 800, -200, 500, 700, 450, 420, 520, 900, True, True, False),
)


def u16(v: int) -> bytes:
    return struct.pack(">H", v & 0xFFFF)


def i16(v: int) -> bytes:
    return struct.pack(">h", v)


def u32(v: int) -> bytes:
    return struct.pack(">I", v & 0xFFFFFFFF)


def pad4(data: bytes) -> bytes:
    return data + b"\0" * (-len(data) % 4)


def checksum(data: bytes) -> int:
    data = pad4(data)
    return sum(struct.unpack(f">{len(data) // 4}I", data)) & 0xFFFFFFFF


# --- outlines -------------------------------------------------------------

Point = tuple[int, int, bool]


def rect(x0: int, y0: int, x1: int, y1: int, clockwise: bool = True) -> list[Point]:
    points = [(x0, y0, True), (x0, y1, True), (x1, y1, True), (x1, y0, True)]
    return points if clockwise else list(reversed(points))


def oval(x0: int, y0: int, x1: int, y1: int) -> list[Point]:
    """Four quadratic arcs: on-curve points at the edge midpoints."""
    mx, my = (x0 + x1) // 2, (y0 + y1) // 2
    return [
        (mx, y0, True), (x0, y0, False), (x0, my, True), (x0, y1, False),
        (mx, y1, True), (x1, y1, False), (x1, my, True), (x1, y0, False),
    ]


def bbox(contours: list[list[Point]]) -> tuple[int, int, int, int]:
    xs = [p[0] for c in contours for p in c]
    ys = [p[1] for c in contours for p in c]
    return min(xs), min(ys), max(xs), max(ys)


def simple_glyph(contours: list[list[Point]]) -> bytes:
    if not contours:
        return b""
    x_min, y_min, x_max, y_max = bbox(contours)
    out = i16(len(contours)) + i16(x_min) + i16(y_min) + i16(x_max) + i16(y_max)
    end = -1
    for contour in contours:
        end += len(contour)
        out += u16(end)
    out += u16(0)  # no instructions
    flags: list[int] = []
    xs = b""
    ys = b""
    px = py = 0
    for contour in contours:
        for x, y, on in contour:
            flag = ON_CURVE if on else 0
            for delta, short, same, axis in ((x - px, X_SHORT, X_SAME_OR_POSITIVE, "x"),
                                             (y - py, Y_SHORT, Y_SAME_OR_POSITIVE, "y")):
                if delta == 0:
                    flag |= same
                    encoded = b""
                elif -256 < delta < 256:
                    flag |= short | (same if delta > 0 else 0)
                    encoded = bytes([abs(delta)])
                else:
                    encoded = i16(delta)
                if axis == "x":
                    xs += encoded
                else:
                    ys += encoded
            flags.append(flag)
            px, py = x, y
    packed = b""
    i = 0
    while i < len(flags):
        run = 1
        while i + run < len(flags) and flags[i + run] == flags[i] and run < 256:
            run += 1
        if run > 1:
            packed += bytes([flags[i] | REPEAT, run - 1])
        else:
            packed += bytes([flags[i]])
        i += run
    return out + packed + xs + ys


def composite_glyph(components: list[tuple[int, int, int, bool]],
                    box: tuple[int, int, int, int]) -> bytes:
    """Components are (glyph, dx, dy, unit_scale)."""
    out = i16(-1) + b"".join(i16(v) for v in box)
    for index, (glyph, dx, dy, scale) in enumerate(components):
        flags = ARG_1_AND_2_ARE_WORDS | ARGS_ARE_XY_VALUES
        if index + 1 < len(components):
            flags |= MORE_COMPONENTS
        if scale:
            flags |= WE_HAVE_A_SCALE
        out += u16(flags) + u16(glyph) + i16(dx) + i16(dy)
        if scale:
            out += u16(0x4000)  # F2Dot14 1.0
    return out


# --- tables ---------------------------------------------------------------

def build_glyphs(f: FaceSpec) -> tuple[list[bytes], list[tuple[int, int]], dict[int, int], dict]:
    """Return glyph data, (advance, lsb) metrics, the cmap and outline stats."""
    lsb = f.units_per_em // 20
    glyphs: list[bytes] = []
    metrics: list[tuple[int, int]] = []
    cmap: dict[int, int] = {}
    stats = {"points": 0, "contours": 0, "composite_points": 0, "composite_contours": 0}

    def add(contours: list[list[Point]], advance: int, char: int | None) -> int:
        glyphs.append(simple_glyph(contours))
        metrics.append((advance, bbox(contours)[0] if contours else 0))
        stats["points"] = max(stats["points"], sum(len(c) for c in contours))
        stats["contours"] = max(stats["contours"], len(contours))
        if char is not None:
            cmap[char] = len(glyphs) - 1
        return len(glyphs) - 1

    em = f.units_per_em
    add([rect(lsb, 0, em // 2 - lsb, f.cap_height), rect(lsb * 2, lsb, em // 2 - lsb * 2,
         f.cap_height - lsb, clockwise=False)], em // 2, None)  # .notdef
    add([], em // 4, 0x20)  # space
    add([oval(lsb, 0, f.zero_advance - lsb, f.cap_height),
         oval(lsb * 3, lsb * 2, f.zero_advance - lsb * 3, f.cap_height - lsb * 2)[::-1]],
        f.zero_advance, ord("0"))
    stem = f.h_advance // 5
    mid = f.cap_height // 2
    h_shape = [
        (lsb, 0, True), (lsb, f.cap_height, True), (lsb + stem, f.cap_height, True),
        (lsb + stem, mid + stem // 2, True), (f.h_advance - lsb - stem, mid + stem // 2, True),
        (f.h_advance - lsb - stem, f.cap_height, True), (f.h_advance - lsb, f.cap_height, True),
        (f.h_advance - lsb, 0, True), (f.h_advance - lsb - stem, 0, True),
        (f.h_advance - lsb - stem, mid - stem // 2, True), (lsb + stem, mid - stem // 2, True),
        (lsb + stem, 0, True),
    ]
    add([h_shape], f.h_advance, ord("H"))
    x_glyph = add([rect(lsb, 0, f.x_advance - lsb, f.x_height)], f.x_advance, ord("x"))
    if f.water_advance is not None:
        add([rect(lsb, -em // 10, f.water_advance - lsb, em * 8 // 10)], f.water_advance, WATER)
    # `w`: two `x` glyphs side by side, the second with an explicit unit scale.
    x_box = bbox([rect(lsb, 0, f.x_advance - lsb, f.x_height)])
    w_box = (x_box[0], x_box[1], x_box[2] + f.x_advance, x_box[3])
    glyphs.append(composite_glyph([(x_glyph, 0, 0, False), (x_glyph, f.x_advance, 0, True)],
                                  w_box))
    metrics.append((f.x_advance * 2, w_box[0]))
    cmap[ord("w")] = len(glyphs) - 1
    stats["composite_points"] = 8
    stats["composite_contours"] = 2
    return glyphs, metrics, cmap, stats


def cmap_format4(cmap: dict[int, int]) -> bytes:
    # One segment per BMP character, except `w`/`x`, which share a segment
    # that maps through glyphIdArray (idRangeOffset != 0).
    chars = sorted(c for c in cmap if c <= 0xFFFF)
    segments: list[tuple[int, int, list[int] | None]] = []
    i = 0
    while i < len(chars):
        c = chars[i]
        if c == ord("w") and i + 1 < len(chars) and chars[i + 1] == ord("x"):
            segments.append((c, c + 1, [cmap[c], cmap[c + 1]]))
            i += 2
            continue
        segments.append((c, c, None))
        i += 1
    segments.append((0xFFFF, 0xFFFF, None))
    seg_count = len(segments)
    power = 1
    while power * 2 <= seg_count:
        power *= 2
    search_range = 2 * power
    entry_selector = power.bit_length() - 1
    ends = b"".join(u16(s[1]) for s in segments)
    starts = b"".join(u16(s[0]) for s in segments)
    deltas = b""
    offsets = b""
    glyph_array: list[int] = []
    for index, (start, _end, array) in enumerate(segments):
        if array is None:
            delta = 1 if start == 0xFFFF else cmap[start] - start
            deltas += u16(delta)
            offsets += u16(0)
        else:
            deltas += u16(0)
            offsets += u16(2 * (seg_count - index) + 2 * len(glyph_array))
            glyph_array.extend(array)
    body = (u16(seg_count * 2) + u16(search_range) + u16(entry_selector)
            + u16(seg_count * 2 - search_range) + ends + u16(0) + starts + deltas + offsets
            + b"".join(u16(g) for g in glyph_array))
    return u16(4) + u16(6 + len(body)) + u16(0) + body


def cmap_format12(cmap: dict[int, int]) -> bytes:
    groups = b""
    count = 0
    for char in sorted(cmap):
        groups += u32(char) + u32(char) + u32(cmap[char])
        count += 1
    return u16(12) + u16(0) + u32(16 + len(groups)) + u32(0) + u32(count) + groups


def cmap_table(f: FaceSpec, cmap: dict[int, int]) -> bytes:
    subtables = [(3, 1, cmap_format4(cmap))]
    if f.cmap12:
        subtables.append((3, 10, cmap_format12(cmap)))
    out = u16(0) + u16(len(subtables))
    offset = 4 + 8 * len(subtables)
    data = b""
    for platform, encoding, table in subtables:
        out += u16(platform) + u16(encoding) + u32(offset + len(data))
        data += table
    return out + data


def name_table(f: FaceSpec) -> bytes:
    names = {1: f.family, 2: f.subfamily, 4: f"{f.family} {f.subfamily}",
             5: "Version 1.000", 6: f.stem}
    if f.typographic_family is not None:
        names[16] = f.typographic_family
        names[17] = f.subfamily
    records = b""
    storage = b""
    for name_id in sorted(names):
        encoded = names[name_id].encode("utf-16-be")
        records += u16(3) + u16(1) + u16(0x409) + u16(name_id) + u16(len(encoded)) + u16(len(storage))
        storage += encoded
    return u16(0) + u16(len(names)) + u16(6 + 12 * len(names)) + records + storage


def os2_table(f: FaceSpec, cmap: dict[int, int], avg: int) -> bytes:
    selection = 0x01 if f.italic else 0x40  # ITALIC or REGULAR
    if f.weight >= 700 and not f.italic:
        selection = 0x20  # BOLD
    out = (u16(f.os2_version) + i16(avg) + u16(f.weight) + u16(WIDTH_CLASSES[f.stretch])
           + u16(0)  # fsType: installable
           + b"".join(i16(v) for v in (650, 600, 0, 75, 650, 600, 0, 350, 50, 250))
           + i16(0)  # sFamilyClass
           + bytes(10)  # panose
           + u32(1) + u32(0) + u32(0) + u32(0)  # Basic Latin
           + b"RSTC" + u16(selection) + u16(min(cmap)) + u16(min(max(cmap), 0xFFFF))
           + i16(f.ascender) + i16(f.descender) + i16(0)
           + u16(f.ascender) + u16(-f.descender))
    if f.os2_version >= 1:
        out += u32(1) + u32(0)
    if f.os2_version >= 2:
        out += i16(f.x_height) + i16(f.cap_height) + u16(0) + u16(0x20) + u16(1)
    return out


def build_font(f: FaceSpec) -> bytes:
    glyphs, metrics, cmap, stats = build_glyphs(f)
    num_glyphs = len(glyphs)
    glyf = b""
    loca_offsets = []
    for glyph in glyphs:
        loca_offsets.append(len(glyf))
        glyf += pad4(glyph)
    loca_offsets.append(len(glyf))
    if f.long_loca:
        loca = b"".join(u32(o) for o in loca_offsets)
    else:
        loca = b"".join(u16(o // 2) for o in loca_offsets)

    # A short hmtx omits the final advance: the last glyph reuses it.
    long_metrics = num_glyphs
    if f.short_hmetrics and metrics[-1][0] == metrics[-2][0]:
        long_metrics -= 1
    elif f.short_hmetrics:
        metrics[-1] = (metrics[-2][0], metrics[-1][1])
        long_metrics -= 1
    hmtx = b"".join(u16(a) + i16(l) for a, l in metrics[:long_metrics])
    hmtx += b"".join(i16(l) for _, l in metrics[long_metrics:])

    boxes = []
    for glyph in glyphs:
        if glyph:
            boxes.append(struct.unpack(">4h", glyph[2:10]))
    x_min = min(b[0] for b in boxes)
    y_min = min(b[1] for b in boxes)
    x_max = max(b[2] for b in boxes)
    y_max = max(b[3] for b in boxes)
    advances = [a for a, _ in metrics]
    mac_style = (0x01 if f.weight >= 700 else 0) | (0x02 if f.italic else 0)

    head = (u32(0x00010000) + u32(0x00010000) + u32(0)  # checkSumAdjustment patched below
            + u32(0x5F0F3CF5) + u16(0x000B) + u16(f.units_per_em) + bytes(16)
            + i16(x_min) + i16(y_min) + i16(x_max) + i16(y_max) + u16(mac_style)
            + u16(8) + i16(2) + i16(1 if f.long_loca else 0) + i16(0))
    hhea = (u32(0x00010000) + i16(f.ascender) + i16(f.descender) + i16(0)
            + u16(max(advances)) + i16(min(b[0] for b in boxes)) + i16(0) + i16(x_max)
            + i16(1) + i16(0) + i16(0) + bytes(8) + i16(0) + u16(long_metrics))
    maxp = (u32(0x00010000) + u16(num_glyphs) + u16(stats["points"]) + u16(stats["contours"])
            + u16(stats["composite_points"]) + u16(stats["composite_contours"])
            + u16(2) + u16(0) + u16(0) + u16(0) + u16(0) + u16(0) + u16(0) + u16(2) + u16(1))
    italic_angle = (-12 << 16) if f.italic else 0
    post = u32(0x00030000) + u32(italic_angle) + i16(-100) + i16(50) + bytes(20)
    tables = {
        b"OS/2": os2_table(f, cmap, sum(advances) // len(advances)),
        b"cmap": cmap_table(f, cmap),
        b"glyf": glyf,
        b"head": head,
        b"hhea": hhea,
        b"hmtx": hmtx,
        b"loca": loca,
        b"maxp": maxp,
        b"name": name_table(f),
        b"post": post,
    }
    tags = sorted(tables)
    count = len(tags)
    power = 1
    while power * 2 <= count:
        power *= 2
    header = (u32(0x00010000) + u16(count) + u16(power * 16) + u16(power.bit_length() - 1)
              + u16(count * 16 - power * 16))
    offset = len(header) + 16 * count
    directory = b""
    body = b""
    head_offset = 0
    for tag in tags:
        data = tables[tag]
        if tag == b"head":
            head_offset = offset + len(body)
        directory += tag + u32(checksum(data)) + u32(offset + len(body)) + u32(len(data))
        body += pad4(data)
    font = bytearray(header + directory + body)
    adjustment = (0xB1B0AFBA - checksum(bytes(font))) & 0xFFFFFFFF
    font[head_offset + 8:head_offset + 12] = u32(adjustment)
    return bytes(font)


def outputs() -> dict[Path, bytes]:
    return {FONT_DIR / f"{face.stem}.ttf": build_font(face) for face in FACES}


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true", help="verify instead of writing")
    args = parser.parse_args(argv)
    stale = []
    for path, data in outputs().items():
        if args.check:
            if not path.is_file() or path.read_bytes() != data:
                stale.append(path.relative_to(ROOT))
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
    if stale:
        for path in stale:
            print(f"{path}: differs from tools/generate_test_fonts.py output", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
