import io
import struct
import sys
import unittest
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import cldr_data  # noqa: E402

SUPPLEMENTAL = """<supplementalData>
  <parentLocales><parentLocale parent="en_001" locales="en_GB"/></parentLocales>
  <parentLocales component="plurals"><parentLocale parent="xx" locales="en_GB"/></parentLocales>
</supplementalData>"""

ROOT = """<ldml><numbers>
  <symbols numberSystem="latn"><decimal>.</decimal><group>,</group></symbols>
  <symbols numberSystem="arab"><alias source="locale" path="../symbols[@numberSystem='latn']"/></symbols>
</numbers></ldml>"""

EN = """<ldml><numbers>
  <symbols numberSystem="latn"><group draft="unconfirmed">_</group><decimal>↑↑↑</decimal></symbols>
</numbers></ldml>"""

EN_001 = """<ldml><numbers>
  <symbols numberSystem="latn"><group>'</group></symbols>
</numbers></ldml>"""


def archive():
    raw = io.BytesIO()
    with zipfile.ZipFile(raw, "w") as output:
        output.writestr("common/supplemental/supplementalData.xml", SUPPLEMENTAL)
        output.writestr("common/main/root.xml", ROOT)
        output.writestr("common/main/en.xml", EN)
        output.writestr("common/main/en_001.xml", EN_001)
        output.writestr("common/main/en_GB.xml", "<ldml/>")
    return cldr_data.Cldr(raw.getvalue())


class CldrTests(unittest.TestCase):
    def test_parents_follow_the_supplemental_table_and_then_truncation(self):
        cldr = archive()
        self.assertEqual(cldr.chain("en-GB"), ["en-GB", "en-001", "en", "root"])
        self.assertEqual(cldr.locales, ["en", "en-001", "en-GB", "root"])

    def test_values_inherit_and_skip_unconfirmed_and_inherited_markers(self):
        cldr = archive()
        latn = "symbols[@numberSystem='latn']"
        self.assertEqual(cldr.get("en", "numbers", latn, "group"), ",")
        self.assertEqual(cldr.get("en", "numbers", latn, "decimal"), ".")
        self.assertEqual(cldr.get("en-GB", "numbers", latn, "group"), "'")

    def test_root_aliases_restart_at_the_requested_locale(self):
        cldr = archive()
        arab = "symbols[@numberSystem='arab']"
        self.assertEqual(cldr.get("en-GB", "numbers", arab, "group"), "'")
        self.assertEqual(cldr.get("root", "numbers", arab, "group"), ",")


class PackTests(unittest.TestCase):
    def test_layout_is_a_string_pool_and_tables_of_cells(self):
        pack = cldr_data.Pack()
        pack.table("t", 2, [[pack.s("é"), 7], [pack.s(None), 9]])
        raw = pack.bytes()
        self.assertEqual(raw[:4], b"RJP1")
        strings, tables = struct.unpack_from("<II", raw, 4)
        self.assertEqual((strings, tables), (3, 1))
        offsets = struct.unpack_from("<4I", raw, 12)
        self.assertEqual(offsets, (0, 0, 2, 3))
        pool = 12 + 16
        self.assertEqual(raw[pool:pool + 3], "ét".encode())
        directory = pool + 4
        self.assertEqual(struct.unpack_from("<IIII", raw, directory), (2, 2, 2, 0))
        cells = struct.unpack_from("<4I", raw, directory + 16)
        self.assertEqual(cells, (1, 7, cldr_data.NONE, 9))

    def test_rows_must_have_the_declared_width(self):
        with self.assertRaises(AssertionError):
            cldr_data.Pack().table("t", 2, [[1]])


if __name__ == "__main__":
    unittest.main()
