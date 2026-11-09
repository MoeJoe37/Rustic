from pathlib import Path
import struct
import sys
import unittest


sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import generate_test_fonts  # noqa: E402


class GeneratedFontTests(unittest.TestCase):
    def test_committed_fonts_match_the_generator(self):
        for path, data in generate_test_fonts.outputs().items():
            with self.subTest(path=path.name):
                self.assertTrue(path.is_file(), "run tools/generate_test_fonts.py")
                self.assertEqual(path.read_bytes(), data)

    def test_output_is_deterministic(self):
        self.assertEqual(generate_test_fonts.outputs(), generate_test_fonts.outputs())

    def test_whole_font_checksum_matches_head_adjustment(self):
        for face in generate_test_fonts.FACES:
            with self.subTest(face=face.stem):
                font = generate_test_fonts.build_font(face)
                self.assertEqual(generate_test_fonts.checksum(font), 0xB1B0AFBA)

    def test_table_directory_is_sorted_and_in_bounds(self):
        for face in generate_test_fonts.FACES:
            font = generate_test_fonts.build_font(face)
            count = struct.unpack(">H", font[4:6])[0]
            tags = []
            for index in range(count):
                tag, _, offset, length = struct.unpack(">4sIII", font[12 + 16 * index:28 + 16 * index])
                tags.append(tag)
                self.assertLessEqual(offset + length, len(font))
                self.assertEqual(offset % 4, 0)
            self.assertEqual(tags, sorted(tags))


if __name__ == "__main__":
    unittest.main()
