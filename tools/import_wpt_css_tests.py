#!/usr/bin/env python3
"""Extract the declared WPT CSS parsing cases for Rustic's CSS conformance gate.

The web-platform-tests parsing tests are HTML pages that call helpers such as
``test_valid_value("color", "red")`` from JavaScript. Rustic cannot run them,
but most calls take only string literals, so this tool reads the calls
statically from a pinned WPT checkout and writes them to ``cases.json`` as
arrays (the test's JSON reader has no objects):

    [kind, file, property, input, expected]

    kind       valid-value | invalid-value | valid-selector | invalid-selector
    file       the WPT file the case came from
    property   the property ("" for selectors)
    input      the value or selector text
    expected   acceptable serializations (valid cases; empty means the input)

Calls whose arguments are not plain literals (variables, concatenation,
template substitutions) are counted per file in the manifest as skipped,
never guessed. The Rust test ``crates/rustic-css/tests/wpt_conformance.rs``
decides which cases are in scope (supported properties) and compares the
results against the reviewed expectation list.

Usage:
    tools/import_wpt_css_tests.py --vendor /path/to/wpt   # regenerate fixtures
    tools/import_wpt_css_tests.py --check                  # verify digests
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DESTINATION = ROOT / "crates/rustic-css/tests/fixtures/wpt-css"
CASES = DESTINATION / "cases.json"
MANIFEST = DESTINATION / "manifest.json"
REPOSITORY = "web-platform-tests/wpt"
REVISION = "cb97c571c3b1947e08b8615eeb4028a2794b23a4"

# Directories whose parsing tests cover M2's declared CSS scope.
DIRECTORIES = (
    "css/selectors/parsing",
    "css/css-pseudo/parsing",
    "css/css-backgrounds/parsing",
    "css/css-box/parsing",
    "css/css-cascade/parsing",
    "css/css-color/parsing",
    "css/css-contain/parsing",
    "css/css-display/parsing",
    "css/css-fonts/parsing",
    "css/css-overflow/parsing",
    "css/css-position/parsing",
    "css/css-sizing/parsing",
    "css/css-text/parsing",
)

FUNCTIONS = {
    "test_valid_value": ("valid-value", 2),
    "test_invalid_value": ("invalid-value", 2),
    "test_valid_selector": ("valid-selector", 1),
    "test_invalid_selector": ("invalid-selector", 1),
}


class NotLiteral(Exception):
    """A call argument that is not a plain string or array of strings."""


def skip_space(text: str, i: int) -> int:
    while i < len(text):
        if text[i].isspace():
            i += 1
        elif text.startswith("//", i):
            end = text.find("\n", i)
            i = len(text) if end < 0 else end
        elif text.startswith("/*", i):
            end = text.find("*/", i + 2)
            if end < 0:
                raise NotLiteral
            i = end + 2
        else:
            break
    return i


ESCAPES = {"n": "\n", "t": "\t", "r": "\r", "b": "\b", "f": "\f", "v": "\v", "0": "\0"}


def string(text: str, i: int) -> tuple[str, int]:
    quote = text[i]
    out = []
    i += 1
    while i < len(text):
        c = text[i]
        if c == quote:
            return "".join(out), i + 1
        if quote == "`" and text.startswith("${", i):
            raise NotLiteral
        if c == "\\":
            i += 1
            e = text[i]
            if e == "u" and text[i + 1] == "{":
                end = text.index("}", i)
                out.append(chr(int(text[i + 2:end], 16)))
                i = end + 1
                continue
            if e == "u":
                code = int(text[i + 1:i + 5], 16)
                i += 5
                # Join a UTF-16 surrogate pair.
                if 0xD800 <= code < 0xDC00 and text.startswith("\\u", i):
                    low = int(text[i + 2:i + 6], 16)
                    if 0xDC00 <= low < 0xE000:
                        code = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00)
                        i += 6
                out.append(chr(code))
                continue
            if e == "x":
                out.append(chr(int(text[i + 1:i + 3], 16)))
                i += 3
                continue
            if e == "\n":
                i += 1
                continue
            out.append(ESCAPES.get(e, e))
            i += 1
            continue
        if c == "\n" and quote != "`":
            raise NotLiteral
        out.append(c)
        i += 1
    raise NotLiteral


def value(text: str, i: int) -> tuple[object, int]:
    i = skip_space(text, i)
    if i >= len(text):
        raise NotLiteral
    if text[i] in "'\"`":
        return string(text, i)
    if text[i] == "[":
        items = []
        i = skip_space(text, i + 1)
        while text[i] != "]":
            item, i = value(text, i)
            if not isinstance(item, str):
                raise NotLiteral
            items.append(item)
            i = skip_space(text, i)
            if text[i] == ",":
                i = skip_space(text, i + 1)
            elif text[i] != "]":
                raise NotLiteral
        return items, i + 1
    raise NotLiteral


def arguments(text: str, i: int) -> tuple[list[object], int]:
    """Literal arguments of a call whose `(` is just before `i`."""
    args = []
    i = skip_space(text, i)
    while text[i] != ")":
        arg, i = value(text, i)
        args.append(arg)
        i = skip_space(text, i)
        if text[i] == ",":
            i = skip_space(text, i + 1)
        elif text[i] != ")":
            raise NotLiteral
    return args, i + 1


def extract(relative: str, text: str) -> tuple[list[dict], int]:
    cases, skipped = [], 0
    for name, (kind, required) in FUNCTIONS.items():
        start = 0
        needle = name + "("
        while (at := text.find(needle, start)) >= 0:
            start = at + len(needle)
            before = text[at - 1] if at else " "
            if before.isalnum() or before in "_.$":
                continue
            # Skip the helper definitions themselves.
            if text[max(0, at - 9):at].endswith("function "):
                continue
            try:
                args, _ = arguments(text, start)
            except (NotLiteral, IndexError, ValueError):
                skipped += 1
                continue
            if len(args) < required or not all(isinstance(a, str) for a in args[:required]):
                skipped += 1
                continue
            case = {"kind": kind, "file": relative}
            rest = args[required:]
            if kind.endswith("value"):
                case["property"] = args[0]
                case["input"] = args[1]
            else:
                case["input"] = args[0]
            if kind.startswith("valid"):
                expected = rest[0] if rest else []
                if isinstance(expected, str):
                    expected = [expected]
                case["expected"] = expected
            cases.append(case)
    return cases, skipped


def generate(wpt: Path) -> tuple[str, str]:
    cases, files = [], []
    for directory in DIRECTORIES:
        for path in sorted((wpt / directory).glob("*.html")):
            relative = path.relative_to(wpt).as_posix()
            raw = path.read_bytes()
            found, skipped = extract(relative, raw.decode("utf-8"))
            if found or skipped:
                files.append({
                    "path": relative,
                    "sha256": hashlib.sha256(raw).hexdigest(),
                    "cases": len(found),
                    "skipped_calls": skipped,
                })
            cases.extend(found)
    rows = [[c["kind"], c["file"], c.get("property", ""), c["input"], c.get("expected", [])]
            for c in cases]
    cases_json = "[\n" + ",\n".join(json.dumps(r, ensure_ascii=False) for r in rows) + "\n]\n"
    manifest = {
        "upstream_repository": REPOSITORY,
        "upstream_revision": REVISION,
        "license": "BSD-3-Clause (web-platform-tests); see LICENSE in this directory",
        "directories": list(DIRECTORIES),
        "case_count": len(cases),
        "skipped_calls": sum(f["skipped_calls"] for f in files),
        "cases_sha256": hashlib.sha256(cases_json.encode()).hexdigest(),
        "files": files,
    }
    return cases_json, json.dumps(manifest, indent=2) + "\n"


def check() -> None:
    manifest = json.loads(MANIFEST.read_text())
    if manifest["upstream_revision"] != REVISION:
        raise ValueError("manifest revision differs from the importer's pinned revision")
    digest = hashlib.sha256(CASES.read_bytes()).hexdigest()
    if digest != manifest["cases_sha256"]:
        raise ValueError("cases.json differs from the manifest; regenerate with --vendor")
    cases = json.loads(CASES.read_text())
    if len(cases) != manifest["case_count"]:
        raise ValueError("case count differs from the manifest")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--vendor", type=Path, help=f"checkout of {REPOSITORY} at {REVISION}")
    group.add_argument("--check", action="store_true", help="verify the vendored digests")
    options = parser.parse_args()
    if options.check:
        check()
        return 0
    head = subprocess.run(
        ["git", "-C", str(options.vendor), "rev-parse", "HEAD"],
        check=True, capture_output=True, text=True,
    ).stdout.strip()
    if head != REVISION:
        print(f"checkout is at {head}, expected {REVISION}", file=sys.stderr)
        return 1
    cases_json, manifest = generate(options.vendor)
    DESTINATION.mkdir(parents=True, exist_ok=True)
    CASES.write_text(cases_json)
    MANIFEST.write_text(manifest)
    (DESTINATION / "LICENSE").write_bytes((options.vendor / "LICENSE.md").read_bytes())
    print(f"wrote {len(json.loads(cases_json))} cases")
    return 0


if __name__ == "__main__":
    sys.exit(main())
