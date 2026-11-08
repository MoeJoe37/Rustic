import importlib.util
from pathlib import Path
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("rustic_idna_data", ROOT / "tools/generate_idna_data.py")
idna = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(idna)


class GenerateIdnaDataTests(unittest.TestCase):
    def test_missing_defaults_apply_before_explicit_values(self):
        text = (
            "# @missing: 0000..10FFFF; Left_To_Right\n"
            "# @missing: 0590..05FF; Right_To_Left\n"
            "0041..0042 ; ON # test\n"
        )
        values = idna.parse_with_missing(text, "L", idna.BIDI_NAMES)
        self.assertEqual(values[0x40], "L")
        self.assertEqual(values[0x41], "ON")
        self.assertEqual(values[0x5FF], "R")
        self.assertEqual(values[0x600], "L")

    def test_ranges_collapse_runs_and_filter(self):
        values = [0] * (idna.MAX_CODE_POINT + 1)
        values[5:8] = [3, 3, 4]
        runs = idna.ranges(values, lambda value: value != 0)
        self.assertEqual(runs, [(5, 6, 3), (7, 7, 4)])

    def test_pool_reuses_existing_text_by_byte_offset(self):
        pool = idna.Pool()
        self.assertEqual(pool.add("éa"), (0, 3))
        self.assertEqual(pool.add("a"), (2, 1))
        self.assertEqual(pool.add("b"), (3, 1))
        self.assertEqual(pool.text, "éab")

    def test_literal_escapes_controls_and_format_characters(self):
        category = {0x200E: "Cf", 0xE9: "Ll"}
        self.assertEqual(idna.literal('a"\\\n\t‎é\x7f', category),
                         '"a\\"\\\\\\n\\t\\u{200e}é\\u{7f}"')

    def test_idna_status_overlap_and_gaps_are_rejected(self):
        with self.assertRaisesRegex(ValueError, "overlapping"):
            idna.parse_idna_mapping("0000..10FFFF ; valid\n0041 ; valid\n")
        with self.assertRaisesRegex(ValueError, "does not cover"):
            idna.parse_idna_mapping("0000..0040 ; valid\n")
        with self.assertRaisesRegex(ValueError, "unknown IDNA status"):
            idna.parse_idna_mapping("0000..10FFFF ; disallowed_STD3_valid\n")

    def test_test_file_statuses_ignore_url_flags(self):
        self.assertEqual(idna.statuses("[V3, B1]") - idna.IGNORED_STATUSES, {"B1"})
        self.assertEqual(idna.statuses("[]"), set())
        self.assertEqual(idna.unescape_test_field("a\\u0308\\x{1F600}"), "ä\U0001F600")
        self.assertEqual(idna.unescape_test_field('""'), "")

    def test_missing_inputs_and_digest_mismatches_fail(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(ValueError, "missing input"):
                idna.load_sources(Path(directory), {})
            for name in idna.SOURCES:
                (Path(directory) / name).write_bytes(b"x")
            with self.assertRaisesRegex(ValueError, "does not match pinned"):
                idna.load_sources(Path(directory), {"UnicodeData.txt": "0" * 64})

    def test_committed_manifest_pins_every_source(self):
        pins = idna.pinned_hashes()
        self.assertEqual(set(pins), set(idna.SOURCES))
        self.assertTrue(all(len(value) == 64 for value in pins.values()))


if __name__ == "__main__":
    unittest.main()
