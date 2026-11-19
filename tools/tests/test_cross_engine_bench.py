"""Statistics and parsing of the PERF-01 cross-engine harness."""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import cross_engine_bench as bench  # noqa: E402


class MedianIntervalTest(unittest.TestCase):
    def test_eleven_samples_use_the_second_and_tenth_order_statistics(self):
        samples = [float(value) for value in [9, 1, 5, 3, 7, 11, 2, 4, 6, 8, 10]]
        self.assertEqual(bench.median_interval(samples), (6.0, 2.0, 10.0))

    def test_small_samples_fall_back_to_the_range(self):
        self.assertEqual(bench.median_interval([3.0, 1.0, 2.0]), (2.0, 1.0, 3.0))

    def test_empty_samples_are_rejected(self):
        with self.assertRaises(ValueError):
            bench.median_interval([])


class ParseTest(unittest.TestCase):
    def test_named_and_unnamed_sample_lines(self):
        text = "size 1\nsample arrays 1.5 42\nsample calls 2 7\nnoise\n"
        self.assertEqual(bench.parse_samples(text, "arrays"), [(1.5, "42")])
        self.assertEqual(bench.parse_samples("sample 3.25 abc def\n"), [(3.25, "abc def")])

    def test_cells_report_unrunnable_engines_as_text(self):
        self.assertEqual(bench.cell("not runnable: crash"), "not runnable: crash")
        self.assertEqual(bench.cell([(1.0, "x"), (2.0, "x"), (3.0, "x")]), "2.00 [1.00, 3.00]")
        self.assertEqual(bench.checksum([(1.0, "x"), (2.0, "y")]), "inconsistent")


if __name__ == "__main__":
    unittest.main()
