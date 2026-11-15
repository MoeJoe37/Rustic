#!/usr/bin/env python3
"""Generate Rustic's time-zone tables from pinned IANA tzdata and zic.

Download tzdata2025b.tar.gz and tzcode2025b.tar.gz from
https://data.iana.org/time-zones/releases/, extract both to a scratch
directory and run 'make zic' there. Pass the tzdata archive and that zic
executable to this script. Use --check to verify the checked-in pack.
"""
import argparse
import hashlib
import json
import re
import struct
import subprocess
import tarfile
import tempfile
from pathlib import Path

from cldr_data import NONE, Pack

ROOT = Path(__file__).resolve().parents[1]
VERSION = "2025b"
SOURCES = ("africa", "antarctica", "asia", "australasia", "europe", "northamerica",
           "southamerica", "etcetera", "backward")
DATA_SHA = "11810413345fc7805017e27ea9fa4885fd74cd61b2911711ad038f5d28d71474"
CODE_SHA = "05f8fedb3525ee70d49c87d3fae78a8a0dbae4fe87aa565c65cda9948ae135ec"
DATA = "crates/rustic-js/data/intl/time-zones.bin"
MANIFEST = ROOT / "crates/rustic-js/tests/fixtures/js_intl_time_zones.json"
NAME = r"(?:[A-Za-z]{3,}|<[^>]+>)"
OFFSET = r"[+-]?\d+(?::\d+(?::\d+)?)?"
FOOTER = re.compile(rf"({NAME})({OFFSET})(?:({NAME})({OFFSET})?,([^,]+),([^,]+))?")


def digest(data):
    return hashlib.sha256(data).hexdigest()


def i32(value):
    return value & 0xFFFF_FFFF


def read_tzif(data):
    """Return type zero, the 64-bit transition list and the POSIX footer."""
    def header(at, width):
        marker, version, *counts = struct.unpack_from(">4sc15x6I", data, at)
        assert marker == b"TZif" and version in b"234"
        utc, std, leap, times, types, chars = counts
        assert not leap
        size = times * (width + 1) + types * 6 + chars + std + utc
        return counts, at + 44 + size

    _, second = header(0, 4)
    (_, _, _, n_times, n_types, n_chars), end = header(second, 8)
    start = second + 44
    times = struct.unpack_from(f">{n_times}q", data, start)
    flags = start + 8 * n_times
    type_start = flags + n_times
    names = data[type_start + 6 * n_types:][:n_chars]
    types = []
    for number in range(n_types):
        offset, dst, index = struct.unpack_from(">iBB", data, type_start + 6 * number)
        name = names[index:].split(b"\0", 1)[0].decode("ascii")
        types.append((offset, name, dst))
    transitions = [(time, types[flag]) for time, flag in zip(times, data[flags:type_start])]
    footer = data[end:].decode("ascii").strip("\n")
    return types[0], transitions, footer


def seconds(text):
    sign = -1 if text.startswith("-") else 1
    fields = [int(value) for value in text.lstrip("+-").split(":")]
    return sign * sum(value * unit for value, unit in zip(fields, (3600, 60, 1)))


def rule(text):
    day, sep, time = text.partition("/")
    at = seconds(time) if sep else 7200
    assert not sep or re.fullmatch(OFFSET, time), text
    if day.startswith("M"):
        month, week, weekday = map(int, day[1:].split("."))
        assert 1 <= month <= 12 and 1 <= week <= 5 and 0 <= weekday <= 6
        return (0, month, week, weekday, i32(at))
    return (int(not day.startswith("J")) + 1, int(day.lstrip("J")), 0, 0, i32(at))


def generate(archive, zic):
    pack = Pack()
    transitions, zone_rows, rule_rows, type_rows, future_rows = [], [], [], [], []

    def add_type(info):
        offset, abbreviation, dst = info
        row = (i32(offset), pack.s(abbreviation), dst)
        if row not in type_rows:
            type_rows.append(row)
        return type_rows.index(row)

    def future(text):
        if not text:
            return NONE
        parsed = FOOTER.fullmatch(text)
        if not parsed:
            raise ValueError(f"unhandled TZif footer: {text}")
        standard, offset, daylight, dst_offset, start, end = parsed.groups()
        offset = -seconds(offset)
        std = add_type((offset, standard.strip("<>"), 0))
        dst = begin = finish = NONE
        if daylight is not None:
            dst_offset = -seconds(dst_offset) if dst_offset else offset + 3600
            dst = add_type((dst_offset, daylight.strip("<>"), 1))
            begin = len(rule_rows)
            rule_rows.extend((rule(start), rule(end)))
            finish = begin + 1
        result = len(future_rows)
        future_rows.append((std, dst, begin, finish))
        return result

    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        links, zones = {}, set()
        for name in (*SOURCES, "zone.tab"):
            file = archive.extractfile(name)
            content = file.read()
            (tmp / name).write_bytes(content)
            if name == "zone.tab":
                primary = {line.split()[2] for line in content.decode().splitlines()
                           if line and not line.startswith("#")}
                continue
            for raw in content.decode().splitlines():
                row = raw.partition("#")[0].split()
                if row and row[0] == "Zone":
                    zones.add(row[1])
                if row and row[0] == "Link":
                    links[row[2]] = row[1]
        output = tmp / "out"
        subprocess.run([zic, "-b", "slim", "-d", str(output), *SOURCES], cwd=tmp, check=True)
        names = sorted(zones | links.keys(), key=str.lower)
        assert all(name.isascii() and len(name) <= 64 for name in names)
        definitions = {}
        identifiers = []
        for name in names:
            tzif = (output / name).read_bytes()
            if tzif not in definitions:
                initial, changes, footer = read_tzif(tzif)
                first = len(transitions)
                for when, info in changes:
                    transitions.append((i32(when), i32(when >> 32), add_type(info)))
                definitions[tzif] = len(zone_rows)
                zone_rows.append((add_type(initial), first, len(changes), future(footer)))
            identifiers.append((pack.s(name.lower()), pack.s(name), definitions[tzif]))
        # Current ECMA-402 preserves a supplied identifier and enumerates only
        # primary names. Keep zone.tab countries even when they share data.
        canonical = (zones | primary | {"UTC"}) - {"Etc/UTC", "Etc/GMT", "Factory"}
        assert canonical <= set(names)
        pack.table("primary", 1, [(pack.s(name),) for name in sorted(canonical)])
        pack.table("ids", 3, identifiers)

    pack.table("zones", 4, zone_rows)
    pack.table("types", 3, type_rows)
    pack.table("transitions", 3, transitions)
    pack.table("future", 4, future_rows)
    pack.table("rules", 5, rule_rows)
    return pack.bytes()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    parser.add_argument("zic", type=Path)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if digest(args.archive.read_bytes()) != DATA_SHA:
        raise SystemExit(f"expected IANA tzdata {VERSION}")
    zic = str(args.zic.resolve())
    if subprocess.check_output([zic, "--version"], text=True).split()[-1] != VERSION:
        raise SystemExit(f"expected IANA zic {VERSION}")
    with tarfile.open(args.archive) as archive:
        data = generate(archive, zic)
    manifest = {
        "version": VERSION,
        "license": "Public domain (crates/rustic-js/data/IANATZ-LICENSE.txt)",
        "sources": {
            "tzdata": {"url": f"https://data.iana.org/time-zones/releases/tzdata{VERSION}.tar.gz",
                       "sha256": DATA_SHA},
            "tzcode": {"url": f"https://data.iana.org/time-zones/releases/tzcode{VERSION}.tar.gz",
                       "sha256": CODE_SHA},
        },
        "outputs": {DATA: digest(data)},
    }
    manifest = (json.dumps(manifest, indent=2) + "\n").encode()
    for path, content in [(ROOT / DATA, data), (MANIFEST, manifest)]:
        if args.check:
            if path.read_bytes() != content:
                raise SystemExit(f"generated time zone data differs: {path}")
        else:
            path.write_bytes(content)


if __name__ == "__main__":
    main()
