from pathlib import Path
import sys
import tempfile
import unittest


sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import import_wpt_flexbox_tests as wpt  # noqa: E402


HARNESS = (
    '<script src="/resources/testharness.js"></script>\n'
    "<script src='/resources/check-layout-th.js'></script>\n"
)


class ExtractTests(unittest.TestCase):
    def test_static_pages_keep_selectors_and_drop_scripts(self):
        text = HARNESS + '<body onload="checkLayout(\'.a\')"><div class=a data-expected-width=5>'
        selectors, page = wpt.extract(text)
        self.assertEqual(selectors, [".a"])
        self.assertNotIn("<script", page)
        self.assertNotIn("onload", page)
        self.assertIn("data-expected-width=5", page)

    def test_inline_calls_are_collected(self):
        text = HARNESS + "<div></div><script>\ncheckLayout('#x');\ncheckLayout(\"#y\")\n</script>"
        selectors, _ = wpt.extract(text)
        self.assertEqual(selectors, ["#x", "#y"])

    def test_any_other_script_makes_the_page_dynamic(self):
        for text in [
            HARNESS + "<script>document.body.offsetTop; checkLayout('.a');</script>",
            HARNESS + "<script src='/other.js'></script><script>checkLayout('.a')</script>",
            HARNESS + "<body onload=\"go(); checkLayout('.a')\">",
            HARNESS + "<p>no layout check</p>",
            HARNESS + "<script>checkLayout('.a')</script>\n#test injected",
        ]:
            self.assertIsNone(wpt.extract(text), text)

    def test_support_stylesheets_are_inlined(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            (directory / "support").mkdir()
            (directory / "support/flexbox.css").write_text(".flexbox { display: flex }\n")
            text = (
                '<link href="support/flexbox.css" rel="stylesheet">'
                '<link rel="stylesheet" href="/fonts/ahem.css">'
                '<link rel="stylesheet" href="support/../../x.css">'
            )
            page = wpt.inline_support(text, directory)
            self.assertIn("<style>\n.flexbox { display: flex }\n</style>", page)
            self.assertIn('href="/fonts/ahem.css"', page)
            self.assertIn("support/../../x.css", page)


class CheckTests(unittest.TestCase):
    def test_committed_fixture_matches_its_manifest(self):
        wpt.check()


if __name__ == "__main__":
    unittest.main()
