import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import import_html_tree_tests as importer


class TreeImporterTests(unittest.TestCase):
    def test_field_delimiters_preserve_input_newlines_and_error_variants(self):
        cases = importer.records('#data\n\nhello\n\n#errors\nold\n#new-errors\nnew\n#script-off\n#document\n| <html>\n\n')
        self.assertEqual(cases[0]['#data'], '\nhello\n')
        self.assertEqual(cases[0]['#document'], '| <html>\n')
        self.assertIn('#script-off', cases[0])

    def test_malformed_records_are_rejected_instead_of_dropped(self):
        for source in ['', '#data\nx\n#errors\n', '#data\nx\n#errors\n#errors\n#document\n']:
            with self.assertRaises(ValueError):
                importer.records(source)

    def test_vendored_corpus_matches_manifest_with_complete_accounting(self):
        importer.check()
        metadata = json.loads(importer.MANIFEST.read_text())
        self.assertEqual(metadata['upstream_revision'], importer.REVISION)
        self.assertEqual(len(metadata['files']), 57)
        for entry in metadata['files']:
            self.assertEqual(entry['runnable_cases'], entry['upstream_cases'])
        self.assertEqual(metadata['runnable_cases'], sum(f['runnable_cases'] for f in metadata['files']))
        self.assertEqual(metadata['runnable_cases'], 1792)


if __name__ == '__main__':
    unittest.main()
