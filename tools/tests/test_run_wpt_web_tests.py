import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch


sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import run_wpt_web_tests as wpt  # noqa: E402


class WebHarnessTests(unittest.TestCase):
    def test_support_scripts_keep_distinct_paths_with_the_same_basename(self):
        with tempfile.TemporaryDirectory() as temp:
            fixtures = Path(temp)
            upstream = fixtures / "upstream"
            case = "wasm/memory/check.any.js"
            support = ["wasm/assertions.js", "wasm/memory/assertions.js"]
            files = {
                wpt.HARNESS: b"harness",
                case: b"// META: script=/wasm/assertions.js\n// META: script=assertions.js\n",
                support[0]: b"common",
                support[1]: b"memory",
            }
            for path, contents in files.items():
                target = upstream / path
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(contents)
            (fixtures / "report.js").write_bytes(b"report")
            manifest = fixtures / "manifest.json"
            manifest.write_text(json.dumps({"cases": [case], "support": support}), encoding="utf-8")

            def inspect(binary, name, directory, helpers, driver):
                page = wpt.window_page(upstream / name, directory, helpers)
                for helper in support:
                    self.assertIn(f'<script src="support/{helper}">'.encode(), page)
                    self.assertEqual((directory / "support" / helper).read_bytes(), files[helper])
                return []

            with (patch.object(wpt, "UPSTREAM", upstream),
                  patch.object(wpt, "FIXTURES", fixtures),
                  patch.object(wpt, "MANIFEST", manifest),
                  patch.object(wpt, "run_case", side_effect=inspect)):
                self.assertEqual(wpt.run(Path("rustic"), [case]), {case: []})
