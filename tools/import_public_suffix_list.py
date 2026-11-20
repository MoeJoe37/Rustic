#!/usr/bin/env python3
"""Vendor the Public Suffix List and its test vectors at a pinned revision.

The list (MPL-2.0) is embedded by `rustic_net::site::PublicSuffixList::
bundled()`; the upstream `tests/tests.txt` (public domain) drives
`crates/rustic-net/tests/public_suffix.rs`. ADR 0054 records the decision.

    python3 tools/import_public_suffix_list.py           # download and pin
    python3 tools/import_public_suffix_list.py --check   # verify digests only

To update, change REVISION, run without --check, and replace the digests
below with the ones it prints after reviewing the upstream diff.
"""

import argparse
import hashlib
import json
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REVISION = "d7d7f1fd55166aaf73ad2f526e72634490acd9c8"
FILES = {
    "crates/rustic-net/data/public_suffix_list.dat": (
        "public_suffix_list.dat",
        "3084c6ca4ac1dfb6e32461504b5dc494cc58ace521957e9e978ed0f870b2de94",
    ),
    "crates/rustic-net/tests/fixtures/psl_tests.txt": (
        "tests/tests.txt",
        "61a3a502cf471d1a919d5e43c10e910023b0c4230e1db506f8e2ff0b47d2234e",
    ),
}
MANIFEST = ROOT / "crates/rustic-net/data/psl_manifest.json"


def manifest() -> str:
    return json.dumps(
        {
            "upstream_repository": "publicsuffix/list",
            "upstream_revision": REVISION,
            "license": "MPL-2.0 (list); public domain (tests); third_party/licenses/public-suffix-list-MPL-2.0.txt",
            "generator": "tools/import_public_suffix_list.py",
            "sources": [
                {"path": path, "upstream_path": upstream, "sha256": digest}
                for path, (upstream, digest) in FILES.items()
            ],
        },
        indent=2,
    ) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    failed = False
    for path, (upstream, digest) in FILES.items():
        target = ROOT / path
        if not args.check:
            url = f"https://raw.githubusercontent.com/publicsuffix/list/{REVISION}/{upstream}"
            with urllib.request.urlopen(url) as response:
                target.write_bytes(response.read())
        actual = hashlib.sha256(target.read_bytes()).hexdigest()
        if actual != digest:
            print(f"{path}: sha256 {actual}, expected {digest}", file=sys.stderr)
            failed = True
    if args.check:
        if MANIFEST.read_text(encoding="utf-8") != manifest():
            print(f"{MANIFEST.relative_to(ROOT)} is stale", file=sys.stderr)
            failed = True
    elif not failed:
        MANIFEST.write_text(manifest(), encoding="utf-8")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
