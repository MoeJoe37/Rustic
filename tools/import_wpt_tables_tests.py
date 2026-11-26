#!/usr/bin/env python3
"""Extract static css-tables layout tests for Rustic's tables WPT comparison.

The same extraction as ``import_wpt_grid_tests.py`` (pages under
``css/css-tables`` whose only script is ``checkLayout(selector)``, scripts
removed, stylesheets under ``css/`` inlined) into
``crates/rustic-layout/tests/fixtures/wpt-tables``. The Rust test
``crates/rustic-layout/tests/wpt_tables.rs`` lays each page out and compares
its expectations against a reviewed expectation list.

Usage:
    tools/import_wpt_tables_tests.py --vendor /path/to/wpt   # regenerate
    tools/import_wpt_tables_tests.py --check                  # verify digests
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import import_wpt_grid_tests as grid  # noqa: E402

ROOT = Path(__file__).resolve().parents[1]
DESTINATION = ROOT / "crates/rustic-layout/tests/fixtures/wpt-tables"
CASES = DESTINATION / "cases.txt"
MANIFEST = DESTINATION / "manifest.json"
DIRECTORY = "css/css-tables"


def check() -> None:
    grid.check(CASES, MANIFEST)


if __name__ == "__main__":
    sys.exit(grid.main(DESTINATION, DIRECTORY, __doc__))
