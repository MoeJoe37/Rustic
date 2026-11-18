#!/usr/bin/env python3
"""Generate rustic-encoding's label table and indexes from the Encoding Standard.

Inputs are `encodings.json` and the `index-*.txt` files of a pinned revision of
github.com/whatwg/encoding. Every input is verified against the SHA-256 pinned
in the committed manifest before anything is generated, so outputs are
reproducible offline.

Download the inputs into one directory, then run for example:

    git clone https://github.com/whatwg/encoding /tmp/encoding-src
    git -C /tmp/encoding-src checkout <ENCODING_REVISION>
    python3 tools/generate_encoding_data.py --source-dir /tmp/encoding-src
    python3 tools/generate_encoding_data.py --source-dir /tmp/encoding-src --check

`--repin` records new input digests after a deliberate revision change; review
the generated diff before committing it. See docs/ENCODING.md.
"""

import argparse
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
CRATE = ROOT / "crates/rustic-encoding"
TABLES = CRATE / "src/tables.rs"
DATA = CRATE / "data"
MANIFEST = DATA / "manifest.json"

ENCODING_REVISION = "be17ac5c49881336538acfc7b40dba3c8009144b"

# Encoding name (as in encodings.json) -> Rust variant name.
VARIANTS = {
    "UTF-8": "Utf8",
    "IBM866": "Ibm866",
    "ISO-8859-2": "Iso8859_2",
    "ISO-8859-3": "Iso8859_3",
    "ISO-8859-4": "Iso8859_4",
    "ISO-8859-5": "Iso8859_5",
    "ISO-8859-6": "Iso8859_6",
    "ISO-8859-7": "Iso8859_7",
    "ISO-8859-8": "Iso8859_8",
    "ISO-8859-8-I": "Iso8859_8I",
    "ISO-8859-10": "Iso8859_10",
    "ISO-8859-13": "Iso8859_13",
    "ISO-8859-14": "Iso8859_14",
    "ISO-8859-15": "Iso8859_15",
    "ISO-8859-16": "Iso8859_16",
    "KOI8-R": "Koi8R",
    "KOI8-U": "Koi8U",
    "macintosh": "Macintosh",
    "windows-874": "Windows874",
    "windows-1250": "Windows1250",
    "windows-1251": "Windows1251",
    "windows-1252": "Windows1252",
    "windows-1253": "Windows1253",
    "windows-1254": "Windows1254",
    "windows-1255": "Windows1255",
    "windows-1256": "Windows1256",
    "windows-1257": "Windows1257",
    "windows-1258": "Windows1258",
    "x-mac-cyrillic": "XMacCyrillic",
    "GBK": "Gbk",
    "gb18030": "Gb18030",
    "Big5": "Big5",
    "EUC-JP": "EucJp",
    "ISO-2022-JP": "Iso2022Jp",
    "Shift_JIS": "ShiftJis",
    "EUC-KR": "EucKr",
    "replacement": "Replacement",
    "UTF-16BE": "Utf16Be",
    "UTF-16LE": "Utf16Le",
    "x-user-defined": "XUserDefined",
}

# Single-byte encodings and the index each one reads. ISO-8859-8-I shares
# ISO-8859-8's index (Encoding Standard §9).
SINGLE_BYTE = {
    name: f"index-{name.lower()}.txt"
    for name in [
        "IBM866", "ISO-8859-2", "ISO-8859-3", "ISO-8859-4", "ISO-8859-5", "ISO-8859-6",
        "ISO-8859-7", "ISO-8859-8", "ISO-8859-10", "ISO-8859-13", "ISO-8859-14",
        "ISO-8859-15", "ISO-8859-16", "KOI8-R", "KOI8-U", "macintosh", "windows-874",
        "windows-1250", "windows-1251", "windows-1252", "windows-1253", "windows-1254",
        "windows-1255", "windows-1256", "windows-1257", "windows-1258", "x-mac-cyrillic",
    ]
}
SINGLE_BYTE["ISO-8859-8-I"] = "index-iso-8859-8.txt"

# Multi-byte indexes stored as little-endian binary arrays: file -> (output, width, length).
# A zero entry means "no code point"; no index maps a pointer to U+0000.
MULTI_BYTE = {
    "index-big5.txt": ("index-big5.bin", 4, 19782),
    "index-euc-kr.txt": ("index-euc-kr.bin", 2, 23750),
    "index-gb18030.txt": ("index-gb18030.bin", 2, 23940),
    "index-jis0208.txt": ("index-jis0208.bin", 2, 11104),
    "index-jis0212.txt": ("index-jis0212.bin", 2, 7211),
}

SOURCES = (
    ["encodings.json", "LICENSE", "index-gb18030-ranges.txt", "index-iso-2022-jp-katakana.txt"]
    + sorted(set(SINGLE_BYTE.values()))
    + sorted(MULTI_BYTE)
)


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def load_sources(directory: Path, pins: dict[str, str] | None) -> dict[str, bytes]:
    sources = {}
    for name in SOURCES:
        data = (directory / name).read_bytes()
        if pins is not None:
            expected = pins.get(name)
            if expected != sha256(data):
                raise SystemExit(f"{name}: SHA-256 {sha256(data)} does not match pin {expected}")
        sources[name] = data
    return sources


def parse_index(text: str) -> dict[int, int]:
    """Pointer -> code point, as the Encoding Standard's "Indexes" section reads them."""
    entries = {}
    for line in text.split("\n"):
        if not line or line.startswith("#"):
            continue
        fields = line.split("\t")
        pointer = int(fields[0].strip())
        code_point = int(fields[1].strip(), 16)
        if pointer in entries:
            raise SystemExit(f"duplicate pointer {pointer}")
        if code_point == 0:
            raise SystemExit("an index maps a pointer to U+0000, which the tables use as null")
        entries[pointer] = code_point
    return entries


def wrap(items: list[str], indent: str = "    ", width: int = 100) -> list[str]:
    lines, current = [], indent
    for item in items:
        piece = item + ","
        if len(current) + len(piece) + 1 > width and current.strip():
            lines.append(current.rstrip())
            current = indent
        current += piece + " "
    if current.strip():
        lines.append(current.rstrip())
    return lines


def generate(sources: dict[str, bytes]) -> tuple[str, dict[str, bytes], dict]:
    encodings = json.loads(sources["encodings.json"])
    names = [encoding["name"] for group in encodings for encoding in group["encodings"]]
    if set(names) != set(VARIANTS) or len(names) != len(VARIANTS):
        raise SystemExit(f"encodings.json names differ from VARIANTS: {sorted(set(names) ^ set(VARIANTS))}")
    labels = []
    for group in encodings:
        for encoding in group["encodings"]:
            for label in encoding["labels"]:
                labels.append((label, VARIANTS[encoding["name"]]))
    labels.sort()
    if len({label for label, _ in labels}) != len(labels):
        raise SystemExit("duplicate label")

    out = [
        "// @generated by tools/generate_encoding_data.py from whatwg/encoding",
        f"// revision {ENCODING_REVISION}. Do not edit by hand.",
        "",
        "use crate::Encoding;",
        "",
        f"/// The Encoding Standard revision these tables were generated from.",
        f'pub const ENCODING_STANDARD_REVISION: &str = "{ENCODING_REVISION}";',
        "",
        "/// Every encoding in the order the Encoding Standard lists them.",
        f"pub(crate) const ALL: [Encoding; {len(names)}] = [",
    ]
    out += wrap([f"Encoding::{VARIANTS[name]}" for name in names])
    out += [
        "];",
        "",
        "/// Each encoding's name, indexed by `Encoding as usize`.",
        f"pub(crate) const NAMES: [&str; {len(names)}] = [",
    ]
    out += wrap([json.dumps(name) for name in names])
    out += [
        "];",
        "",
        "/// Every label, sorted by its ASCII bytes, with the encoding it selects.",
        f"pub(crate) const LABELS: [(&str, Encoding); {len(labels)}] = [",
    ]
    out += [f"    ({json.dumps(label)}, Encoding::{variant})," for label, variant in labels]
    out += ["];", ""]

    single_names = {
        name: file.removeprefix("index-").removesuffix(".txt").upper().replace("-", "_")
        for name, file in SINGLE_BYTE.items()
    }
    emitted = set()
    for name, file in sorted(SINGLE_BYTE.items(), key=lambda item: item[1]):
        constant = single_names[name]
        if constant in emitted:
            continue
        emitted.add(constant)
        index = parse_index(sources[file].decode("utf-8"))
        if max(index) > 127 or max(index.values()) > 0xFFFF:
            raise SystemExit(f"{file}: not a single-byte index")
        values = [f"0x{index.get(pointer, 0):04X}" for pointer in range(128)]
        out.append(f"/// `{file}`: code points for bytes 0x80 to 0xFF; 0 means unmapped.")
        out.append(f"pub(crate) static {constant}: [u16; 128] = [")
        out += wrap(values)
        out += ["];", ""]
    out.append("/// The single-byte index of `encoding`, or `None` for other encodings.")
    out.append("pub(crate) fn single_byte_index(encoding: Encoding) -> Option<&'static [u16; 128]> {")
    out.append("    match encoding {")
    for name, constant in single_names.items():
        out.append(f"        Encoding::{VARIANTS[name]} => Some(&{constant}),")
    out += ["        _ => None,", "    }", "}", ""]

    ranges = parse_index(sources["index-gb18030-ranges.txt"].decode("utf-8"))
    out.append("/// `index-gb18030-ranges.txt`: (pointer, code point) pairs sorted by both.")
    out.append(f"pub(crate) static GB18030_RANGES: [(u32, u32); {len(ranges)}] = [")
    out += wrap([f"({pointer}, 0x{code:04X})" for pointer, code in sorted(ranges.items())])
    out += ["];", ""]
    pointers = sorted(ranges)
    if [ranges[p] for p in pointers] != sorted(ranges.values()):
        raise SystemExit("gb18030 ranges are not monotonic")

    katakana = parse_index(sources["index-iso-2022-jp-katakana.txt"].decode("utf-8"))
    if sorted(katakana) != list(range(63)):
        raise SystemExit("ISO-2022-JP katakana index is not dense")
    out.append("/// `index-iso-2022-jp-katakana.txt`: fullwidth katakana for U+FF61 to U+FF9F.")
    out.append("pub(crate) static ISO_2022_JP_KATAKANA: [u16; 63] = [")
    out += wrap([f"0x{katakana[pointer]:04X}" for pointer in range(63)])
    out += ["];", ""]

    binaries = {}
    counts = {}
    for file, (output, width, length) in MULTI_BYTE.items():
        index = parse_index(sources[file].decode("utf-8"))
        if max(index) + 1 != length:
            raise SystemExit(f"{file}: expected {length} pointers, found {max(index) + 1}")
        fmt = "<I" if width == 4 else "<H"
        if width == 2 and max(index.values()) > 0xFFFF:
            raise SystemExit(f"{file}: needs 32-bit entries")
        binaries[output] = b"".join(struct.pack(fmt, index.get(pointer, 0)) for pointer in range(length))
        constant = output.removeprefix("index-").removesuffix(".bin").upper().replace("-", "_")
        rust_type = "u32" if width == 4 else "u16"
        counts[file] = len(index)
        out.append(f"/// `{file}` as little-endian `{rust_type}` entries for pointers 0 to {length - 1};")
        out.append("/// 0 means unmapped.")
        out.append(f'pub(crate) static {constant}: &[u8; {length * width}] = include_bytes!("../data/{output}");')
        out.append("")

    manifest = {
        "encoding_standard": {
            "repository": "https://github.com/whatwg/encoding",
            "revision": ENCODING_REVISION,
        },
        "license": "CC BY 4.0 for the data; BSD-3-Clause where incorporated into source "
                   "(crates/rustic-encoding/data/WHATWG-LICENSE.txt)",
        "counts": {"encodings": len(names), "labels": len(labels), **counts},
    }
    source = "\n".join(out).rstrip() + "\n"
    # Format with the pinned toolchain's rustfmt so `cargo fmt --check` accepts the output.
    source = subprocess.run(
        ["rustfmt", "--edition", "2024", "--emit", "stdout"],
        input=source, capture_output=True, text=True, check=True,
    ).stdout
    return source, binaries, manifest


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--source-dir", type=Path, required=True)
    parser.add_argument("--check", action="store_true", help="fail if outputs differ")
    parser.add_argument("--repin", action="store_true", help="record new input digests")
    args = parser.parse_args()

    old = json.loads(MANIFEST.read_text()) if MANIFEST.is_file() else {}
    pins = None if args.repin or not old else {k: v["sha256"] for k, v in old["sources"].items()}
    sources = load_sources(args.source_dir, pins)
    tables, binaries, manifest = generate(sources)

    outputs = {TABLES: tables.encode("utf-8"), DATA / "WHATWG-LICENSE.txt": sources["LICENSE"]}
    outputs.update({DATA / name: data for name, data in binaries.items()})
    manifest["sources"] = {name: {"sha256": sha256(data)} for name, data in sorted(sources.items())}
    manifest["outputs"] = {
        str(path.relative_to(ROOT)): sha256(data) for path, data in sorted(outputs.items())
    }
    outputs[MANIFEST] = (json.dumps(manifest, indent=2, sort_keys=True) + "\n").encode("utf-8")

    stale = [path for path, data in outputs.items() if not path.is_file() or path.read_bytes() != data]
    if args.check:
        for path in stale:
            print(f"{path.relative_to(ROOT)} is stale; rerun tools/generate_encoding_data.py", file=sys.stderr)
        return 1 if stale else 0
    DATA.mkdir(parents=True, exist_ok=True)
    for path in stale:
        path.write_bytes(outputs[path])
        print(f"wrote {path.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
