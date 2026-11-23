#!/usr/bin/env python3
"""PERF-01 cross-engine harness: run Rustic's JS-09 corpus in Rustic and in
JavaScript engine shells, and the DOM and page pipeline benchmarks in Rustic,
then write a
Markdown report with exact versions, medians and 95% confidence intervals.

Each shell sample is a fresh process that loads the workload, calls
`run(size)` once and times it with the engine's own clock, as `js_bench`
does with a fresh agent per run. Startup is not measured. A workload or
engine that cannot run is reported as "not runnable", never as a number.

    python3 tools/cross_engine_bench.py \\
        --rustic target/release/examples/js_bench \\
        --dom target/release/examples/dom_bench \\
        --engine "V8 (Deno 2.9.8, V8 15.0.245.2)=deno run --quiet {file}" \\
        --engine "SpiderMonkey 160.0.1=/path/js {file}" \\
        --output report.md

The reference-device workflow (.github/workflows/perf.yml) adds `--page`
(the page pipeline fixtures of `page_bench`), `--device` and `--json` for a
per-device report, and then merges every device's JSON with `--merge`.
"""

import argparse
import json
import math
import os
from pathlib import Path
import platform
import shlex
import statistics
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
CORPUS = ROOT / "crates/rustic-js/benchmarks"
NOT_RUNNABLE = "not runnable"

DRIVER = """
const __now = (typeof performance === "object" && performance && typeof performance.now === "function")
  ? () => performance.now()
  : (typeof preciseTime === "function" ? () => preciseTime() * 1000 : () => Date.now());
const __out = typeof print === "function" ? print : (value) => console.log(value);
const __start = __now();
const __result = run(%d);
const __elapsed = __now() - __start;
__out("sample " + __elapsed + " " + __result);
"""


def median_interval(samples: list[float], confidence: float = 0.95) -> tuple[float, float, float]:
    """The median and a distribution-free confidence interval for it.

    The interval is the pair of order statistics whose ranks a binomial
    (n, 1/2) distribution puts outside with total probability at most
    1 - confidence. With fewer than six samples no such pair exists, and
    the interval is the sample range.
    """
    if not samples:
        raise ValueError("no samples")
    ordered = sorted(samples)
    n = len(ordered)
    middle = statistics.median(ordered)
    tail = (1 - confidence) / 2
    # Largest k with P(B <= k - 1) <= tail, where B ~ Binomial(n, 1/2).
    cumulative = 0.0
    k = 0
    for j in range(n):
        probability = math.comb(n, j) / 2**n
        if cumulative + probability > tail:
            break
        cumulative += probability
        k = j + 1
    if k == 0:
        return middle, ordered[0], ordered[-1]
    return middle, ordered[k - 1], ordered[n - k]


def parse_samples(text: str, workload: str | None = None) -> list[tuple[float, str]]:
    """`sample [workload] <ms> <result>` lines, optionally for one workload."""
    out = []
    for line in text.splitlines():
        parts = line.split()
        if not parts or parts[0] != "sample":
            continue
        if workload is not None:
            if len(parts) < 4 or parts[1] != workload:
                continue
            out.append((float(parts[2]), " ".join(parts[3:])))
        else:
            if len(parts) < 3:
                continue
            out.append((float(parts[1]), " ".join(parts[2:])))
    return out


def run_shell(command: str, workload: Path, size: int, runs: int) -> list[tuple[float, str]] | str:
    source = workload.read_text(encoding="utf-8") + DRIVER % size
    samples = []
    with tempfile.TemporaryDirectory(prefix="rustic-perf-") as temp:
        script = Path(temp) / f"{workload.stem}.js"
        script.write_text(source, encoding="utf-8")
        argv = [part.replace("{file}", str(script)) for part in shlex.split(command)]
        for _ in range(runs):
            try:
                result = subprocess.run(argv, capture_output=True, encoding="utf-8", timeout=300, check=False)
            except (OSError, subprocess.TimeoutExpired) as error:
                return f"{NOT_RUNNABLE}: {error}"
            parsed = parse_samples(result.stdout)
            if result.returncode or len(parsed) != 1:
                message = (result.stderr or result.stdout).strip().splitlines()
                return f"{NOT_RUNNABLE}: {message[-1] if message else 'no sample'}"
            samples.append(parsed[0])
    return samples


def run_rustic(binary: Path, workloads: list[str], size: int, runs: int, extra: list[str]) -> dict:
    command = [str(binary), "--size", str(size), "--runs", str(runs), *extra]
    result = subprocess.run(command, capture_output=True, encoding="utf-8", timeout=3600, check=False)
    found = {}
    for name in workloads:
        samples = parse_samples(result.stdout, name)
        found[name] = samples if len(samples) == runs else f"{NOT_RUNNABLE}: {result.stderr.strip() or 'missing samples'}"
    return found


def cell(samples) -> str:
    if isinstance(samples, str):
        return samples
    middle, low, high = median_interval([ms for ms, _ in samples])
    return f"{middle:.2f} [{low:.2f}, {high:.2f}]"


def checksum(samples) -> str | None:
    if isinstance(samples, str):
        return None
    results = {result for _, result in samples}
    return results.pop() if len(results) == 1 else "inconsistent"


DOM_WORKLOADS = ["dom_create_append", "dom_query", "dom_attributes", "dom_dispatch"]
PAGE_FIXTURES = ["page_tiny", "page_article", "page_large_dom", "page_selectors", "page_deep"]
PAGE_STAGES = ["parse", "style", "layout", "paint", "raster", "total"]

# M8 desktop-alpha page budgets (docs/PERFORMANCE.md, "M8 budgets"): the
# median end-to-end time of one fixture, parse to raster, in milliseconds,
# on every reference device. Keep this table and the document in step.
M8_PAGE_BUDGETS_MS = {
    "page_tiny.total": 16.7,
    "page_article.total": 250.0,
    "page_large_dom.total": 600.0,
    "page_selectors.total": 300.0,
    "page_deep.total": 50.0,
}


# compositor.parity frame-time gates (docs/PERFORMANCE.md, "GPU frame
# budgets"): the median time of one composited frame of `gpu_frame_bench`,
# upload to readback, in milliseconds. A hardware adapter gets one 60 Hz
# frame; a software adapter (lavapipe, WARP) is CI's regression gate, with
# room for shared runners. The GPU must also stay within GPU_CPU_RATIO of the
# CPU reference on the same device, which does not depend on its speed.
GPU_SCENES = ["frames_720p", "frames_1440p"]
GPU_HARDWARE_BUDGETS_MS = {"frames_720p.gpu": 16.7, "frames_1440p.gpu": 16.7}
GPU_SOFTWARE_BUDGETS_MS = {"frames_720p.gpu": 33.4, "frames_1440p.gpu": 200.0}
GPU_CPU_RATIO = 2.0


def run_gpu(binary: Path, runs: int) -> dict:
    """`gpu_frame_bench` results: the adapter and each scene's samples."""
    keys = [f"{scene}.{kind}" for scene in GPU_SCENES for kind in ("gpu", "cpu")]
    try:
        result = subprocess.run([str(binary), "--runs", str(runs)], capture_output=True,
                                encoding="utf-8", timeout=3600, check=False)
    except (OSError, subprocess.TimeoutExpired) as error:
        return {"adapter": None, "results": {key: {"status": f"{NOT_RUNNABLE}: {error}"} for key in keys}}
    adapter = None
    for line in result.stdout.splitlines():
        parts = line.split(maxsplit=3)
        if parts[:1] == ["adapter"] and len(parts) == 4:
            adapter = {"backend": parts[1], "software": parts[2] == "software", "name": parts[3]}
    reason = result.stderr.strip().splitlines()
    results = {}
    for key in keys:
        samples = parse_samples(result.stdout, key)
        if len(samples) == runs:
            results[key] = summarize(samples)
        else:
            why = next((line for line in reason if line.startswith(key)), reason[-1] if reason else "missing samples")
            results[key] = {"status": f"{NOT_RUNNABLE}: {why}"}
    return {"adapter": adapter, "results": results}


def check_gpu_budgets(gpu: dict) -> list[dict]:
    """Frame-time rows for one device: the adapter kind's budget and the
    ratio to the CPU reference. Without an adapter every row misses."""
    adapter = gpu.get("adapter")
    budgets = GPU_SOFTWARE_BUDGETS_MS if adapter is None or adapter["software"] else GPU_HARDWARE_BUDGETS_MS
    results = gpu.get("results", {})
    rows = check_budgets(results, budgets)
    for scene in GPU_SCENES:
        gpu_ms = results.get(f"{scene}.gpu", {}).get("median_ms")
        cpu_ms = results.get(f"{scene}.cpu", {}).get("median_ms")
        ratio = gpu_ms / cpu_ms if gpu_ms is not None and cpu_ms else None
        rows.append({"metric": f"{scene}.gpu ÷ cpu", "budget_ms": GPU_CPU_RATIO, "median_ms": ratio,
                     "within": ratio is not None and ratio <= GPU_CPU_RATIO})
    return rows


def _run_text(argv: list[str]) -> str:
    try:
        result = subprocess.run(argv, capture_output=True, encoding="utf-8", timeout=60, check=False)
    except (OSError, subprocess.TimeoutExpired):
        return ""
    return result.stdout.strip() if result.returncode == 0 else ""


def device_info(label: str) -> dict:
    """What the report needs to name the machine: OS, architecture, CPU,
    logical cores and memory, from the platform's own tools when present."""
    system = platform.system()
    cpu = ""
    memory = ""
    if system == "Linux":
        try:
            for line in Path("/proc/cpuinfo").read_text(encoding="utf-8").splitlines():
                key, _, value = line.partition(":")
                if key.strip() in ("model name", "Model", "Hardware") and value.strip():
                    cpu = value.strip()
                    break
            if not cpu:
                for line in _run_text(["lscpu"]).splitlines():
                    if line.startswith("Model name:"):
                        cpu = line.partition(":")[2].strip()
                        break
            for line in Path("/proc/meminfo").read_text(encoding="utf-8").splitlines():
                if line.startswith("MemTotal:"):
                    memory = f"{int(line.split()[1]) / 1024 / 1024:.1f} GiB"
                    break
        except OSError:
            pass
    elif system == "Darwin":
        cpu = _run_text(["sysctl", "-n", "machdep.cpu.brand_string"])
        bytes_ = _run_text(["sysctl", "-n", "hw.memsize"])
        memory = f"{int(bytes_) / 2**30:.1f} GiB" if bytes_.isdigit() else ""
    elif system == "Windows":
        cpu = _run_text(["powershell", "-NoProfile", "-Command", "(Get-CimInstance Win32_Processor | Select-Object -First 1).Name"])
        bytes_ = _run_text(["powershell", "-NoProfile", "-Command", "(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory"])
        memory = f"{int(bytes_) / 2**30:.1f} GiB" if bytes_.isdigit() else ""
    return {
        "label": label,
        "system": f"{system} {platform.release()}",
        "machine": platform.machine(),
        "cpu": cpu or platform.processor() or "unknown",
        "logical_cores": os.cpu_count() or 0,
        "memory": memory or "unknown",
        "python": platform.python_version(),
    }


def summarize(samples) -> dict:
    """JSON form of one cell: the median and interval, or the reason it did not run."""
    if isinstance(samples, str):
        return {"status": samples}
    middle, low, high = median_interval([ms for ms, _ in samples])
    return {"median_ms": middle, "low_ms": low, "high_ms": high, "runs": len(samples), "check": checksum(samples)}


def summary_cell(entry: dict) -> str:
    if "median_ms" not in entry:
        return entry["status"]
    return f"{entry['median_ms']:.2f} [{entry['low_ms']:.2f}, {entry['high_ms']:.2f}]"


def check_budgets(page: dict, budgets: dict[str, float]) -> list[dict]:
    """Budget rows for the page results; a fixture that did not run misses."""
    rows = []
    for key, limit in budgets.items():
        entry = page.get(key, {"status": f"{NOT_RUNNABLE}: not measured"})
        median = entry.get("median_ms")
        rows.append({"metric": key, "budget_ms": limit, "median_ms": median, "within": median is not None and median <= limit})
    return rows


def render_device(report: dict) -> list[str]:
    device = report["device"]
    lines = [
        f"### {device['label']}",
        "",
        f"{device['system']}, {device['machine']}, {device['cpu']}, {device['logical_cores']} logical cores, "
        f"{device['memory']}. Rustic `{report.get('commit', 'unknown')}`; {report['runs']} runs per cell.",
        "",
    ]
    js = report.get("js", {})
    if js:
        labels = list(js)
        workloads = sorted({name for results in js.values() for name in results})
        lines += [
            f"JavaScript corpus (size {report['size']}). Cells are median ms [95% CI]; the ratio is "
            "Rustic ÷ fastest other engine.",
            "",
            "| Workload | " + " | ".join(labels) + " | Rustic ÷ best | Results agree |",
            "| --- | " + " | ".join("---" for _ in labels) + " | --- | --- |",
        ]
        for name in workloads:
            entries = {label: js[label].get(name, {"status": NOT_RUNNABLE}) for label in labels}
            medians = {label: e["median_ms"] for label, e in entries.items() if "median_ms" in e}
            others = [v for label, v in medians.items() if label != "Rustic"]
            ratio = f"{medians['Rustic'] / min(others):.1f}" if "Rustic" in medians and others and min(others) > 0 else "—"
            sums = {e.get("check") for e in entries.values()} - {None}
            agree = "yes" if len(sums) == 1 and "inconsistent" not in sums else "no"
            lines.append(f"| `{name}` | " + " | ".join(summary_cell(e) for e in entries.values()) + f" | {ratio} | {agree} |")
        lines.append("")
    dom = report.get("dom", {})
    if dom:
        lines += [
            f"DOM microbenchmarks (size {report['dom_size']}; JavaScript shells have no DOM, so only Rustic runs):",
            "",
            "| Workload | Rustic |",
            "| --- | --- |",
        ]
        lines += [f"| `{name}` | {summary_cell(dom[name])} |" for name in DOM_WORKLOADS if name in dom]
        lines.append("")
    page = report.get("page", {})
    if page:
        lines += [
            "Page pipeline (Rustic fixtures, 1280×720, median ms [95% CI]; other engines: not compared, "
            "they need the browser-hosted harness):",
            "",
            "| Fixture | " + " | ".join(PAGE_STAGES) + " |",
            "| --- | " + " | ".join("---" for _ in PAGE_STAGES) + " |",
        ]
        for fixture in PAGE_FIXTURES:
            cells = [summary_cell(page.get(f"{fixture}.{stage}", {"status": NOT_RUNNABLE})) for stage in PAGE_STAGES]
            lines.append(f"| `{fixture}` | " + " | ".join(cells) + " |")
        lines.append("")
    gpu = report.get("gpu")
    if gpu:
        adapter = gpu.get("adapter")
        name = (f"{adapter['name']} ({adapter['backend']}, {'software' if adapter['software'] else 'hardware'})"
                if adapter else "no adapter")
        lines += [
            f"GPU compositor frames on {name}, median ms [95% CI], upload to readback; the CPU reference "
            "composites the same scene (byte-identical results):",
            "",
            "| Scene | GPU | CPU reference |",
            "| --- | --- | --- |",
        ]
        results = gpu.get("results", {})
        for scene in GPU_SCENES:
            cells = [summary_cell(results.get(f"{scene}.{kind}", {"status": NOT_RUNNABLE})) for kind in ("gpu", "cpu")]
            lines.append(f"| `{scene}` | " + " | ".join(cells) + " |")
        lines += ["", "GPU frame budgets (ratio rows are GPU ÷ CPU, not ms):", "",
                  "| Metric | Budget | Median | Within |", "| --- | --- | --- | --- |"]
        for row in gpu.get("budgets", []):
            median = "not measured" if row["median_ms"] is None else f"{row['median_ms']:.2f}"
            lines.append(f"| `{row['metric']}` | {row['budget_ms']} | {median} | {'yes' if row['within'] else 'no'} |")
        lines.append("")
    budgets = report.get("budgets", [])
    if budgets:
        lines += ["M8 page budgets:", "", "| Metric | Budget ms | Median ms | Within |", "| --- | --- | --- | --- |"]
        for row in budgets:
            median = "not measured" if row["median_ms"] is None else f"{row['median_ms']:.2f}"
            lines.append(f"| `{row['metric']}` | {row['budget_ms']} | {median} | {'yes' if row['within'] else 'no'} |")
        lines.append("")
    return lines


def merge_reports(reports: list[dict]) -> str:
    """One Markdown page for several per-device JSON reports."""
    lines = ["# Rustic reference-device performance report", ""]
    if reports:
        commits = sorted({report.get("commit", "unknown") for report in reports})
        lines += [f"Commit(s): {', '.join(f'`{c}`' for c in commits)}. {len(reports)} reference devices.", ""]
        lines += ["| Device | Page budgets within | GPU frame budgets within | Slowest JS ratio |",
                  "| --- | --- | --- | --- |"]
        for report in reports:
            budgets = report.get("budgets", [])
            within = sum(1 for row in budgets if row["within"])
            ratios = []
            js = report.get("js", {})
            for name in js.get("Rustic", {}):
                rustic = js["Rustic"][name].get("median_ms")
                others = [js[label][name].get("median_ms") for label in js if label != "Rustic" and name in js[label]]
                others = [v for v in others if v]
                if rustic and others:
                    ratios.append(rustic / min(others))
            slowest = f"{max(ratios):.1f}" if ratios else "not compared"
            gpu_rows = report.get("gpu", {}).get("budgets", [])
            gpu_within = (f"{sum(1 for row in gpu_rows if row['within'])}/{len(gpu_rows)}"
                          if gpu_rows else "not measured")
            lines.append(f"| {report['device']['label']} | {within}/{len(budgets)} | {gpu_within} | {slowest} |")
        lines.append("")
    for report in reports:
        lines += render_device(report)
    return "\n".join(lines) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--rustic", type=Path, help="js_bench built in release mode")
    parser.add_argument("--dom", type=Path, help="dom_bench built in release mode")
    parser.add_argument("--page", type=Path, help="page_bench built in release mode")
    parser.add_argument("--engine", action="append", default=[], help='"label=command with {file}"')
    parser.add_argument("--size", type=int, default=200_000)
    parser.add_argument("--dom-size", type=int, default=20_000)
    parser.add_argument("--runs", type=int, default=11)
    parser.add_argument("--device", default=None, help="reference device name for the report")
    parser.add_argument("--commit", default=None, help="Rustic commit measured")
    parser.add_argument("--json", type=Path, help="also write the per-device report as JSON")
    parser.add_argument("--enforce-budgets", action="store_true", help="exit 1 when a page budget is missed")
    parser.add_argument("--gpu", type=Path, help="gpu_frame_bench built in release mode")
    parser.add_argument("--enforce-gpu", action="store_true",
                        help="exit 1 when a GPU frame budget is missed or no adapter opens")
    parser.add_argument("--merge", type=Path, nargs="+", help="merge per-device JSON reports into --output")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    if args.merge:
        reports = [json.loads(path.read_text(encoding="utf-8")) for path in args.merge]
        reports.sort(key=lambda report: report["device"]["label"])
        args.output.write_text(merge_reports(reports), encoding="utf-8")
        print(f"wrote {args.output}")
        return 0
    if not args.rustic and not args.page and not args.gpu:
        parser.error("--rustic, --page or --gpu is required")

    engines = []
    for spec in args.engine:
        label, _, command = spec.partition("=")
        if not command:
            parser.error(f"--engine needs label=command: {spec!r}")
        engines.append((label, command))

    label = args.device or f"{platform.system()} {platform.machine()}"
    report = {
        "device": device_info(label),
        "commit": args.commit or _run_text(["git", "-C", str(ROOT), "rev-parse", "--short", "HEAD"]) or "unknown",
        "runs": args.runs,
        "size": args.size,
        "dom_size": args.dom_size,
    }
    if args.rustic:
        workloads = sorted(path.stem for path in CORPUS.glob("*.js"))
        raw = {"Rustic": run_rustic(args.rustic, workloads, args.size, args.runs, ["--samples"])}
        for engine, command in engines:
            print(f"running {engine}", file=sys.stderr)
            raw[engine] = {name: run_shell(command, CORPUS / f"{name}.js", args.size, args.runs) for name in workloads}
        report["js"] = {engine: {name: summarize(s) for name, s in results.items()} for engine, results in raw.items()}
    if args.dom:
        dom = run_rustic(args.dom, DOM_WORKLOADS, args.dom_size, args.runs, [])
        report["dom"] = {name: summarize(s) for name, s in dom.items()}
    if args.page:
        keys = [f"{fixture}.{stage}" for fixture in PAGE_FIXTURES for stage in PAGE_STAGES]
        page = run_rustic(args.page, keys, args.size, args.runs, [])
        report["page"] = {key: summarize(s) for key, s in page.items()}
        report["budgets"] = check_budgets(report["page"], M8_PAGE_BUDGETS_MS)
    if args.gpu:
        report["gpu"] = run_gpu(args.gpu, args.runs)
        report["gpu"]["budgets"] = check_gpu_budgets(report["gpu"])

    args.output.write_text("\n".join(render_device(report)) + "\n", encoding="utf-8")
    if args.json:
        args.json.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote {args.output}")
    status = 0
    missed = [row["metric"] for row in report.get("budgets", []) if not row["within"]]
    if missed:
        print(f"M8 page budgets missed: {', '.join(missed)}", file=sys.stderr)
        if args.enforce_budgets:
            status = 1
    missed = [row["metric"] for row in report.get("gpu", {}).get("budgets", []) if not row["within"]]
    if missed:
        print(f"GPU frame budgets missed: {', '.join(missed)}", file=sys.stderr)
        if args.enforce_gpu:
            status = 1
    return status


if __name__ == "__main__":
    raise SystemExit(main())
