import hashlib
import importlib.util
import json
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("rustic_html_entities", ROOT / "tools/import_html_entities.py")
entities = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(entities)


class EntityTableTests(unittest.TestCase):
    def test_generation_preserves_legacy_names_scalars_and_digests(self):
        data = {
            "&amp;": {"codepoints": [38], "characters": "&"},
            "&amp": {"codepoints": [38], "characters": "&"},
            "&test;": {"codepoints": [0x1F642, 0x338], "characters": "🙂\u0338"},
            "&quot;": {"codepoints": [34], "characters": '"'},
        }
        source = json.dumps(data).encode()
        rust, metadata = entities.generate(source, "2026-11-07")
        manifest = json.loads(metadata)
        self.assertIn(r'("test;", "\u{1f642}\u{338}")', rust)
        self.assertIn(r'("quot;", "\"")', rust)
        self.assertLess(rust.index('("amp",'), rust.index('("amp;",'))
        self.assertEqual((manifest["entries"], manifest["legacy_entries"]), (4, 1))
        self.assertEqual(manifest["source_sha256"], hashlib.sha256(source).hexdigest())
        self.assertEqual(manifest["table_sha256"], hashlib.sha256(rust.encode()).hexdigest())
        self.assertEqual(entities.generate(source, "2026-11-07"), (rust, metadata))

    def test_invalid_names_scalars_duplicate_keys_and_missing_legacy_forms_fail(self):
        for data in [
            {}, {"bad": {"codepoints": [65], "characters": "A"}},
            {"&a;": {"codepoints": [0xD800], "characters": "\ud800"}},
            {"&a;": {"codepoints": [True], "characters": "A"}},
            {"&a;": {"codepoints": [65], "characters": "B"}},
            {"&a": {"codepoints": [65], "characters": "A"}},
        ]:
            with self.assertRaises(ValueError): entities.generate(json.dumps(data).encode(), "2026-11-07")
        with self.assertRaisesRegex(ValueError, "duplicate JSON key"):
            entities.generate(b'{"&a;":{},"&a;":{}}', "2026-11-07")
        with self.assertRaisesRegex(ValueError, "explicit YYYY-MM-DD"):
            entities.generate(b"{}", "latest")


if __name__ == "__main__":
    unittest.main()
