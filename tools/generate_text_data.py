#!/usr/bin/env python3
"""Generate rustic-text's Unicode segmentation and bidi tables and fixtures.

Inputs are exact Unicode 17.0.0 data files from the same pinned
github.com/unicode-org/unicodetools revision as the IDNA data (which mirrors
https://www.unicode.org/Public/). Every input is verified against the SHA-256
recorded in the committed manifest before anything is generated, so outputs
are reproducible offline.

Download the inputs (URLs are in SOURCES and the manifest) into one
directory, then run for example:

    python3 tools/generate_text_data.py --source-dir /tmp/ucd
    python3 tools/generate_text_data.py --source-dir /tmp/ucd --check

Outputs:
- crates/rustic-text/src/unicode/tables.rs: range tables for UAX #14 line
  breaking, UAX #29 grapheme clusters, UAX #9 bidi and UAX #50 vertical
  orientation.
- crates/rustic-text/tests/fixtures/*.txt: compact conformance cases. The
  line break and grapheme tests are complete; the two bidi tests are large,
  so a deterministic sample is committed and the full files can be run with
  `RUSTIC_UCD_DIR=<dir> cargo test -p rustic-text -- --ignored`.

See docs/TEXT.md ("Unicode data") for details.
"""

import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import sys


ROOT = Path(__file__).resolve().parents[1]
TABLES = ROOT / "crates/rustic-text/src/unicode/tables.rs"
FIXTURES = ROOT / "crates/rustic-text/tests/fixtures"
MANIFEST = FIXTURES / "text_data_manifest.json"

_SPEC = importlib.util.spec_from_file_location("rustic_idna_data",
                                               Path(__file__).resolve().parent / "generate_idna_data.py")
idna = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(idna)

UNICODE_VERSION = idna.UNICODE_VERSION
UNICODETOOLS_REVISION = idna.UNICODETOOLS_REVISION
UCD = f"{idna.UNICODE_BASE}/ucd/{UNICODE_VERSION}"
MAX_CODE_POINT = idna.MAX_CODE_POINT

SOURCES = {
    "UnicodeData.txt": f"{UCD}/UnicodeData.txt",
    "LineBreak.txt": f"{UCD}/LineBreak.txt",
    "EastAsianWidth.txt": f"{UCD}/EastAsianWidth.txt",
    "emoji-data.txt": f"{UCD}/emoji/emoji-data.txt",
    "GraphemeBreakProperty.txt": f"{UCD}/auxiliary/GraphemeBreakProperty.txt",
    "DerivedCoreProperties.txt": f"{UCD}/DerivedCoreProperties.txt",
    "DerivedBidiClass.txt": f"{UCD}/extracted/DerivedBidiClass.txt",
    "BidiBrackets.txt": f"{UCD}/BidiBrackets.txt",
    "BidiMirroring.txt": f"{UCD}/BidiMirroring.txt",
    "VerticalOrientation.txt": f"{UCD}/VerticalOrientation.txt",
    "LineBreakTest.txt": f"{UCD}/auxiliary/LineBreakTest.txt",
    "GraphemeBreakTest.txt": f"{UCD}/auxiliary/GraphemeBreakTest.txt",
    "BidiTest.txt": f"{UCD}/BidiTest.txt",
    "BidiCharacterTest.txt": f"{UCD}/BidiCharacterTest.txt",
}

LINE_BREAK_CLASSES = (
    "BK", "CR", "LF", "CM", "NL", "SG", "WJ", "ZW", "GL", "SP", "ZWJ", "B2", "BA", "BB",
    "HY", "CB", "CL", "CP", "EX", "IN", "NS", "OP", "QU", "IS", "NU", "PO", "PR", "SY",
    "AI", "AK", "AL", "AP", "AS", "CJ", "EB", "EM", "H2", "H3", "HL", "HH", "ID", "JL",
    "JV", "JT", "RI", "SA", "VF", "VI", "XX",
)
GRAPHEME_CLASSES = (
    "CR", "LF", "Control", "Extend", "ZWJ", "Regional_Indicator", "Prepend",
    "SpacingMark", "L", "V", "T", "LV", "LVT",
)
BIDI_CLASSES = (
    "L", "R", "AL", "EN", "ES", "ET", "AN", "CS", "NSM", "BN", "B", "S", "WS", "ON",
    "LRE", "LRO", "RLE", "RLO", "PDF", "LRI", "RLI", "FSI", "PDI",
)

# Deterministic sampling for the committed bidi fixtures: every Nth case.
BIDI_CHARACTER_STRIDE = 16
BIDI_CLASS_STRIDE = 64


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def variant(name: str) -> str:
    """`ZWJ` -> `Zwj`, `Regional_Indicator` -> `RegionalIndicator`."""
    return "".join(part[:1].upper() + part[1:].lower() for part in name.split("_"))


def load_sources(directory: Path, pins: dict[str, str]) -> dict[str, str]:
    data = {}
    for name, url in SOURCES.items():
        path = directory / name
        if not path.is_file():
            raise ValueError(f"missing input {path}; download {url}")
        content = path.read_bytes()
        expected = pins.get(name)
        if expected and sha256(content) != expected:
            raise ValueError(f"{name}: SHA-256 {sha256(content)} does not match pinned {expected}")
        data[name] = content.decode("utf-8")
    return data


def property_values(text: str, default: str) -> list[str]:
    """Expand a property file, applying its @missing defaults first."""
    return idna.parse_with_missing(text, default)


def flags(text: str, name: str) -> list[bool]:
    values = [False] * (MAX_CODE_POINT + 1)
    for fields in idna.data_lines(text):
        if fields[1] == name:
            start, end = idna.code_range(fields[0])
            values[start:end + 1] = [True] * (end - start + 1)
    return values


def range_table(name: str, kind: str, runs, render) -> list[str]:
    lines = ["#[rustfmt::skip]", f"pub(crate) static {name}: [{kind}; {len(runs)}] = ["]
    lines.extend(idna.wrap([render(run) for run in runs]))
    lines.extend(["];", ""])
    return lines


def generate_tables(text: dict[str, str]) -> tuple[str, dict]:
    category, _, _ = idna.parse_unicode_data(text["UnicodeData.txt"])
    line_break = property_values(text["LineBreak.txt"], "XX")
    unknown = sorted(set(line_break) - set(LINE_BREAK_CLASSES))
    if unknown:
        raise ValueError(f"new Line_Break values {unknown}; review UAX #14 changes")
    east_asian = property_values(text["EastAsianWidth.txt"], "N")
    pictographic = flags(text["emoji-data.txt"], "Extended_Pictographic")
    grapheme = property_values(text["GraphemeBreakProperty.txt"], "Other")
    unknown = sorted(set(grapheme) - set(GRAPHEME_CLASSES) - {"Other"})
    if unknown:
        raise ValueError(f"new Grapheme_Cluster_Break values {unknown}; review UAX #29")
    incb = ["None"] * (MAX_CODE_POINT + 1)
    for fields in idna.data_lines(text["DerivedCoreProperties.txt"]):
        if fields[1] == "InCB":
            start, end = idna.code_range(fields[0])
            incb[start:end + 1] = [fields[2]] * (end - start + 1)
    bidi = idna.parse_with_missing(text["DerivedBidiClass.txt"], "L", idna.BIDI_NAMES)
    unknown = sorted(set(bidi) - set(BIDI_CLASSES))
    if unknown:
        raise ValueError(f"new Bidi_Class values {unknown}; review UAX #9")

    vertical = property_values(text["VerticalOrientation.txt"], "R")
    unknown = sorted(set(vertical) - {"U", "R", "Tu", "Tr"})
    if unknown:
        raise ValueError(f"new Vertical_Orientation values {unknown}; review UAX #50")

    def gc(code: int) -> str:
        return category.get(code, "Cn")

    lb_runs = idna.ranges(line_break, lambda value: value != "XX")
    east_asian_runs = idna.ranges([value in ("F", "W", "H") for value in east_asian], bool)
    ambiguous_runs = idna.ranges([value == "A" for value in east_asian], bool)
    pictographic_unassigned = idna.ranges(
        [pictographic[code] and gc(code) == "Cn" for code in range(MAX_CODE_POINT + 1)], bool)
    pictographic_runs = idna.ranges(pictographic, bool)
    sa_marks = idna.ranges(
        [line_break[code] == "SA" and gc(code) in ("Mn", "Mc") for code in range(MAX_CODE_POINT + 1)],
        bool)
    quote_pi = idna.ranges(
        [line_break[code] == "QU" and gc(code) == "Pi" for code in range(MAX_CODE_POINT + 1)], bool)
    quote_pf = idna.ranges(
        [line_break[code] == "QU" and gc(code) == "Pf" for code in range(MAX_CODE_POINT + 1)], bool)
    grapheme_runs = idna.ranges(grapheme, lambda value: value != "Other")
    incb_runs = idna.ranges(incb, lambda value: value != "None")
    bidi_runs = idna.ranges(bidi, lambda value: value != "L")
    vertical_runs = idna.ranges(vertical, lambda value: value != "R")

    brackets = []
    for fields in idna.data_lines(text["BidiBrackets.txt"]):
        kind = {"o": "true", "c": "false"}[fields[2]]
        brackets.append((int(fields[0], 16), int(fields[1], 16), kind))
    brackets.sort()
    mirrors = sorted((int(fields[0], 16), int(fields[1], 16))
                     for fields in idna.data_lines(text["BidiMirroring.txt"]))

    def span(run) -> str:
        return f"(0x{run[0]:X}, 0x{run[1]:X})"

    out = [
        "// Generated by tools/generate_text_data.py. Do not edit by hand.",
        f"// Unicode {UNICODE_VERSION} data from unicode-org/unicodetools {UNICODETOOLS_REVISION};",
        "// Unicode License v3, see crates/rustic-text/UNICODE-LICENSE.md.",
        "",
        "use super::BidiClass as Bc;",
        "use super::GraphemeClass as Gc;",
        "use super::IndicConjunctBreak as Incb;",
        "use super::LineBreakClass as Lb;",
        "use super::VerticalOrientation as Vo;",
        "",
        f'pub(crate) const UNICODE_VERSION: &str = "{UNICODE_VERSION}";',
        "",
    ]
    out += range_table("LINE_BREAK", "(u32, u32, Lb)", lb_runs,
                       lambda r: f"(0x{r[0]:X}, 0x{r[1]:X}, Lb::{variant(r[2])})")
    out += range_table("EAST_ASIAN_FWH", "(u32, u32)", east_asian_runs, span)
    out += range_table("EAST_ASIAN_A", "(u32, u32)", ambiguous_runs, span)
    out += range_table("EXTENDED_PICTOGRAPHIC", "(u32, u32)", pictographic_runs, span)
    out += range_table("PICTOGRAPHIC_UNASSIGNED", "(u32, u32)", pictographic_unassigned, span)
    out += range_table("SA_MARKS", "(u32, u32)", sa_marks, span)
    out += range_table("QUOTE_INITIAL", "(u32, u32)", quote_pi, span)
    out += range_table("QUOTE_FINAL", "(u32, u32)", quote_pf, span)
    out += range_table("GRAPHEME", "(u32, u32, Gc)", grapheme_runs,
                       lambda r: f"(0x{r[0]:X}, 0x{r[1]:X}, Gc::{variant(r[2])})")
    out += range_table("INDIC_CONJUNCT_BREAK", "(u32, u32, Incb)", incb_runs,
                       lambda r: f"(0x{r[0]:X}, 0x{r[1]:X}, Incb::{variant(r[2])})")
    out += range_table("BIDI_CLASS", "(u32, u32, Bc)", bidi_runs,
                       lambda r: f"(0x{r[0]:X}, 0x{r[1]:X}, Bc::{variant(r[2])})")
    out += range_table("BIDI_BRACKETS", "(u32, u32, bool)", brackets,
                       lambda b: f"(0x{b[0]:X}, 0x{b[1]:X}, {b[2]})")
    out += range_table("BIDI_MIRRORING", "(u32, u32)", mirrors, span)
    out += range_table("VERTICAL_ORIENTATION", "(u32, u32, Vo)", vertical_runs,
                       lambda r: f"(0x{r[0]:X}, 0x{r[1]:X}, Vo::{variant(r[2])})")
    stats = {
        "line_break_runs": len(lb_runs),
        "grapheme_runs": len(grapheme_runs),
        "bidi_class_runs": len(bidi_runs),
        "bidi_brackets": len(brackets),
        "bidi_mirrors": len(mirrors),
        "vertical_orientation_runs": len(vertical_runs),
    }
    return "\n".join(out), stats


def header(description: str) -> list[str]:
    return [
        f"# Generated by tools/generate_text_data.py from Unicode {UNICODE_VERSION} {description}.",
        "# Unicode License v3, see crates/rustic-text/UNICODE-LICENSE.md.",
    ]


def segmentation_cases(text: str, description: str) -> tuple[str, int]:
    """`÷ 0041 × 0308 ÷` -> `0041 0308;0,2`: code points, then break positions."""
    lines = header(description) + [
        "# Format: code points;positions with a break opportunity (0..=len)."]
    count = 0
    for raw in text.splitlines():
        body = raw.split("#", 1)[0].strip()
        if not body:
            continue
        tokens = body.split()
        points, breaks = [], []
        for token in tokens:
            if token == "÷":
                breaks.append(str(len(points)))
            elif token == "×":
                continue
            else:
                points.append(token)
        lines.append(" ".join(points) + ";" + ",".join(breaks))
        count += 1
    return "\n".join(lines) + "\n", count


def bidi_character_cases(text: str) -> tuple[str, int, int]:
    lines = header(f"BidiCharacterTest.txt (every {BIDI_CHARACTER_STRIDE}th case)") + [
        "# Format: code points;direction (0 LTR, 1 RTL, 2 auto);paragraph level;levels;order."]
    upstream = selected = 0
    for raw in text.splitlines():
        body = raw.split("#", 1)[0].strip()
        if not body:
            continue
        if upstream % BIDI_CHARACTER_STRIDE == 0:
            lines.append(body)
            selected += 1
        upstream += 1
    return "\n".join(lines) + "\n", upstream, selected


def bidi_class_cases(text: str) -> tuple[str, int, int]:
    """Expand BidiTest.txt into `classes;bitset;levels;order` lines (sampled)."""
    lines = header(f"BidiTest.txt (every {BIDI_CLASS_STRIDE}th case)") + [
        "# Format: classes;paragraph bitset (1 auto, 2 LTR, 4 RTL);levels;order."]
    levels = order = ""
    upstream = selected = 0
    for raw in text.splitlines():
        body = raw.split("#", 1)[0].strip()
        if not body:
            continue
        if body.startswith("@Levels:"):
            levels = body[len("@Levels:"):].strip()
        elif body.startswith("@Reorder:"):
            order = body[len("@Reorder:"):].strip()
        elif body.startswith("@"):
            raise ValueError(f"unknown BidiTest.txt directive {body!r}")
        else:
            classes, bitset = (field.strip() for field in body.split(";"))
            if upstream % BIDI_CLASS_STRIDE == 0:
                lines.append(f"{classes};{bitset};{levels};{order}")
                selected += 1
            upstream += 1
    return "\n".join(lines) + "\n", upstream, selected


def generate(directory: Path, pins: dict[str, str]) -> dict[Path, str]:
    text = load_sources(directory, pins)
    tables, stats = generate_tables(text)
    line_cases, line_count = segmentation_cases(text["LineBreakTest.txt"], "LineBreakTest.txt")
    grapheme_cases, grapheme_count = segmentation_cases(text["GraphemeBreakTest.txt"],
                                                        "GraphemeBreakTest.txt")
    character_cases, character_upstream, character_selected = bidi_character_cases(
        text["BidiCharacterTest.txt"])
    class_cases, class_upstream, class_selected = bidi_class_cases(text["BidiTest.txt"])
    outputs = {
        TABLES: tables,
        FIXTURES / "line_break_cases.txt": line_cases,
        FIXTURES / "grapheme_cases.txt": grapheme_cases,
        FIXTURES / "bidi_character_cases.txt": character_cases,
        FIXTURES / "bidi_class_cases.txt": class_cases,
    }
    manifest = {
        "unicode_version": UNICODE_VERSION,
        "unicodetools_revision": UNICODETOOLS_REVISION,
        "sources": {name: {"url": SOURCES[name], "sha256": sha256(content.encode())}
                    for name, content in text.items()},
        "outputs": {str(path.relative_to(ROOT)): sha256(value.encode())
                    for path, value in outputs.items()},
        "tables": stats,
        "line_break_test": {"cases": line_count},
        "grapheme_break_test": {"cases": grapheme_count},
        "bidi_character_test": {"upstream_cases": character_upstream,
                                "selected_cases": character_selected,
                                "stride": BIDI_CHARACTER_STRIDE},
        "bidi_test": {"upstream_cases": class_upstream, "selected_cases": class_selected,
                      "stride": BIDI_CLASS_STRIDE},
        "license": "Unicode License v3 (crates/rustic-text/UNICODE-LICENSE.md)",
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
            print("Stale text data output: " + ", ".join(stale), file=sys.stderr)
            return 1
    else:
        for path, value in outputs.items():
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(value, encoding="utf-8", newline="\n")
    manifest = json.loads(outputs[MANIFEST])
    print(f"Unicode {UNICODE_VERSION} text data: {manifest['tables']['line_break_runs']} line break runs; "
          f"{manifest['line_break_test']['cases']} line break cases; "
          f"{manifest['grapheme_break_test']['cases']} grapheme cases; "
          f"{manifest['bidi_character_test']['selected_cases']} bidi character cases; "
          f"{manifest['bidi_test']['selected_cases']} bidi class cases.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
