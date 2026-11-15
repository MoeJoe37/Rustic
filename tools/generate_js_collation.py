#!/usr/bin/env python3
"""Generate compact collation tables from CLDR 48 and Unicode 17."""

import argparse
import hashlib
import re
import struct
from pathlib import Path
from zipfile import ZipFile

DATA = Path(__file__).resolve().parents[1] / "crates/rustic-js/data"
CORE_SHA = "06c7c698d6fd8d67cefac15a0206b0109b00e0ef1636f86f84449fa959561f74"
UCD_SHA = "2e1efc1dcb59c575eedf5ccae60f95229f706ee6d031835247d843c11d96470c"
ELEMENT = re.compile(r"\[([.*])([0-9A-F]+)\.([0-9A-F]+)\.([0-9A-F]+)\]")


def checked(path, digest):
    if hashlib.sha256(path.read_bytes()).hexdigest() != digest:
        raise ValueError(f"unexpected digest for {path}")
    return path


def collation(core):
    with ZipFile(core) as archive:
        contents = archive.read("common/uca/allkeys_CLDR.txt").decode("utf-8")
    rows = {}
    for line in contents.splitlines():
        text = line.partition("#")[0].strip()
        if not text or text == "@version 17.0.0":
            continue
        source, encoded = map(str.strip, text.split(";"))
        source = tuple(int(point, 16) for point in source.split())
        weights = tuple(
            (int(primary, 16), int(secondary, 16), int(tertiary, 16), variable == "*")
            for variable, primary, secondary, tertiary in ELEMENT.findall(encoded)
        )
        if not weights or source in rows or len(source) > 4:
            raise ValueError(f"invalid collation mapping {line}")
        rows[source] = weights

    keys = bytearray()
    elements = bytearray()
    expansions = {}
    # Bias code points by one to distinguish a missing key unit from U+0000.
    for source, weights in sorted(rows.items()):
        for point in [*(point + 1 for point in source), *([0] * (4 - len(source)))]:
            keys.extend(point.to_bytes(3, "little"))
        if weights not in expansions:
            expansions[weights] = len(elements) // 6
            for primary, secondary, tertiary, variable in weights:
                elements.extend(struct.pack("<HHBB", primary, secondary, tertiary, variable))
        keys.extend(expansions[weights].to_bytes(3, "little"))
        keys.append(len(weights))
    return bytes(keys), bytes(elements)


def digits(path):
    starts = []
    for line in path.read_text().splitlines():
        point, _, category, _, _, _, decimal, *_ = line.split(";")
        if category == "Nd" and decimal == "0":
            starts.append(int(point, 16))
    return b"".join(struct.pack("<I", point) for point in starts)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source_directory", type=Path)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    core = checked(args.source_directory / "core.zip", CORE_SHA)
    ucd = checked(args.source_directory / "UnicodeData.txt", UCD_SHA)
    keys, elements = collation(core)
    for name, data in [
        ("collation-keys.bin", keys),
        ("collation-elements.bin", elements),
        ("collation-digits.bin", digits(ucd)),
    ]:
        path = DATA / name
        if args.check:
            if path.read_bytes() != data:
                raise SystemExit(f"generated collation data differs: {path}")
        else:
            path.write_bytes(data)


if __name__ == "__main__":
    main()
