import copy
import hashlib
import importlib.util
import json
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("rustic_html_import", ROOT / "tools/import_html_tests.py")
importer = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(importer)


class HtmlImportTests(unittest.TestCase):
    def test_selection_excludes_unsupported_scope_without_consulting_results(self):
        base = {"input": "<!--x-->", "output": [["Comment", "x"]]}
        self.assertIsNone(importer.exclusion(base))
        for updates, reason in [
            ({"initialStates": ["Script data state"]}, "non_default_state_or_context"),
            ({"lastStartTag": "script"}, "non_default_state_or_context"),
            ({"doubleEscaped": True}, "double_escaped"),
            ({"input": "<!--\ud800-->"}, "surrogate_input_or_output"),
            ({"output": [["Comment", "\udfff"]]}, "surrogate_input_or_output"),
            ({"input": "plain"}, "outside_declaration_entry_points"),
            ({"input": "<!--&amp;-->"}, "conservative_ampersand_exclusion"),
            ({"output": [["StartTag", "a", {}]]}, "outside_declaration_token_types"),
            ({"errors": [{"code": "control-character-in-input-stream"}]}, "outside_declared_error_codes"),
        ]:
            case = copy.deepcopy(base)
            case.update(updates)
            self.assertEqual(importer.exclusion(case), reason)

    def test_generated_fixture_preserves_optional_fields_flags_and_provenance(self):
        cases = [
            {"description": "missing", "input": "<!DOCTYPE>", "output": [["DOCTYPE", None, None, None, False]], "errors": [{"code": "missing-doctype-name"}]},
            {"description": "empty", "input": "<!DOCTYPE html PUBLIC ''>", "output": [["DOCTYPE", "html", "", None, True]]},
            {"description": "instruction", "input": "<?Task é?>", "output": [["ProcessingInstruction", "Task", "é"]]},
            {"description": "excluded", "input": "&amp;", "output": [["Character", "&"]]},
        ]
        sources = {path: json.dumps({"tests": cases if index == 0 else []}).encode() for index, path in enumerate(importer.FILES)}
        rust, metadata = importer.generate(sources, "a" * 40)
        self.assertIn("Expected::Doctype(None, None, None, true)", rust)
        self.assertIn('Expected::Doctype(Some("html"), Some(""), None, false)', rust)
        self.assertIn(r'Expected::Instruction("Task", "\u{e9}")', rust)
        self.assertIn("D::MissingDoctypeName", rust)
        manifest = json.loads(metadata)
        self.assertEqual((manifest["upstream_cases"], manifest["selected_cases"], manifest["excluded_cases"]), (4, 3, 1))
        self.assertEqual(manifest["fixture_sha256"], hashlib.sha256(rust.encode()).hexdigest())
        self.assertEqual(manifest["sources"][0]["selected_array_indices"], [0, 1, 2])
        self.assertEqual(manifest["sources"][0]["source_sha256"], hashlib.sha256(sources[importer.FILES[0]]).hexdigest())
        self.assertEqual(importer.generate(sources, "a" * 40), (rust, metadata))

    def test_rust_literals_escape_controls_and_revision_is_immutable(self):
        self.assertEqual(importer.literal("\n"), '"\\u{a}"')
        self.assertEqual(importer.literal('"'), '"\\\""')
        self.assertEqual(importer.literal("\\"), '"\\\\"')
        self.assertEqual(importer.literal("🙂"), '"\\u{1f642}"')
        with self.assertRaisesRegex(ValueError, "full upstream commit SHA"):
            importer.generate({}, "main")


if __name__ == "__main__":
    unittest.main()
