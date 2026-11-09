import importlib.util
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("rustic_text_data", ROOT / "tools/generate_text_data.py")
text_data = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(text_data)


class GenerateTextDataTests(unittest.TestCase):
    def test_variant_names_match_rust_enums(self):
        self.assertEqual(text_data.variant("ZWJ"), "Zwj")
        self.assertEqual(text_data.variant("Regional_Indicator"), "RegionalIndicator")
        self.assertEqual(text_data.variant("NSM"), "Nsm")
        self.assertEqual(text_data.variant("H2"), "H2")

    def test_segmentation_cases_record_break_positions(self):
        source = "# header\n÷ 0041 × 0308 ÷ 0020 ÷\t#  comment\n"
        cases, count = text_data.segmentation_cases(source, "test")
        self.assertEqual(count, 1)
        self.assertTrue(cases.splitlines()[-1] == "0041 0308 0020;0,2,3")

    def test_bidi_class_cases_expand_directives_and_sample(self):
        source = "@Levels: x 0\n@Reorder: 1\nLRE L; 7\nRLE L; 7\n@Levels: 1\n@Reorder: 0\nR; 4\n"
        cases, upstream, selected = text_data.bidi_class_cases(source)
        self.assertEqual(upstream, 3)
        self.assertEqual(selected, 1)
        self.assertIn("LRE L;7;x 0;1", cases)
        with self.assertRaisesRegex(ValueError, "unknown BidiTest.txt directive"):
            text_data.bidi_class_cases("@Other: 1\n")

    def test_committed_outputs_match_manifest(self):
        import hashlib
        import json
        manifest = json.loads(text_data.MANIFEST.read_text(encoding="utf-8"))
        for output, digest in manifest["outputs"].items():
            self.assertEqual(hashlib.sha256((ROOT / output).read_bytes()).hexdigest(), digest)
        self.assertEqual(manifest["line_break_test"]["cases"], 19338)


if __name__ == "__main__":
    unittest.main()
