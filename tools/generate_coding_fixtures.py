#!/usr/bin/env python3
"""Regenerate rustic-net's content-coding fixtures.

Needs the `brotli` Python package (Google's reference encoder bindings).
Outputs are committed; this script only documents how they were made.

    python3 tools/generate_coding_fixtures.py
"""

import gzip
import zlib
from pathlib import Path

import brotli

OUT = Path(__file__).resolve().parents[1] / "crates/rustic-net/tests/fixtures/coding"
TEXT = b"hello hello hello\n"


def raw_deflate(data: bytes) -> bytes:
    encoder = zlib.compressobj(9, zlib.DEFLATED, -15)
    return encoder.compress(data) + encoder.flush()


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    gz = gzip.compress(TEXT, mtime=0)
    files = {
        "hello.gz": gz,
        "hello.zlib": zlib.compress(TEXT, 9),
        "hello.deflate": raw_deflate(TEXT),
        "hello.br": brotli.compress(TEXT),
        "hello.gz.br": brotli.compress(gz),
        "zeros-16m.br": brotli.compress(bytes(16 << 20), quality=11),
        "zeros-16m.gz": gzip.compress(bytes(16 << 20), compresslevel=9, mtime=0),
    }
    for name, data in files.items():
        (OUT / name).write_bytes(data)
        print(f"{name}: {len(data)} bytes")


if __name__ == "__main__":
    main()
