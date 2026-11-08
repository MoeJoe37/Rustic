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

    def test_selection_is_independent_of_parser_results_and_conservatively_closed(self):
        for data, reason in [('<table>', 'outside_element_whitelist'), ('<svg>', 'outside_element_whitelist'), ('<?target?>', 'processing_instruction'), ('<script>"<b>"</script>', 'outside_element_whitelist')]:
            self.assertEqual(importer.exclusion({'#data': data, '#document': '| <html>\n'}), reason)
        self.assertIsNone(importer.exclusion({'#data': '<DIV><span>x</span>', '#document': '| <html>\n|   <div>\n'}))
        self.assertEqual(importer.exclusion({'#data': '', '#document': '| <html>\n', '#document-fragment': 'div'}), 'fragment_context')
        self.assertEqual(importer.exclusion({'#data': '', '#document': '| <html>\n', '#script-on': ''}), 'scripting_enabled')

    def test_malformed_records_are_rejected_instead_of_dropped(self):
        for source in ['', '#data\nx\n#errors\n', '#data\nx\n#errors\n#errors\n#document\n']:
            with self.assertRaises(ValueError):
                importer.records(source)

    def test_checked_in_source_regenerates_with_complete_selection_accounting(self):
        importer.check()
        raw = (importer.FIXTURES / 'tree_tests1.dat').read_bytes()
        rust, manifest = importer.generate(raw)
        metadata = json.loads(manifest)
        self.assertEqual(metadata['upstream_cases'], 112)
        self.assertEqual(metadata['selected_cases'], 45)
        self.assertEqual(metadata['selected_cases'] + metadata['excluded_cases'], metadata['upstream_cases'])
        self.assertEqual(sum(metadata['exclusion_reasons'].values()), metadata['excluded_cases'])
        self.assertEqual(len(metadata['selected_array_indices']), 45)
        self.assertIn('#[rustfmt::skip]', rust)


if __name__ == '__main__':
    unittest.main()
