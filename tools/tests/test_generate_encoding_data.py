import hashlib
import importlib.util
import json
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("rustic_encoding_data", ROOT / "tools/generate_encoding_data.py")
encoding = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(encoding)


class GenerateEncodingDataTests(unittest.TestCase):
    def test_index_lines_follow_the_standard(self):
        text = "# Identifier: x\n\n     0\t0x0080\tcomment\n   127\t0x00FF\n"
        self.assertEqual(encoding.parse_index(text), {0: 0x80, 127: 0xFF})

    def test_duplicate_pointers_and_null_code_points_are_rejected(self):
        with self.assertRaises(SystemExit):
            encoding.parse_index("1\t0x41\n1\t0x42\n")
        with self.assertRaises(SystemExit):
            encoding.parse_index("1\t0x0000\n")

    def test_every_encoding_has_a_variant_and_single_byte_indexes_are_named(self):
        self.assertEqual(len(encoding.VARIANTS), 40)
        self.assertEqual(len(encoding.SINGLE_BYTE), 28)
        self.assertEqual(encoding.SINGLE_BYTE["ISO-8859-8-I"], encoding.SINGLE_BYTE["ISO-8859-8"])
        self.assertTrue(set(encoding.SINGLE_BYTE) <= set(encoding.VARIANTS))

    def test_committed_outputs_match_the_manifest(self):
        manifest = json.loads(encoding.MANIFEST.read_text())
        self.assertEqual(manifest["encoding_standard"]["revision"], encoding.ENCODING_REVISION)
        for path, digest in manifest["outputs"].items():
            data = (ROOT / path).read_bytes()
            self.assertEqual(hashlib.sha256(data).hexdigest(), digest, path)
        for output, width, length in encoding.MULTI_BYTE.values():
            self.assertEqual((encoding.DATA / output).stat().st_size, width * length, output)


if __name__ == "__main__":
    unittest.main()
