#!/usr/bin/env python3
"""Extract static css-grid layout tests for Rustic's grid WPT comparison.

Like ``import_wpt_flexbox_tests.py`` (whose extraction this reuses), this
copies the pages under ``css/css-grid`` whose only script is
``checkLayout(selector)`` into ``cases.txt``, with scripts and ``onload``
removed. Stylesheets linked from ``/css/support/`` or relative to the page
are inlined when they live under ``css/`` in the checkout; the Ahem font
link stays (Rustic's test fonts do not include Ahem, so pages measuring
Ahem text are reviewed expected failures). The Rust test
``crates/rustic-layout/tests/wpt_grid.rs`` lays each page out and compares
its expectations against a reviewed expectation list.

Usage:
    tools/import_wpt_grid_tests.py --vendor /path/to/wpt   # regenerate
    tools/import_wpt_grid_tests.py --check                  # verify digests
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import import_wpt_flexbox_tests as flexbox  # noqa: E402

ROOT = Path(__file__).resolve().parents[1]
DESTINATION = ROOT / "crates/rustic-layout/tests/fixtures/wpt-grid"
CASES = DESTINATION / "cases.txt"
MANIFEST = DESTINATION / "manifest.json"
REPOSITORY = flexbox.REPOSITORY
REVISION = flexbox.REVISION
DIRECTORY = "css/css-grid"

LINK = re.compile(r"<link\b[^>]*>", re.IGNORECASE)
HREF = re.compile(r"""\bhref\s*=\s*(["']?)([^"'\s>]+)\1""", re.IGNORECASE)


def inline_styles(text: str, page: Path, wpt: Path) -> str:
    """Inline linked stylesheets that resolve inside the checkout's css/."""

    def replace(match: re.Match[str]) -> str:
        tag = match.group(0)
        href = HREF.search(tag)
        if not href or "stylesheet" not in tag.lower():
            return tag
        target = href.group(2)
        if target.startswith("/"):
            path = (wpt / target.lstrip("/")).resolve()
        else:
            path = (page.parent / target).resolve()
        css_root = (wpt / "css").resolve()
        if css_root not in path.parents or not path.is_file():
            return tag
        return "<style>\n" + path.read_text() + "</style>"

    return LINK.sub(replace, text)


def generate(wpt: Path) -> tuple[str, str]:
    parts, files, dynamic = [], [], []
    for path in sorted((wpt / DIRECTORY).rglob("*.html")):
        relative = path.relative_to(wpt).as_posix()
        if "/support/" in relative or ".tentative." in relative:
            continue
        raw = path.read_bytes()
        try:
            text = raw.decode("utf-8")
        except UnicodeDecodeError:
            continue
        if "checkLayout" not in text:
            continue
        found = flexbox.extract(inline_styles(text, path, wpt))
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
