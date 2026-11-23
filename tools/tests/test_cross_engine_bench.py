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


class DeviceReportTest(unittest.TestCase):
    def report(self, label, tiny_ms):
        page = {"page_tiny.total": bench.summarize([(tiny_ms, "11")] * 3)}
        return {
            "device": {"label": label, "system": "Linux 6", "machine": "x86_64", "cpu": "cpu",
                       "logical_cores": 4, "memory": "16.0 GiB", "python": "3"},
            "commit": "abc1234",
            "runs": 3,
            "size": 10,
            "dom_size": 10,
            "js": {
                "Rustic": {"calls": bench.summarize([(4.0, "7")] * 3)},
                "V8": {"calls": bench.summarize([(2.0, "7")] * 3)},
            },
            "page": page,
            "budgets": bench.check_budgets(page, {"page_tiny.total": 16.7, "page_deep.total": 50.0}),
        }

    def test_budgets_pass_only_when_measured_within_the_limit(self):
        rows = self.report("A", 3.0)["budgets"]
        self.assertEqual([row["within"] for row in rows], [True, False])
        self.assertIsNone(rows[1]["median_ms"])
        self.assertFalse(self.report("A", 20.0)["budgets"][0]["within"])

    def test_merged_report_lists_every_device_with_its_ratio(self):
        text = bench.merge_reports([self.report("Linux x64", 3.0), self.report("macOS ARM64", 30.0)])
        self.assertIn("| Linux x64 | 1/2 | not measured | 2.0 |", text)
        self.assertIn("| macOS ARM64 | 0/2 | not measured | 2.0 |", text)
        self.assertIn("### macOS ARM64", text)
        self.assertIn("| `calls` | 4.00 [4.00, 4.00] | 2.00 [2.00, 2.00] | 2.0 | yes |", text)

    def test_device_info_names_the_machine(self):
        info = bench.device_info("Here")
        self.assertEqual(info["label"], "Here")
        self.assertGreater(info["logical_cores"], 0)
        self.assertTrue(info["machine"])

    def test_budget_table_matches_the_page_fixtures(self):
        for key in bench.M8_PAGE_BUDGETS_MS:
            fixture, _, stage = key.partition(".")
            self.assertIn(fixture, bench.PAGE_FIXTURES)
            self.assertIn(stage, bench.PAGE_STAGES)


if __name__ == "__main__":
    unittest.main()


class GpuBudgetTest(unittest.TestCase):
    def gpu(self, software, gpu_ms, cpu_ms):
        results = {}
        for scene in bench.GPU_SCENES:
            results[f"{scene}.gpu"] = bench.summarize([(gpu_ms, "5")] * 3)
            results[f"{scene}.cpu"] = bench.summarize([(cpu_ms, "5")] * 3)
        adapter = {"backend": "vulkan", "software": software, "name": "test"}
        return {"adapter": adapter, "results": results}

    def test_software_adapters_get_the_regression_budget_and_hardware_one_frame(self):
        rows = bench.check_gpu_budgets(self.gpu(True, 30.0, 40.0))
        self.assertEqual({row["metric"]: row["within"] for row in rows}["frames_720p.gpu"], True)
        rows = bench.check_gpu_budgets(self.gpu(False, 30.0, 40.0))
        self.assertEqual({row["metric"]: row["within"] for row in rows}["frames_720p.gpu"], False)

    def test_a_gpu_slower_than_twice_the_cpu_reference_misses(self):
        rows = bench.check_gpu_budgets(self.gpu(True, 10.0, 4.0))
        ratio = {row["metric"]: row for row in rows}["frames_720p.gpu ÷ cpu"]
        self.assertAlmostEqual(ratio["median_ms"], 2.5)
        self.assertFalse(ratio["within"])

    def test_no_adapter_misses_every_gpu_row(self):
        results = {f"{scene}.{kind}": {"status": "not runnable: no adapter"}
                   for scene in bench.GPU_SCENES for kind in ("gpu", "cpu")}
        rows = bench.check_gpu_budgets({"adapter": None, "results": results})
        self.assertTrue(rows and not any(row["within"] for row in rows))

    def test_the_device_report_lists_the_adapter_and_gpu_budgets(self):
        gpu = self.gpu(True, 5.0, 8.0)
        gpu["budgets"] = bench.check_gpu_budgets(gpu)
        report = {"device": {"label": "Linux x64", "system": "Linux", "machine": "x86_64", "cpu": "cpu",
                             "logical_cores": 4, "memory": "16 GiB"}, "runs": 3, "gpu": gpu}
        text = "\n".join(bench.render_device(report))
        self.assertIn("test (vulkan, software)", text)
        self.assertIn("| `frames_1440p.gpu ÷ cpu` | 2.0 | 0.62 | yes |", text)
        merged = bench.merge_reports([report])
        self.assertIn("| Linux x64 | 0/0 | 4/4 |", merged)
