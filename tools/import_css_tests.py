#!/usr/bin/env python3
"""Vendor and verify the pinned CSS syntax parsing corpus, offline.

The corpus is SimonSapin/css-parsing-tests (CC0-1.0). The CSS Syntax
entry-point files and the An+B microsyntax file are vendored; colour and
byte-stream files belong to later tasks. Files are copied byte for byte; the Rust runner reads them.
"""

import argparse
import hashlib
import json
from pathlib import Path
import shutil

ROOT = Path(__file__).resolve().parents[1]
CORPUS = ROOT / "crates/rustic-css/tests/fixtures/css-parsing-tests"
MANIFEST = CORPUS / "manifest.json"
REPOSITORY = "https://github.com/SimonSapin/css-parsing-tests"
REVISION = "203ce36bffd617db7f118c551e32794561fb273d"
SUITE_FILES = (
    "component_value_list.json",
    "one_component_value.json",
    "declaration_list.json",
    "blocks_contents.json",
    "one_declaration.json",
    "one_rule.json",
    "rule_list.json",
    "stylesheet.json",
    "An+B.json",
)
NOTICE = "LICENSE"


def cases(raw: bytes) -> int:
    data = json.loads(raw.decode("utf-8"))
    if not isinstance(data, list) or len(data) % 2:
        raise ValueError("expected an even-length JSON array of input/output pairs")
    return len(data) // 2


def manifest() -> str:
    files = []
    for name in SUITE_FILES:
        raw = (CORPUS / name).read_bytes()
        files.append({"path": name, "sha256": hashlib.sha256(raw).hexdigest(), "cases": cases(raw)})
    notice = (CORPUS / NOTICE).read_bytes()
    metadata = {
        "upstream_repository": REPOSITORY,
        "upstream_revision": REVISION,
        "license": "CC0-1.0; LICENSE",
        "license_sha256": hashlib.sha256(notice).hexdigest(),
        "files": files,
        "cases": sum(f["cases"] for f in files),
    }
    return json.dumps(metadata, indent=2) + "\n"


def vendor(source: Path) -> None:
    CORPUS.mkdir(parents=True, exist_ok=True)
    for name in (*SUITE_FILES, NOTICE):
        shutil.copyfile(source / name, CORPUS / name)
    MANIFEST.write_text(manifest())


def check() -> None:
    if not MANIFEST.is_file():
        raise ValueError("missing CSS corpus manifest")
    if MANIFEST.read_text() != manifest():
        raise ValueError("CSS corpus files do not match the manifest; re-run with --vendor")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--vendor", type=Path, help=f"checkout of {REPOSITORY} at {REVISION}")
    group.add_argument("--check", action="store_true", help="verify vendored files offline")
    args = parser.parse_args()
    if args.vendor:
        vendor(args.vendor)
    else:
        check()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
