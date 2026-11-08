from pathlib import Path
import sys
import unittest


sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import import_wpt_css_tests as wpt  # noqa: E402


class ExtractTests(unittest.TestCase):
    def test_literal_calls_become_cases(self):
        text = """
        test_valid_value("color", "red");
        test_valid_value('width', 'calc(1px)', ["calc(1px)", "1px"]);
        test_invalid_value(`margin`, `1px 2px 3px 4px 5px`);
        test_valid_selector(":is(a, b)", ':is(a, b)');
        test_invalid_selector("a::before b");
        """
        cases, skipped = wpt.extract("f.html", text)
        self.assertEqual(skipped, 0)
        self.assertIn({"kind": "valid-value", "file": "f.html", "property": "color",
                       "input": "red", "expected": []}, cases)
        self.assertIn({"kind": "valid-value", "file": "f.html", "property": "width",
                       "input": "calc(1px)", "expected": ["calc(1px)", "1px"]}, cases)
        self.assertIn({"kind": "invalid-value", "file": "f.html", "property": "margin",
                       "input": "1px 2px 3px 4px 5px"}, cases)
        self.assertIn({"kind": "valid-selector", "file": "f.html", "input": ":is(a, b)",
                       "expected": [":is(a, b)"]}, cases)
        self.assertIn({"kind": "invalid-selector", "file": "f.html",
                       "input": "a::before b"}, cases)

    def test_non_literal_calls_are_skipped_not_guessed(self):
        text = """
        function test_valid_value(property, value) {}
        test_valid_value("color", value);
        test_valid_value("color", "a" + "b");
        test_valid_value("color", `${x}`);
        object.test_valid_value("color", "red");
        """
        cases, skipped = wpt.extract("f.html", text)
        self.assertEqual(cases, [])
        self.assertEqual(skipped, 3)

    def test_escapes_decode_like_javascript(self):
        cases, _ = wpt.extract("f.html", r'test_valid_selector("é\ud83d\ude00\x41\n");')
        self.assertEqual(cases[0]["input"], "\u00e9\U0001F600A\n")

    def test_vendored_fixtures_match_the_manifest(self):
        wpt.check()


if __name__ == "__main__":
    unittest.main()
