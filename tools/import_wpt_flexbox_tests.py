#!/usr/bin/env python3
"""Extract static css-flexbox layout tests for Rustic's flexbox WPT comparison.

Many web-platform-tests in ``css/css-flexbox`` use ``check-layout-th.js``:
elements carry ``data-expected-width``, ``data-offset-x`` and similar
attributes, and one ``checkLayout(selector)`` call compares them with the
element's layout. When that call is the only script, the expectations are
static, so this tool copies those pages from a pinned WPT checkout into
``cases.txt`` with every ``<script>`` element and ``onload`` attribute
removed, and stylesheets linked from the directory's ``support/`` folder
inlined. Pages with any other script are left out and counted in the
manifest. The Rust test ``crates/rustic-layout/tests/wpt_flexbox.rs`` lays
each page out and compares the expectations against the reviewed
expectation list.

``cases.txt`` is plain text so the Rust test needs no JSON reader:

    #test <wpt path>
    #selector <checkLayout selector>      (one or more)
    <page HTML, scripts removed>
    #end

Usage:
    tools/import_wpt_flexbox_tests.py --vendor /path/to/wpt   # regenerate
    tools/import_wpt_flexbox_tests.py --check                  # verify digests
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DESTINATION = ROOT / "crates/rustic-layout/tests/fixtures/wpt-flexbox"
CASES = DESTINATION / "cases.txt"
MANIFEST = DESTINATION / "manifest.json"
REPOSITORY = "web-platform-tests/wpt"
# The same pinned revision as the CSS parsing import.
REVISION = "cb97c571c3b1947e08b8615eeb4028a2794b23a4"
DIRECTORY = "css/css-flexbox"

HARNESS = {
    "/resources/testharness.js",
    "/resources/testharnessreport.js",
    "/resources/check-layout-th.js",
}
SCRIPT = re.compile(r"<script\b([^>]*)>(.*?)</script\s*>", re.IGNORECASE | re.DOTALL)
SRC = re.compile(r"""\bsrc\s*=\s*(["']?)([^"'\s>]+)\1""", re.IGNORECASE)
CALL = re.compile(r"""checkLayout\(\s*(["'])((?:(?!\1).)*)\1\s*\)\s*;?""", re.DOTALL)
LINK = re.compile(r"<link\b[^>]*>", re.IGNORECASE)
HREF = re.compile(r"""\bhref\s*=\s*(["']?)([^"'\s>]+)\1""", re.IGNORECASE)
ONLOAD = re.compile(r"""\sonload\s*=\s*(["'])((?:(?!\1).)*)\1""", re.IGNORECASE | re.DOTALL)


def selectors_only(code: str) -> list[str] | None:
    """The selectors if `code` is nothing but checkLayout('...') calls."""
    found = []
    rest = code
    for match in CALL.finditer(code):
        found.append(match.group(2))
        rest = rest.replace(match.group(0), "", 1)
    if rest.strip() or not found:
        return None
    return found


def inline_support(text: str, directory: Path) -> str:
    """Replace stylesheet links to the test's own ``support/`` directory with
    their contents, since the Rust test loads no external resources."""

    def replace(match: re.Match[str]) -> str:
        tag = match.group(0)
        href = HREF.search(tag)
        if not href or "stylesheet" not in tag.lower():
            return tag
        target = href.group(2)
        if not target.startswith("support/") or ".." in target:
            return tag
        path = directory / target
        if not path.is_file():
            return tag
        return "<style>\n" + path.read_text() + "</style>"

    return LINK.sub(replace, text)


def extract(text: str) -> tuple[list[str], str] | None:
    """Selectors and the page without scripts, or None for a dynamic page."""
    if "checkLayout" not in text:
        return None
    selectors: list[str] = []
    for match in SCRIPT.finditer(text):
        attributes, code = match.group(1), match.group(2)
        src = SRC.search(attributes)
        if src:
            if src.group(2) not in HARNESS or code.strip():
                return None
            continue
        found = selectors_only(code)
        if found is None:
            return None
        selectors.extend(found)
    for match in ONLOAD.finditer(text):
        found = selectors_only(match.group(2))
        if found is None:
            return None
        selectors.extend(found)
    if not selectors:
        return None
    page = ONLOAD.sub("", SCRIPT.sub("", text))
    if any(line.startswith("#") for line in page.splitlines()):
        return None
    return selectors, page


def generate(wpt: Path) -> tuple[str, str]:
    parts, files, dynamic = [], [], []
    for path in sorted((wpt / DIRECTORY).glob("*.html")):
        relative = path.relative_to(wpt).as_posix()
        raw = path.read_bytes()
        try:
            text = raw.decode("utf-8")
        except UnicodeDecodeError:
            continue
        if "checkLayout" not in text:
            continue
        found = extract(inline_support(text, path.parent))
        if found is None:
            dynamic.append(relative)
            continue
        selectors, page = found
        files.append({"path": relative, "sha256": hashlib.sha256(raw).hexdigest()})
        parts.append(f"#test {relative}\n")
        parts.extend(f"#selector {s}\n" for s in selectors)
        parts.append(page if page.endswith("\n") else page + "\n")
        parts.append("#end\n")
    cases = "".join(parts)
    manifest = {
        "upstream_repository": REPOSITORY,
        "upstream_revision": REVISION,
        "license": "BSD-3-Clause (web-platform-tests); see LICENSE in this directory",
        "directory": DIRECTORY,
        "case_count": len(files),
        "skipped_dynamic": dynamic,
        "cases_sha256": hashlib.sha256(cases.encode()).hexdigest(),
        "files": files,
    }
    return cases, json.dumps(manifest, indent=2) + "\n"


def check() -> None:
    manifest = json.loads(MANIFEST.read_text())
    if manifest["upstream_revision"] != REVISION:
        raise ValueError("manifest revision differs from the importer's pinned revision")
    cases = CASES.read_bytes()
    if hashlib.sha256(cases).hexdigest() != manifest["cases_sha256"]:
        raise ValueError("cases.txt differs from the manifest; regenerate with --vendor")
    count = sum(1 for line in cases.decode().splitlines() if line.startswith("#test "))
    if count != manifest["case_count"]:
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
    cases, manifest = generate(options.vendor)
    DESTINATION.mkdir(parents=True, exist_ok=True)
    CASES.write_text(cases)
    MANIFEST.write_text(manifest)
    (DESTINATION / "LICENSE").write_bytes((options.vendor / "LICENSE.md").read_bytes())
    print(f"wrote {json.loads(manifest)['case_count']} cases")
    return 0


if __name__ == "__main__":
    sys.exit(main())
