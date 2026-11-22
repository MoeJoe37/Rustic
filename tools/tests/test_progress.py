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
                feature.pop("steps", None)
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
        self.assertEqual(data["percent"], "0.12")  # Size 2 of 16 foundation points, weighted at 1%, truncated.
        self.assertEqual(data["verified_percent"], "0.12")
        self.assertEqual(data["partial_percent"], "0.00")
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

    def test_external_dependencies_are_described_and_reported(self):
        feature = self.catalog["groups"][0]["features"][0]
        feature["external"] = "Needs <outside> reviewers"
        progress.validate(self.catalog, self.root)
        data = progress.measure(self.catalog, "0" * 64)
        self.assertIn("**external dependency:** Needs <outside> reviewers", progress.summary(data))
        self.assertIn("external dependency: Needs &lt;outside&gt; reviewers", progress.render_svg(data))
        for value in ["", 7]:
            feature["external"] = value
            with self.assertRaisesRegex(ValueError, "external dependency"):
                progress.validate(self.catalog, self.root)
        feature["external"] = "Needs reviewers"
        self.complete(feature)
        with self.assertRaisesRegex(ValueError, "external dependency"):
            progress.validate(self.catalog, self.root)

    def test_mobile_device_work_is_the_last_milestone_and_cannot_be_dropped(self):
        ids = {feature["id"]: feature for group in self.catalog["groups"] for feature in group["features"]}
        last = self.catalog["milestones"][-1]["id"]
        for identifier in ["platform.android", "platform.ios", "security.mobile", "release.mobile"]:
            self.assertEqual(ids[identifier]["milestone"], last)
        self.assertTrue({"platform.android", "platform.ios", "release.mobile"} <= progress.REQUIRED)

    def test_catalog_cannot_drop_major_systems_gates_or_duplicate_ids(self):
        for edit in [
            lambda catalog: catalog["groups"].pop(),
            lambda catalog: catalog["groups"][0].update(weight=99),
            lambda catalog: catalog["groups"][0]["features"].append(copy.deepcopy(catalog["groups"][0]["features"][0])),
            lambda catalog: catalog["groups"][-1]["features"].pop(),
            lambda catalog: catalog["groups"].remove(next(group for group in catalog["groups"] if group["id"] == "extensions")),
            lambda catalog: next(group for group in catalog["groups"] if group["id"] == "wasm")["features"].pop(),
            lambda catalog: next(group for group in catalog["groups"] if group["id"] == "filtering")["features"].pop(),
            lambda catalog: next(group for group in catalog["groups"] if group["id"] == "perf")["features"].pop(),
            lambda catalog: catalog["groups"].remove(next(group for group in catalog["groups"] if group["id"] == "media")),
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
        self.assertIn("25 subsystems", svg)
        height = float(tree.attrib["height"])
        ns = {"svg": "http://www.w3.org/2000/svg"}
        self.assertTrue(all(float(node.attrib["y"]) < height for node in tree.findall(".//svg:text", ns)))
        self.assertTrue(all(float(node.attrib["x"]) + float(node.attrib["width"]) <= 976.01
                            for node in tree.findall(".//svg:rect", ns) if node.attrib.get("width", "").replace(".", "").isdigit() and float(node.attrib["width"]) < 1000))

    def test_worktree_metadata_does_not_change_the_source_fingerprint(self):
        before = progress.source_fingerprint(self.root)
        (self.root / ".git").write_text("gitdir: /repo/.git/worktrees/feature\n")
        self.assertEqual(progress.source_fingerprint(self.root), before)

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

    def test_capability_size_weights_progress_within_subsystem(self):
        group = next(group for group in self.catalog["groups"] if group["id"] == "css")
        properties = next(feature for feature in group["features"] if feature["id"] == "css.properties")
        syntax = next(feature for feature in group["features"] if feature["id"] == "css.syntax")
        self.complete(properties)
        large = progress.measure(self.catalog, "0" * 64)
        properties["status"], properties["evidence"] = "planned", []
        self.complete(syntax)
        small = progress.measure(self.catalog, "0" * 64)
        self.assertEqual(large["percent"], "0.97")  # 8 of 49 points of a 6% subsystem, truncated.
        self.assertEqual(small["percent"], "0.36")  # 3 of 49 points.

    def in_progress(self, feature, done, total):
        feature["status"] = "in_progress"
        feature["steps"] = [{"label": f"step {index}", "status": "planned", "evidence": []} for index in range(total)]
        for step in feature["steps"][:done]:
            self.complete(step)

    def test_partial_credit_counts_only_verified_steps_and_is_reported_separately(self):
        feature = self.catalog["groups"][0]["features"][0]
        self.in_progress(feature, 1, 4)
        progress.validate(self.catalog, self.root)
        data = progress.measure(self.catalog, "0" * 64)
        self.assertEqual(data["percent"], "0.03")  # 1% × 2/16 × 1/4, truncated.
        self.assertEqual(data["verified_percent"], "0.00")
        self.assertEqual(data["partial_percent"], "0.03")
        self.assertEqual(data["in_progress"], 1)
        self.assertEqual(data["groups"][0]["features"][0]["credit"], "25.00")
        feature["status"] = "in_progress"
        feature.pop("steps")
        self.assertEqual(progress.measure(self.catalog, "0" * 64)["percent"], "0.00")

    def test_step_rules_are_enforced(self):
        feature = self.catalog["groups"][0]["features"][0]
        for setup, message in [
            (lambda: (self.in_progress(feature, 1, 2), feature["steps"][0].update(evidence=[])), "step needs source and test"),
            (lambda: self.in_progress(feature, 2, 2), "mark the capability complete"),
            (lambda: self.in_progress(feature, 0, 1), "two or more steps"),
            (lambda: (self.in_progress(feature, 1, 3), feature.update(status="planned")), "two or more steps"),
            (lambda: (self.in_progress(feature, 1, 3), feature["steps"][0]["evidence"][1].update(marker="gone")), "missing evidence marker"),
        ]:
            catalog_feature = copy.deepcopy(feature)
            setup()
            with self.assertRaisesRegex(ValueError, message):
                progress.validate(self.catalog, self.root)
            feature.clear()
            feature.update(catalog_feature)

    def test_sizes_and_milestones_are_validated_and_milestones_sum_to_engine(self):
        for edit, message in [(lambda feature: feature.update(size=4), "size"), (lambda feature: feature.update(milestone="M99"), "milestone")]:
            catalog = copy.deepcopy(self.catalog)
            edit(catalog["groups"][0]["features"][0])
            with self.assertRaisesRegex(ValueError, message):
                progress.validate(catalog, self.root)
        for group in self.catalog["groups"]:
            for feature in group["features"]:
                self.complete(feature)
        data = progress.measure(self.catalog, "0" * 64)
        self.assertTrue(all(milestone["percent"] == "100.00" for milestone in data["milestones"]))
        total = sum(Fraction(milestone["share"]) for milestone in data["milestones"])
        self.assertLessEqual(abs(total - 100), Fraction(len(data["milestones"]), 100))


if __name__ == "__main__":
    unittest.main()
