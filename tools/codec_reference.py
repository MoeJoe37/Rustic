#!/usr/bin/env python3
"""Generate the JPEG, GIF and WebP conformance images and their references.

Writes crates/rustic-codecs/tests/images/: synthetic test pictures encoded
with Pillow (libjpeg-turbo, Pillow's GIF writer and libwebp), an RGBA8
reference for each decoded by Pillow as an independent decoder, and
MANIFEST, one line per image: "file width height frames tolerance", where
tolerance is the largest per-channel difference the decoder may show.
Animated references hold every composited frame, concatenated.

Nothing here is downloaded: every picture is drawn by this script from a
fixed seed, so rerunning it reproduces the suite for the same Pillow,
libjpeg-turbo and libwebp versions. Install Pillow outside the repository
(for example in a virtualenv under a scratch directory) and run from the
repository root: python3 tools/codec_reference.py
"""
import io
import math
import pathlib
import random
import sys

from PIL import Image, ImageDraw, ImageOps

OUT = pathlib.Path("crates/rustic-codecs/tests/images")
# JPEG decoders may differ in IDCT rounding and upsampling (T.81 allows
# it). VP8 decoding is exact, and Rustic uses libwebp's upsampling and
# YUV->RGB arithmetic, so WebP must match exactly.
JPEG_TOLERANCE = 3


def picture(width, height, seed, alpha=False):
    """Gradients, hard edges, text-like strokes and noise."""
    rng = random.Random(seed)
    im = Image.new("RGBA", (width, height))
    px = im.load()
    for y in range(height):
        for x in range(width):
            r = int(255 * x / max(1, width - 1))
            g = int(255 * y / max(1, height - 1))
            b = int(127 + 127 * math.sin((x + 2 * y) / 5.0))
            a = 255
            if alpha:
                a = int(255 * ((x * 3 + y * 5) % 97) / 96)
            px[x, y] = (r, g, b, a)
    draw = ImageDraw.Draw(im)
    for _ in range(6):
        x0, y0 = rng.randrange(width), rng.randrange(height)
        x1, y1 = rng.randrange(width), rng.randrange(height)
        color = tuple(rng.randrange(256) for _ in range(3)) + (
            rng.randrange(256) if alpha else 255,
        )
        draw.rectangle([min(x0, x1), min(y0, y1), max(x0, x1), max(y0, y1)], fill=color)
        draw.line([x0, y1, x1, y0], fill=(255, 255, 255, 255), width=1)
    for _ in range(width * height // 16):
        x, y = rng.randrange(width), rng.randrange(height)
        px[x, y] = tuple(rng.randrange(256) for _ in range(3)) + (px[x, y][3],)
    return im


def palette_picture(width, height, seed, colors):
    rng = random.Random(seed)
    im = Image.new("RGB", (width, height))
    palette = [tuple(rng.randrange(256) for _ in range(3)) for _ in range(colors)]
    px = im.load()
    for y in range(height):
        for x in range(width):
            px[x, y] = palette[((x // 3) + (y // 2) * 5 + rng.randrange(2)) % colors]
    return im


def rgba(im):
    return im.convert("RGBA").tobytes()


def jpeg_reference(data):
    im = Image.open(io.BytesIO(data))
    im.load()
    im = ImageOps.exif_transpose(im)
    if im.mode == "CMYK":
        # Pillow undoes Adobe inversion; convert with the decoder's formula.
        out = bytearray()
        data = im.get_flattened_data() if hasattr(im, "get_flattened_data") else im.getdata()
        for c, m, y, k in data:
            out += bytes(
                [((255 - v) * (255 - k) + 127) // 255 for v in (c, m, y)] + [255]
            )
        return im.size, bytes(out)
    return im.size, rgba(im)


def frames_reference(data):
    im = Image.open(io.BytesIO(data))
    out = bytearray()
    count = getattr(im, "n_frames", 1)
    for i in range(count):
        im.seek(i)
        im.load()
        out += rgba(im)
    return im.size, count, bytes(out)


def jpegs():
    base = picture(37, 29, 1).convert("RGB")
    yield "baseline-444", dict(quality=90, subsampling=0), base
    yield "baseline-422", dict(quality=85, subsampling=1), base
    yield "baseline-420", dict(quality=75, subsampling=2), base
    yield "baseline-q100", dict(quality=100, subsampling=0), base
    yield "baseline-q5", dict(quality=5, subsampling=2), base
    yield "optimized-420", dict(quality=80, subsampling=2, optimize=True), base
    yield "progressive-444", dict(quality=90, subsampling=0, progressive=True), base
    yield "progressive-420", dict(quality=80, subsampling=2, progressive=True), base
    yield "progressive-422", dict(quality=60, subsampling=1, progressive=True), base
    yield "restart-420", dict(quality=80, subsampling=2, restart_marker_blocks=3), base
    yield "restart-progressive", dict(
        quality=80, subsampling=2, progressive=True, restart_marker_rows=1
    ), base
    gray = picture(40, 24, 2).convert("L")
    yield "gray", dict(quality=85), gray
    yield "gray-progressive", dict(quality=85, progressive=True), gray
    yield "tiny-1x1", dict(quality=90), base.crop((5, 5, 6, 6))
    yield "large-420", dict(quality=70, subsampling=2), picture(161, 97, 3).convert("RGB")
    yield "large-progressive", dict(
        quality=70, subsampling=2, progressive=True
    ), picture(161, 97, 3).convert("RGB")
    yield "cmyk", dict(quality=90), picture(24, 20, 4).convert("CMYK")
    for orientation in (3, 6, 8):
        exif = Image.Exif()
        exif[0x0112] = orientation
        yield f"exif-orientation-{orientation}", dict(
            quality=90, exif=exif.tobytes()
        ), picture(21, 13, 5).convert("RGB")


def gifs():
    still = palette_picture(31, 23, 6, 16)
    yield "still-16", dict(), [still], None
    yield "still-256", dict(), [
        picture(30, 30, 7).convert("RGB").quantize(256, dither=Image.Dither.NONE)
    ], None
    yield "interlaced", dict(interlace=True), [palette_picture(17, 19, 8, 7)], None
    yield "two-colors", dict(), [palette_picture(9, 9, 9, 2)], None
    t = palette_picture(20, 16, 10, 8).convert("P")
    yield "transparent", dict(transparency=3), [t], None
    frames = []
    for i in range(4):
        f = Image.new("RGBA", (24, 18), (0, 0, 0, 0))
        d = ImageDraw.Draw(f)
        d.rectangle([2 + 4 * i, 2 + 2 * i, 10 + 4 * i, 8 + 2 * i], fill=(200, 40 * i, 90, 255))
        frames.append(f)
    for disposal in (0, 1, 2):
        yield f"animated-disposal-{disposal}", dict(
            duration=[70, 120, 30, 200], loop=0, disposal=disposal
        ), frames, None


def webps():
    rgba_pic = picture(33, 27, 11, alpha=True)
    rgb_pic = picture(33, 27, 12).convert("RGB")
    for method in (0, 3, 6):
        yield f"lossless-m{method}", dict(lossless=True, method=method, exact=True), rgba_pic
    yield "lossless-rgb", dict(lossless=True, quality=100), rgb_pic
    for colors in (2, 4, 16, 200):
        yield f"lossless-palette-{colors}", dict(lossless=True, quality=100), palette_picture(
            29, 21, 13 + colors, colors
        )
    yield "lossless-large", dict(lossless=True, quality=50, method=4), picture(150, 90, 14)
    yield "lossy", dict(quality=80), rgb_pic
    yield "lossy-q5", dict(quality=5, method=6), rgb_pic
    yield "lossy-q100", dict(quality=100), rgb_pic
    yield "lossy-alpha", dict(quality=70, alpha_quality=100), rgba_pic
    yield "lossy-alpha-lossy", dict(quality=70, alpha_quality=40), rgba_pic
    yield "lossy-large", dict(quality=60, method=2), picture(150, 90, 15).convert("RGB")
    yield "lossy-tiny", dict(quality=80), rgb_pic.crop((0, 0, 1, 1))
    frames = []
    for i in range(5):
        f = picture(40, 30, 20, alpha=True) if i % 2 else Image.new("RGBA", (40, 30), (0, 0, 0, 0))
        d = ImageDraw.Draw(f)
        d.ellipse([3 * i, 2 * i, 12 + 4 * i, 10 + 3 * i], fill=(30 * i, 200, 255 - 40 * i, 160 + 20 * i))
        frames.append(f)
    yield "animated-lossless", dict(
        save_all=True, append_images=frames[1:], duration=[50, 80, 120, 40, 90], loop=3,
        lossless=True,
    ), frames[0]
    yield "animated-mixed", dict(
        save_all=True, append_images=frames[1:], duration=100, loop=0, allow_mixed=True,
        quality=60,
    ), frames[0]
    yield "animated-lossy", dict(
        save_all=True, append_images=frames[1:], duration=70, quality=50, minimize_size=True,
    ), frames[0]


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    for old in OUT.iterdir():
        if old.name != "README":
            old.unlink()
    manifest = []
    for name, options, im in jpegs():
        buf = io.BytesIO()
        im.save(buf, "JPEG", **options)
        data = buf.getvalue()
        (OUT / f"{name}.jpg").write_bytes(data)
        size, ref = jpeg_reference(data)
        (OUT / f"{name}.rgba").write_bytes(ref)
        manifest.append(f"{name}.jpg {size[0]} {size[1]} 1 {JPEG_TOLERANCE}")
    for name, options, frames, _ in gifs():
        buf = io.BytesIO()
        frames[0].save(buf, "GIF", save_all=len(frames) > 1, append_images=frames[1:], **options)
        data = buf.getvalue()
        (OUT / f"{name}.gif").write_bytes(data)
        size, count, ref = frames_reference(data)
        (OUT / f"{name}.rgba").write_bytes(ref)
        manifest.append(f"{name}.gif {size[0]} {size[1]} {count} 0")
    for name, options, im in webps():
        buf = io.BytesIO()
        im.save(buf, "WEBP", **options)
        data = buf.getvalue()
        (OUT / f"{name}.webp").write_bytes(data)
        size, count, ref = frames_reference(data)
        (OUT / f"{name}.rgba").write_bytes(ref)
        manifest.append(f"{name}.webp {size[0]} {size[1]} {count} 0")
    (OUT / "MANIFEST").write_text("\n".join(manifest) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
