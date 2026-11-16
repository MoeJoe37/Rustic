from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import generate_dom_idl as generator  # noqa: E402


class GenerateDomIdlTests(unittest.TestCase):
    def test_committed_metadata_reproduces_offline(self):
        generator.check()

    def test_stale_generated_output_and_modified_input_are_reported(self):
        with tempfile.TemporaryDirectory() as directory:
            changed = Path(directory) / "changed"
            changed.write_bytes(b"// changed\n")
            with patch.object(generator, "OUTPUT", changed):
                with self.assertRaisesRegex(ValueError, "metadata is stale"):
                    generator.check()
            with patch.object(generator, "SUBSET", changed):
                with self.assertRaisesRegex(ValueError, "SHA-256 differs"):
                    generator.generate()

    def test_unsupported_syntax_and_overloads_fail_closed(self):
        for body, message in [
            ("sequence<DOMString> elements();", "unsupported IDL token"),
            ("[Unrecognized] Text splitText(unsigned long offset);", "expected 'NewObject'"),
            ("constructor(optional DOMString data);", "expected '='"),
            ('constructor(optional DOMString data = "", unsigned long offset);',
             "required argument follows"),
            ("undefined f(); undefined f(DOMString data);", "duplicate or overloaded"),
        ]:
            with self.subTest(body=body), self.assertRaisesRegex(ValueError, message):
                generator.parse_subset(f"[Exposed=Window] interface Text : Node {{ {body} }};",
                                       ["Text"])

    def test_cross_interface_validation_rejects_unknown_parents_and_cycles(self):
        for parent, message in [("Missing", "unsupported parent"), ("Text", "inheritance cycle")]:
            with self.subTest(parent=parent), self.assertRaisesRegex(ValueError, message):
                generator.parse_subset(f"[Exposed=Window] interface Text : {parent} {{}};",
                                       ["Text"])

    def test_extract_checks_that_each_interface_has_one_upstream_block(self):
        text = "<pre class=idl>\n[Exposed=Window]\ninterface Text : Node {};\n</pre>"
        self.assertEqual(
            generator.extract_subset(text, ["Text"]),
            b"[Exposed=Window]\ninterface Text : Node {};\n",
        )
        with self.assertRaisesRegex(ValueError, "found 2"):
            generator.extract_subset(text * 2, ["Text"])
