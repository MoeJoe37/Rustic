"""Tests for the css-tables WPT importer, which reuses the css-grid one."""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import import_wpt_tables_tests as tables


class Fixture(unittest.TestCase):
    def test_the_vendored_fixture_matches_its_manifest(self) -> None:
        tables.check()

    def test_the_fixture_covers_the_css_tables_directory(self) -> None:
        manifest = (tables.MANIFEST).read_text()
        self.assertIn('"directory": "css/css-tables"', manifest)


if __name__ == "__main__":
    unittest.main()
