"""Tests for the css-grid WPT importer's stylesheet inlining and digests."""

import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import import_wpt_grid_tests as grid


class InlineStyles(unittest.TestCase):
    def test_css_links_inside_the_checkout_are_inlined(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "css/support").mkdir(parents=True)
            (root / "css/css-grid/a").mkdir(parents=True)
            (root / "css/support/grid.css").write_text(".grid { display: grid }")
            (root / "fonts").mkdir()
            (root / "fonts/ahem.css").write_text("@font-face {}")
            page = root / "css/css-grid/a/t.html"
            text = (
                '<link rel="stylesheet" href="/css/support/grid.css">'
                '<link rel="stylesheet" href="../../support/grid.css">'
                '<link rel="stylesheet" href="/fonts/ahem.css">'
                '<link rel="help" href="/css/support/grid.css">'
            )
            out = grid.inline_styles(text, page, root)
            self.assertEqual(out.count("<style>"), 2)
            self.assertIn('href="/fonts/ahem.css"', out)
            self.assertIn('rel="help"', out)

    def test_the_vendored_fixture_matches_its_manifest(self) -> None:
        grid.check()


if __name__ == "__main__":
    unittest.main()
