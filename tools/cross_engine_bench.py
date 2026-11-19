#!/usr/bin/env python3
"""PERF-01 cross-engine harness: run Rustic's JS-09 corpus in Rustic and in
JavaScript engine shells, and the DOM microbenchmarks in Rustic, then write a
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
"""

import argparse
import math
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


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--rustic", type=Path, required=True, help="js_bench built in release mode")
    parser.add_argument("--dom", type=Path, help="dom_bench built in release mode")
    parser.add_argument("--engine", action="append", default=[], help='"label=command with {file}"')
    parser.add_argument("--size", type=int, default=200_000)
    parser.add_argument("--dom-size", type=int, default=20_000)
    parser.add_argument("--runs", type=int, default=11)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    workloads = sorted(path.stem for path in CORPUS.glob("*.js"))
    engines = []
    for spec in args.engine:
        label, _, command = spec.partition("=")
        if not command:
            parser.error(f"--engine needs label=command: {spec!r}")
        engines.append((label, command))

    results = {"Rustic": run_rustic(args.rustic, workloads, args.size, args.runs, ["--samples"])}
    for label, command in engines:
        print(f"running {label}", file=sys.stderr)
        results[label] = {name: run_shell(command, CORPUS / f"{name}.js", args.size, args.runs) for name in workloads}

    labels = list(results)
    lines = [
        f"Host: {platform.machine()}, {platform.system()} {platform.release()}; "
        f"{args.runs} fresh-process (or fresh-agent) runs per cell, size {args.size}.",
        "",
        "Cells are median ms [95% confidence interval of the median]. The ratio is",
        "Rustic ÷ fastest other engine, so lower is better and 1.0 is parity.",
        "",
        "| Workload | " + " | ".join(labels) + " | Rustic ÷ best | Results agree |",
        "| --- | " + " | ".join("---" for _ in labels) + " | --- | --- |",
    ]
    for name in workloads:
        cells = [cell(results[label][name]) for label in labels]
        medians = {
            label: median_interval([ms for ms, _ in results[label][name]])[0]
            for label in labels
            if not isinstance(results[label][name], str)
        }
        others = [value for label, value in medians.items() if label != "Rustic"]
        ratio = f"{medians['Rustic'] / min(others):.1f}" if "Rustic" in medians and others else "—"
        sums = {checksum(results[label][name]) for label in labels} - {None}
        agree = "yes" if len(sums) == 1 and "inconsistent" not in sums else "no"
        lines.append(f"| `{name}` | " + " | ".join(cells) + f" | {ratio} | {agree} |")
    if args.dom:
        dom_names = ["dom_create_append", "dom_query", "dom_attributes", "dom_dispatch"]
        dom = run_rustic(args.dom, dom_names, args.dom_size, args.runs, [])
        lines += [
            "",
            f"DOM microbenchmarks (size {args.dom_size}; shells have no DOM, so only Rustic runs):",
            "",
            "| Workload | Rustic | Other engines |",
            "| --- | --- | --- |",
        ]
        for name in dom_names:
            lines.append(f"| `{name}` | {cell(dom[name])} | {NOT_RUNNABLE}: no DOM in a JavaScript shell |")
    args.output.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"wrote {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
