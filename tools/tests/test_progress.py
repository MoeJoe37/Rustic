import copy
from fractions import Fraction
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
import xml.etree.ElementTree as ET


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("rustic_progress", ROOT / "tools/progress.py")
progress = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(progress)


class ProgressTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.catalog = json.loads((ROOT / "progress/catalog.json").read_text())
        for group in self.catalog["groups"]:
            for feature in group["features"]:
                feature["status"] = "planned"
                feature["evidence"] = []
        (self.root / "source.rs").write_text("pub fn implementation() {}\n")
        (self.root / "tests.rs").write_text("#[test] fn behavior_is_verified() {}\n")
        (self.root / "README.md").write_text("# Test engine\n\n" + progress.START + "\nold output\n" + progress.END + "\n")

    def complete(self, feature):
        feature["status"] = "complete"
        feature["evidence"] = [
            {"kind": "source", "path": "source.rs", "marker": "pub fn implementation"},
            {"kind": "test", "path": "tests.rs", "marker": "behavior_is_verified"},
        ]

    def test_weights_use_whole_engine_scope_and_never_round_to_100(self):
        self.complete(self.catalog["groups"][0]["features"][0])
        progress.validate(self.catalog, self.root)
        data = progress.measure(self.catalog, "0" * 64)
        self.assertEqual(data["percent"], "0.25")  # One of eight foundation gates, weighted at 2%.
        self.assertFalse(data["fully_complete"])
        self.assertEqual(progress.percent(Fraction(999999, 10000)), "99.99")
        for group in self.catalog["groups"]:
            for feature in group["features"]:
                self.complete(feature)
        data = progress.measure(self.catalog, "0" * 64)
        self.assertEqual(data["percent"], "100.00")
        self.assertTrue(data["fully_complete"])
        self.catalog["groups"][-1]["features"][-1]["status"] = "planned"
        self.assertFalse(progress.measure(self.catalog, "0" * 64)["fully_complete"])

    def test_completed_capabilities_require_existing_source_and_test_markers(self):
        feature = self.catalog["groups"][0]["features"][0]
        feature["status"] = "complete"
        with self.assertRaisesRegex(ValueError, "source and test"):
            progress.validate(self.catalog, self.root)
        self.complete(feature)
        feature["evidence"][1]["marker"] = "missing_test"
        with self.assertRaisesRegex(ValueError, "missing evidence marker"):
            progress.validate(self.catalog, self.root)
        feature["evidence"][1]["path"] = "../outside.rs"
        with self.assertRaisesRegex(ValueError, "invalid evidence path"):
            progress.validate(self.catalog, self.root)

    def test_catalog_cannot_drop_major_systems_gates_or_duplicate_ids(self):
        for edit in [
            lambda catalog: catalog["groups"].pop(),
            lambda catalog: catalog["groups"][0].update(weight=99),
            lambda catalog: catalog["groups"][0]["features"].append(copy.deepcopy(catalog["groups"][0]["features"][0])),
            lambda catalog: catalog["groups"][-1]["features"].pop(),
            lambda catalog: catalog["groups"].remove(next(group for group in catalog["groups"] if group["id"] == "extensions")),
            lambda catalog: next(group for group in catalog["groups"] if group["id"] == "wasm")["features"].pop(),
            lambda catalog: next(group for group in catalog["groups"] if group["id"] == "filtering")["features"].pop(),
        ]:
            catalog = copy.deepcopy(self.catalog)
            edit(catalog)
            with self.assertRaises(ValueError):
                progress.validate(catalog, self.root)

    def test_svg_escapes_labels_and_has_accessible_well_formed_output(self):
        self.catalog["groups"][0]["label"] = '<script>&"'
        svg = progress.render_svg(progress.measure(self.catalog, "0" * 64))
        tree = ET.fromstring(svg)
        self.assertEqual(tree.tag, "{http://www.w3.org/2000/svg}svg")
        self.assertNotIn("<script>", svg)
        self.assertIn("&lt;script&gt;", svg)
        self.assertIn("aria-labelledby", svg)
        self.assertIn("21 subsystems", svg)
        self.assertEqual(tree.attrib["height"], "804")
        ns = {"svg": "http://www.w3.org/2000/svg"}
        self.assertTrue(all(float(node.attrib["y"]) < 804 for node in tree.findall(".//svg:text", ns)))

    def test_generated_files_do_not_change_fingerprint_but_source_changes_do(self):
        before = progress.source_fingerprint(self.root)
        for name in progress.OUTPUTS:
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("generated data")
        (self.root / "README.md").write_text("# Test engine\n\n" + progress.START + "\nnew output\n" + progress.END + "\n")
        self.assertEqual(progress.source_fingerprint(self.root), before)
        (self.root / "source.rs").write_text("pub fn changed_behavior() {}\n")
        self.assertNotEqual(progress.source_fingerprint(self.root), before)


if __name__ == "__main__":
    unittest.main()
