#!/usr/bin/env python3
"""Generate rustic-url's UTS #46 IDNA tables and conformance fixtures.

Inputs are exact Unicode data files from a pinned revision of
github.com/unicode-org/unicodetools (which mirrors https://www.unicode.org/Public/)
plus WPT's url/resources/toascii.json. Every input is verified against a pinned
SHA-256 before anything is generated, so outputs are reproducible offline.

Download the inputs into one directory, then run for example:

    python3 tools/generate_idna_data.py --source-dir /tmp/idna-src
    python3 tools/generate_idna_data.py --source-dir /tmp/idna-src --check

See docs/URL.md ("Unicode IDNA data") for the exact download URLs.
"""

import argparse
import hashlib
import json
from pathlib import Path
import re
import sys


ROOT = Path(__file__).resolve().parents[1]
TABLES = ROOT / "crates/rustic-url/src/idna/tables.rs"
NORMALIZATION = ROOT / "crates/rustic-base/src/unicode/tables.rs"
FIXTURES = ROOT / "crates/rustic-url/tests/fixtures"
MANIFEST = FIXTURES / "idna_manifest.json"

UNICODE_VERSION = "17.0.0"
UNICODETOOLS_REVISION = "63aff0ca8dea894a057d04e7aef7ae1241757680"
WPT_REVISION = "57bbaa12ff3515848cc0b3d3791791086a563047"
UNICODE_BASE = (f"https://raw.githubusercontent.com/unicode-org/unicodetools/"
                f"{UNICODETOOLS_REVISION}/unicodetools/data")
WPT_BASE = f"https://raw.githubusercontent.com/web-platform-tests/wpt/{WPT_REVISION}"

# File name -> download URL. Input SHA-256 pins live in the committed manifest;
# changing a revision means rerunning with --repin and reviewing the diff.
SOURCES = {
    "IdnaMappingTable.txt": f"{UNICODE_BASE}/idna/{UNICODE_VERSION}/IdnaMappingTable.txt",
    "IdnaTestV2.txt": f"{UNICODE_BASE}/idna/{UNICODE_VERSION}/IdnaTestV2.txt",
    "UnicodeData.txt": f"{UNICODE_BASE}/ucd/{UNICODE_VERSION}/UnicodeData.txt",
    "DerivedBidiClass.txt": f"{UNICODE_BASE}/ucd/{UNICODE_VERSION}/extracted/DerivedBidiClass.txt",
    "DerivedJoiningType.txt": f"{UNICODE_BASE}/ucd/{UNICODE_VERSION}/extracted/DerivedJoiningType.txt",
    "DerivedNormalizationProps.txt": f"{UNICODE_BASE}/ucd/{UNICODE_VERSION}/DerivedNormalizationProps.txt",
    "NormalizationTest.txt": f"{UNICODE_BASE}/ucd/{UNICODE_VERSION}/NormalizationTest.txt",
    "toascii.json": f"{WPT_BASE}/url/resources/toascii.json",
}

# Status codes that UTS #46 says to ignore for flags the URL Standard sets to
# false when beStrict is false (CheckHyphens, UseSTD3ASCIIRules,
# VerifyDnsLength). X4_2 is the empty-label compatibility code that replaced
# A4_2 in toUnicode, so it follows VerifyDnsLength too.
IGNORED_STATUSES = frozenset({"V2", "V3", "U1", "A4_1", "A4_2", "X4_2"})

BIDI_NAMES = {
    "Left_To_Right": "L", "Right_To_Left": "R", "Arabic_Letter": "AL",
    "European_Number": "EN", "European_Separator": "ES", "European_Terminator": "ET",
    "Arabic_Number": "AN", "Common_Separator": "CS", "Nonspacing_Mark": "NSM",
    "Boundary_Neutral": "BN", "Paragraph_Separator": "B", "Segment_Separator": "S",
    "White_Space": "WS", "Other_Neutral": "ON", "Left_To_Right_Embedding": "LRE",
    "Left_To_Right_Override": "LRO", "Right_To_Left_Embedding": "RLE",
    "Right_To_Left_Override": "RLO", "Pop_Directional_Format": "PDF",
    "Left_To_Right_Isolate": "LRI", "Right_To_Left_Isolate": "RLI",
    "First_Strong_Isolate": "FSI", "Pop_Directional_Isolate": "PDI",
}
# Classes RFC 5893 names individually; every other class is "Other" there.
BIDI_VARIANTS = {
    "L": "L", "R": "R", "AL": "Al", "EN": "En", "ES": "Es", "ET": "Et",
    "AN": "An", "CS": "Cs", "NSM": "Nsm", "BN": "Bn", "ON": "On",
}
JOINING_VARIANTS = {"L": "Left", "D": "Dual", "R": "Right", "T": "Transparent"}
MAX_CODE_POINT = 0x10FFFF


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def load_sources(directory: Path, pins: dict[str, str]) -> dict[str, bytes]:
    data = {}
    for name in SOURCES:
        path = directory / name
        if not path.is_file():
            raise ValueError(f"missing input {path}; download {SOURCES[name]}")
        content = path.read_bytes()
        expected = pins.get(name)
        if expected and sha256(content) != expected:
            raise ValueError(f"{name}: SHA-256 {sha256(content)} does not match pinned {expected}")
        data[name] = content
    return data


def data_lines(text: str):
    for line in text.splitlines():
        line = line.split("#", 1)[0].strip()
        if line:
            yield [field.strip() for field in line.split(";")]


def code_range(field: str) -> tuple[int, int]:
    start, _, end = field.partition("..")
    return int(start, 16), int(end or start, 16)


def code_points(field: str) -> str:
    return "".join(chr(int(value, 16)) for value in field.split())


# ---------------------------------------------------------------------------
# Unicode property parsing


def parse_unicode_data(text: str):
    """Return (general category, canonical combining class, canonical decomposition)."""
    category: dict[int, str] = {}
    combining: dict[int, int] = {}
    decomposition: dict[int, list[int]] = {}
    pending_first = None
    for line in text.splitlines():
        fields = line.split(";")
        code = int(fields[0], 16)
        name = fields[1]
        if name.endswith(", First>"):
            pending_first = code
            continue
        start = code
        if name.endswith(", Last>"):
            start = pending_first
            pending_first = None
        for value in range(start, code + 1):
            category[value] = fields[2]
            if fields[3] != "0":
                combining[value] = int(fields[3])
        if fields[5] and not fields[5].startswith("<"):
            decomposition[code] = [int(value, 16) for value in fields[5].split()]
    return category, combining, decomposition


def parse_full_composition_exclusion(text: str) -> set[int]:
    excluded = set()
    for fields in data_lines(text):
        if len(fields) >= 2 and fields[1] == "Full_Composition_Exclusion":
            start, end = code_range(fields[0])
            excluded.update(range(start, end + 1))
    return excluded


def parse_with_missing(text: str, default: str, names: dict[str, str] | None = None) -> list[str]:
    """Expand a derived property file, applying its @missing defaults in order."""
    values = [default] * (MAX_CODE_POINT + 1)
    for line in text.splitlines():
        match = re.match(r"#\s*@missing:\s*([0-9A-F.]+)\s*;\s*(\w+)", line)
        if match:
            start, end = code_range(match.group(1))
            value = match.group(2)
            value = names.get(value, value) if names else value
            values[start:end + 1] = [value] * (end - start + 1)
    for fields in data_lines(text):
        start, end = code_range(fields[0])
        values[start:end + 1] = [fields[1]] * (end - start + 1)
    return values


def parse_idna_mapping(text: str):
    statuses: list[tuple[str, str] | None] = [None] * (MAX_CODE_POINT + 1)
    for fields in data_lines(text):
        start, end = code_range(fields[0])
        status = fields[1]
        mapping = code_points(fields[2]) if len(fields) > 2 and fields[2] else ""
        if status not in {"valid", "ignored", "mapped", "deviation", "disallowed"}:
            raise ValueError(f"unknown IDNA status {status!r}; review UTS #46 changes")
        if status == "mapped" and not mapping:
            raise ValueError(f"mapped range {fields[0]} has no mapping")
        for value in range(start, end + 1):
            if statuses[value] is not None:
                raise ValueError(f"overlapping IDNA ranges at U+{value:04X}")
            statuses[value] = (status, mapping)
    missing = [value for value, status in enumerate(statuses) if status is None]
    if missing:
        raise ValueError(f"IDNA mapping table does not cover U+{missing[0]:04X}")
    return statuses


# ---------------------------------------------------------------------------
# Rust rendering helpers

UNSAFE_CATEGORIES = {"Cc", "Cf", "Zs", "Zl", "Zp", "Co", "Cs", "Cn"}


def literal(value: str, category: dict[int, str]) -> str:
    pieces = ['"']
    for char in value:
        code = ord(char)
        if char == '"':
            pieces.append('\\"')
        elif char == "\\":
            pieces.append("\\\\")
        elif char == "\n":
            pieces.append("\\n")
        elif char == "\t":
            pieces.append("\\t")
        elif 0x20 <= code < 0x7F:
            pieces.append(char)
        elif code < 0x80 or category.get(code, "Cn") in UNSAFE_CATEGORIES:
            pieces.append(f"\\u{{{code:x}}}")
        else:
            pieces.append(char)
    return "".join(pieces) + '"'


def ranges(values: list, keep=lambda value: True) -> list[tuple[int, int, object]]:
    """Collapse a full code point array into (start, end, value) runs."""
    runs = []
    start = 0
    for code in range(1, MAX_CODE_POINT + 2):
        if code > MAX_CODE_POINT or values[code] != values[start]:
            if keep(values[start]):
                runs.append((start, code - 1, values[start]))
            start = code
    return runs


class Pool:
    """Deduplicated UTF-8 string pool addressed by byte offset and length."""

    def __init__(self) -> None:
        self.text = ""
        self.offsets: dict[str, int] = {}

    def add(self, value: str) -> tuple[int, int]:
        if value not in self.offsets:
            found = self.text.find(value)
            if found >= 0:
                self.offsets[value] = len(self.text[:found].encode())
            else:
                self.offsets[value] = len(self.text.encode())
                self.text += value
        return self.offsets[value], len(value.encode())


def wrap(items: list[str], indent: str = "    ", width: int = 100) -> list[str]:
    lines, current = [], indent
    for item in items:
        candidate = item + ","
        if len(current) + len(candidate) + 1 > width and current.strip():
            lines.append(current.rstrip())
            current = indent
        current += candidate + " "
    if current.strip():
        lines.append(current.rstrip())
    return lines


def pool_literal(name: str, pool: Pool, category: dict[int, str]) -> list[str]:
    lines = [f"pub(super) const {name}: &str = concat!("]
    chunk = ""
    for char in pool.text:
        chunk += char
        if len(chunk) >= 40:
            lines.append(f"    {literal(chunk, category)},")
            chunk = ""
    if chunk:
        lines.append(f"    {literal(chunk, category)},")
    lines.append(");")
    return lines


# ---------------------------------------------------------------------------
# Table generation


def generate_tables(sources: dict[str, bytes]) -> tuple[str, dict, dict[int, str]]:
    text = {name: content.decode("utf-8") for name, content in sources.items()}
    category, combining, decomposition = parse_unicode_data(text["UnicodeData.txt"])
    exclusions = parse_full_composition_exclusion(text["DerivedNormalizationProps.txt"])
    statuses = parse_idna_mapping(text["IdnaMappingTable.txt"])
    bidi = parse_with_missing(text["DerivedBidiClass.txt"], "L", BIDI_NAMES)
    joining = parse_with_missing(text["DerivedJoiningType.txt"], "U",
                                 {"Non_Joining": "U"})

    mapping_pool = Pool()
    status_values = []
    for status, mapping in statuses:
        if status == "mapped":
            offset, length = mapping_pool.add(mapping)
            status_values.append(f"Mapped({offset}, {length})")
        else:
            status_values.append(status.capitalize())
    # A run of equal values shares one entry; for Mapped that means every code
    # point in the run maps to the same replacement string.
    mapping_runs = ranges(status_values)

    compatibility = dict(decomposition)
    for fields in (line.split(";") for line in text["UnicodeData.txt"].splitlines()):
        if fields[5].startswith("<"):
            compatibility[int(fields[0], 16)] = [int(part, 16) for part in fields[5].split()[1:]]

    def expand(mapping: dict[int, list[int]], code: int):
        for part in mapping[code]:
            if part in mapping:
                yield from expand(mapping, part)
            else:
                yield part

    def decomposition_table(mapping: dict[int, list[int]], omit_canonical: bool = False):
        pool = Pool()
        entries = []
        for code in sorted(mapping):
            expanded = list(expand(mapping, code))
            if omit_canonical and code in decomposition and expanded == list(expand(decomposition, code)):
                continue
            offset, length = pool.add("".join(chr(part) for part in expanded))
            entries.append(f"(0x{code:X}, {offset}, {length})")
        if len(pool.text.encode()) > 0xFFFF or max(
                len(value.encode()) for value in pool.offsets) > 0xFF:
            raise ValueError("decomposition pool no longer fits (u16, u8) addressing")
        return entries, pool

    decomposition_entries, decomposition_pool = decomposition_table(decomposition)
    compatibility_entries, compatibility_pool = decomposition_table(compatibility, omit_canonical=True)

    compositions = []
    for code, parts in decomposition.items():
        if len(parts) == 2 and code not in exclusions:
            compositions.append((parts[0], parts[1], code))
    compositions.sort()

    combining_runs = ranges([combining.get(code, 0) for code in range(MAX_CODE_POINT + 1)],
                            lambda value: value != 0)
    mark_runs = ranges([category.get(code, "Cn").startswith("M")
                        for code in range(MAX_CODE_POINT + 1)], bool)
    bidi_runs = ranges([BIDI_VARIANTS.get(value, "Other") for value in bidi])
    joining_runs = ranges([JOINING_VARIANTS.get(value) for value in joining],
                          lambda value: value is not None)

    out = [
        "// Generated by tools/generate_idna_data.py. Do not edit by hand.",
        f"// Unicode {UNICODE_VERSION} data from unicode-org/unicodetools {UNICODETOOLS_REVISION};",
        "// Unicode License v3, see crates/rustic-url/UNICODE-LICENSE.md.",
        "",
        "use super::bidi::BidiClass::{self, *};",
        "use super::mapping::Status::{self, *};",
        "use super::validity::JoiningType::{self, *};",
        "",
        f'pub(super) const UNICODE_VERSION: &str = "{UNICODE_VERSION}";',
        "",
        "/// UTS #46 status runs as (first code point, status), covering U+0000..=U+10FFFF.",
        "#[rustfmt::skip]",
        "pub(super) static IDNA_STATUS: &[(u32, Status)] = &[",
        *wrap([f"(0x{start:X}, {value})" for start, _, value in mapping_runs]),
        "];",
        "",
        "/// UTF-8 replacement text for `Status::Mapped(byte offset, byte length)`.",
        "#[rustfmt::skip]",
        *pool_literal("IDNA_MAPPINGS", mapping_pool, category),
        "",
        "/// General_Category=Mark (Mn, Mc, Me) as inclusive runs.",
        "#[rustfmt::skip]",
        "pub(super) static MARKS: &[(u32, u32)] = &[",
        *wrap([f"(0x{a:X}, 0x{b:X})" for a, b, _ in mark_runs]),
        "];",
        "",
        "/// Bidi_Class runs as (first code point, class), covering U+0000..=U+10FFFF.",
        "#[rustfmt::skip]",
        "pub(super) static BIDI_CLASSES: &[(u32, BidiClass)] = &[",
        *wrap([f"(0x{a:X}, {value})" for a, _, value in bidi_runs]),
        "];",
        "",
        "/// Joining_Type L, D, R and T as inclusive runs; everything else is non-joining.",
        "#[rustfmt::skip]",
        "pub(super) static JOINING_TYPES: &[(u32, u32, JoiningType)] = &[",
        *wrap([f"(0x{a:X}, 0x{b:X}, {value})" for a, b, value in joining_runs]),
        "];",
        "",
    ]
    normalization = [
        "// Generated by tools/generate_idna_data.py. Do not edit by hand.",
        f"// Unicode {UNICODE_VERSION} data from unicode-org/unicodetools {UNICODETOOLS_REVISION};",
        "// Unicode License v3, see crates/rustic-url/UNICODE-LICENSE.md.",
        "",
        "/// Canonical decompositions as (code point, byte offset, byte length), fully expanded.",
        "#[rustfmt::skip]",
        "pub(super) static DECOMPOSITIONS: &[(u32, u16, u8)] = &[",
        *wrap(decomposition_entries),
        "];",
        "",
        "#[rustfmt::skip]",
        *pool_literal("DECOMPOSITION_TEXT", decomposition_pool, category),
        "",
        "/// Primary composites as (first, second, composite), sorted by (first, second).",
        "#[rustfmt::skip]",
        "pub(super) static COMPOSITIONS: &[(u32, u32, u32)] = &[",
        *wrap([f"(0x{a:X}, 0x{b:X}, 0x{c:X})" for a, b, c in compositions]),
        "];",
        "",
        "/// Non-zero canonical combining classes as inclusive (first, last, class) runs.",
        "#[rustfmt::skip]",
        "pub(super) static COMBINING_CLASSES: &[(u32, u32, u8)] = &[",
        *wrap([f"(0x{a:X}, 0x{b:X}, {value})" for a, b, value in combining_runs]),
        "];",
        "",
        "/// Compatibility decompositions that differ from the canonical mapping.",
        "#[rustfmt::skip]",
        "pub(super) static COMPATIBILITY: &[(u32, u16, u8)] = &[",
        *wrap(compatibility_entries),
        "];",
        "",
        "#[rustfmt::skip]",
        *pool_literal("COMPATIBILITY_TEXT", compatibility_pool, category),
        "",
    ]
    if len(mapping_pool.text.encode()) > 0xFFFF or max(
            len(value.encode()) for value in mapping_pool.offsets) > 0xFF:
        raise ValueError("mapping pool no longer fits (u16, u8) addressing")
    stats = {
        "idna_status_runs": len(mapping_runs),
        "idna_mapping_bytes": len(mapping_pool.text.encode()),
        "decompositions": len(decomposition_entries),
        "compatibility_decompositions": len(compatibility_entries),
        "compositions": len(compositions),
        "combining_class_runs": len(combining_runs),
        "mark_runs": len(mark_runs),
        "bidi_runs": len(bidi_runs),
        "joining_runs": len(joining_runs),
    }
    return "\n".join(out), "\n".join(normalization), stats, category


# ---------------------------------------------------------------------------
# Conformance fixtures


def unescape_test_field(value: str) -> str:
    value = re.sub(r"\\x\{([0-9A-Fa-f]+)\}", lambda m: chr(int(m.group(1), 16)), value)
    value = re.sub(r"\\u([0-9A-Fa-f]{4})", lambda m: chr(int(m.group(1), 16)), value)
    return '' if value == '""' else value


def statuses(value: str) -> set[str]:
    value = value.strip()
    if not value or value == "[]":
        return set()
    return {item.strip() for item in value.strip("[]").split(",") if item.strip()}


def generate_idna_cases(text: str, category: dict[int, str]) -> tuple[str, dict]:
    lines = [
        "// Generated by tools/generate_idna_data.py. Do not edit by hand.",
        f"// Unicode {UNICODE_VERSION} IdnaTestV2.txt; Unicode License v3, see crates/rustic-url/UNICODE-LICENSE.md.",
        "use super::Case;",
        "",
        "#[rustfmt::skip]",
        "pub const CASES: &[Case] = &[",
    ]
    total = selected = surrogates = ascii_ok = ascii_err = 0
    for number, raw in enumerate(text.splitlines(), 1):
        body = raw.split("#", 1)[0]
        if not body.strip():
            continue
        columns = [column.strip() for column in body.split(";")]
        columns += [""] * (7 - len(columns))
        total += 1
        source = unescape_test_field(columns[0])
        if any(0xD800 <= ord(char) <= 0xDFFF for char in "".join(columns)) or re.search(
                r"\\u[dD][89a-fA-F][0-9a-fA-F]{2}", body):
            surrogates += 1  # Rust strings cannot hold ill-formed UTF-16.
            continue
        to_unicode = unescape_test_field(columns[1]) if columns[1] else source
        unicode_status = statuses(columns[2]) - IGNORED_STATUSES
        to_ascii = unescape_test_field(columns[3]) if columns[3] else to_unicode
        ascii_status = (statuses(columns[4]) if columns[4] else statuses(columns[2]))
        ascii_status -= IGNORED_STATUSES
        selected += 1
        if ascii_status:
            ascii_err += 1
        else:
            ascii_ok += 1
        expected_ascii = "None" if ascii_status else f"Some({literal(to_ascii, category)})"
        expected_unicode = "None" if unicode_status else f"Some({literal(to_unicode, category)})"
        lines.append(f"    Case {{ line: {number}, input: {literal(source, category)}, "
                     f"to_unicode: {expected_unicode}, to_ascii: {expected_ascii} }},")
    lines.extend(["];", ""])
    return "\n".join(lines), {
        "upstream_cases": total,
        "selected_cases": selected,
        "skipped_ill_formed_utf16": surrogates,
        "to_ascii_success": ascii_ok,
        "to_ascii_failure": ascii_err,
        "ignored_statuses": sorted(IGNORED_STATUSES),
    }


def generate_normalization_cases(text: str, category: dict[int, str]) -> tuple[str, dict]:
    """Pack every NFC conformance pair as "source[TAB expected]" entries split by LF."""
    pairs = set()
    for raw in text.splitlines():
        body = raw.split("#", 1)[0].strip()
        if not body or body.startswith("@"):
            continue
        columns = [code_points(column) for column in body.split(";")[:5]]
        # NormalizationTest.txt: NFC(c1) = NFC(c2) = NFC(c3) = c2 and NFC(c4) = NFC(c5) = c4.
        for source in (columns[0], columns[1], columns[2]):
            pairs.add((source, columns[1]))
        for source in (columns[3], columns[4]):
            pairs.add((source, columns[3]))
    entries = [source if source == expected else f"{source}\t{expected}"
               for source, expected in sorted(pairs)]
    lines = [
        "// Generated by tools/generate_idna_data.py. Do not edit by hand.",
        f"// Unicode {UNICODE_VERSION} NormalizationTest.txt NFC pairs; an entry without a TAB",
        "// is its own NFC. Unicode License v3, see crates/rustic-url/UNICODE-LICENSE.md.",
        "",
        f"pub const NFC_CASE_COUNT: usize = {len(entries)};",
        "",
        "#[rustfmt::skip]",
        "pub const NFC_CASES: &str = concat!(",
    ]
    chunk = ""
    for entry in entries:
        chunk += entry + "\n"
        if len(chunk) >= 30:
            lines.append(f"    {literal(chunk, category)},")
            chunk = ""
    if chunk:
        lines.append(f"    {literal(chunk, category)},")
    lines.extend([");", ""])
    return "\n".join(lines), {"nfc_checks": len(entries)}


def generate_toascii_cases(source: bytes, category: dict[int, str]) -> tuple[str, dict]:
    upstream = json.loads(source)
    lines = [
        "// Generated by tools/generate_idna_data.py. Do not edit by hand.",
        f"// WPT url/resources/toascii.json at {WPT_REVISION}; BSD-3-Clause, see WPT-LICENSE.md.",
        "",
        "#[rustfmt::skip]",
        "pub const TOASCII_CASES: &[(&str, Option<&str>)] = &[",
    ]
    count = failures = 0
    for case in upstream:
        if not isinstance(case, dict):
            continue  # Upstream comment strings.
        if any(0xD800 <= ord(char) <= 0xDFFF for char in case["input"]):
            raise ValueError("toascii.json now contains ill-formed UTF-16; review selection")
        output = case["output"]
        count += 1
        failures += output is None
        expected = "None" if output is None else f"Some({literal(output, category)})"
        lines.append(f"    ({literal(case['input'], category)}, {expected}),")
    lines.extend(["];", ""])
    return "\n".join(lines), {"toascii_cases": count, "toascii_failures": failures}


def generate(directory: Path, pins: dict[str, str]) -> dict[Path, str]:
    sources = load_sources(directory, pins)
    tables, normalization, table_stats, category = generate_tables(sources)
    idna_cases, idna_stats = generate_idna_cases(sources["IdnaTestV2.txt"].decode("utf-8"), category)
    nfc_cases, nfc_stats = generate_normalization_cases(
        sources["NormalizationTest.txt"].decode("utf-8"), category)
    toascii_cases, toascii_stats = generate_toascii_cases(sources["toascii.json"], category)
    outputs = {
        TABLES: tables,
        NORMALIZATION: normalization,
        FIXTURES / "idna_cases.rs": idna_cases,
        FIXTURES / "nfc_cases.rs": nfc_cases,
        FIXTURES / "toascii_cases.rs": toascii_cases,
    }
    manifest = {
        "unicode_version": UNICODE_VERSION,
        "unicodetools_revision": UNICODETOOLS_REVISION,
        "wpt_revision": WPT_REVISION,
        "sources": {name: {"url": SOURCES[name], "sha256": sha256(content)}
                    for name, content in sources.items()},
        "outputs": {str(path.relative_to(ROOT)): sha256(value.encode())
                    for path, value in outputs.items()},
        "tables": table_stats,
        "idna_test_v2": idna_stats,
        "normalization_test": nfc_stats,
        "wpt_toascii": toascii_stats,
        "license": "Unicode License v3 (crates/rustic-url/UNICODE-LICENSE.md); WPT BSD-3-Clause (WPT-LICENSE.md)",
    }
    outputs[MANIFEST] = json.dumps(manifest, indent=2, sort_keys=True) + "\n"
    return outputs


def pinned_hashes() -> dict[str, str]:
    """Pinned input digests are the ones recorded in the committed manifest."""
    if not MANIFEST.exists():
        return {}
    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    return {name: entry["sha256"] for name, entry in manifest["sources"].items()}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--source-dir", required=True, type=Path)
    parser.add_argument("--check", action="store_true", help="verify outputs without writing")
    parser.add_argument("--repin", action="store_true",
                        help="accept new input digests (only when deliberately updating versions)")
    args = parser.parse_args()
    pins = {} if args.repin else pinned_hashes()
    try:
        outputs = generate(args.source_dir, pins)
    except ValueError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    if args.check:
        stale = [str(path.relative_to(ROOT)) for path, value in outputs.items()
                 if not path.exists() or path.read_text(encoding="utf-8") != value]
        if stale:
            print("Stale IDNA output: " + ", ".join(stale), file=sys.stderr)
            return 1
    else:
        for path, value in outputs.items():
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(value, encoding="utf-8", newline="\n")
    manifest = json.loads(outputs[MANIFEST])
    print(f"Unicode {UNICODE_VERSION} IDNA data: {manifest['tables']['idna_status_runs']} status runs; "
          f"{manifest['idna_test_v2']['selected_cases']} IdnaTestV2 cases; "
          f"{manifest['normalization_test']['nfc_checks']} NFC checks; "
          f"{manifest['wpt_toascii']['toascii_cases']} WPT toascii cases.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
