# PERF-01 reference-device report (M8) — 2026-11-23

The [Performance workflow](../../.github/workflows/perf.yml) ran on the six
GitHub-hosted CI reference devices named in
[PERFORMANCE.md](../PERFORMANCE.md#reference-devices): run
[45112682043](https://github.com/moejoe873/Rustic/actions/runs/45112682043)
on `feature` at `5db217a`, 11 fresh-agent or fresh-process runs per cell.
Every device built the benchmarks natively and returned the same checksum
for every workload in every engine. The per-device Markdown and JSON reports
are the run's `perf-<device>` artifacts; the merged page is `perf-report`.

## Devices

| Device | Runner image | CPU | Cores | Memory | Engines compared |
| --- | --- | --- | --- | --- | --- |
| Linux x64 | `ubuntu-24.04` | Intel Xeon 6975P-C | 4 | 15.3 GiB | V8 (Node 22.23.5) |
| Linux ARM64 | `ubuntu-24.04-arm` | Arm Neoverse-N2 | 4 | 15.6 GiB | V8 (Node 22.23.5) |
| Windows x64 | `windows-2022` | AMD EPYC 9V45 | 4 | 16.0 GiB | V8 (Node 22.23.5) |
| Windows ARM64 | `windows-11-arm` | Azure Cobalt 100 | 4 | 16.0 GiB | V8 (Node 24.22.3) |
| macOS ARM64 | `macos-15` | Apple M1 (virtual) | 3 | 7.0 GiB | V8 (Node 22.23.5), JavaScriptCore (macOS 15.8.2) |
| macOS x64 | `macos-15-intel` | Intel Core i7-8700B | 4 | 14.0 GiB | V8 (Node 22.23.2), JavaScriptCore (macOS 15.7.9) |

## M8 page budgets

Median end-to-end time of each page fixture (parse to raster, 1280×720), ms.
Every device is within every [M8 budget](../PERFORMANCE.md#m8-budgets).

| Fixture (budget) | Linux x64 | Linux ARM64 | Windows x64 | Windows ARM64 | macOS ARM64 | macOS x64 |
| --- | --- | --- | --- | --- | --- | --- |
| `page_tiny` (16.7) | 2.05 | 1.25 | 3.62 | 2.32 | 1.68 | 1.97 |
| `page_article` (250) | 58.95 | 71.81 | 106.54 | 110.91 | 93.11 | 131.51 |
| `page_large_dom` (600) | 149.02 | 191.84 | 331.18 | 336.94 | 257.46 | 407.44 |
| `page_selectors` (300) | 89.06 | 165.53 | 144.99 | 194.86 | 121.15 | 201.61 |
| `page_deep` (50) | 6.68 | 7.89 | 9.70 | 21.19 | 10.45 | 14.05 |

The first run of the workflow (run 45111119578, `43f9c36`) measured
`page_large_dom` over its budget on macOS x64 only; the rerun 15 minutes
later was at 407 ms. That spread on one shared Intel Mac is why the budgets
are about twice the first measurement and why hosted results never gate a
merge.

## JavaScript corpus

Rustic ÷ the fastest other engine on each device (lower is better; 1.0 is
parity). Rustic is a bytecode interpreter; the others run their JITs.

| Workload | Linux x64 | Linux ARM64 | Windows x64 | Windows ARM64 | macOS ARM64 | macOS x64 |
| --- | --- | --- | --- | --- | --- | --- |
| `alloc_churn` | 14.1 | 14.6 | 16.1 | 19.0 | 16.1 | 33.3 |
| `arrays` | 5.1 | 5.8 | 5.2 | 6.0 | 5.5 | 10.5 |
| `calls` | 10.7 | 13.7 | 9.6 | 16.9 | 16.0 | 37.4 |
| `closures` | 16.6 | 20.6 | 14.0 | 19.3 | 13.5 | 41.6 |
| `framework_diff` | 22.4 | 23.3 | 28.5 | 29.3 | 25.8 | 51.7 |
| `json_tree` | 4.2 | 4.1 | 5.1 | 5.4 | 6.2 | 10.8 |
| `property_access` | 5.3 | 5.7 | 3.7 | 4.7 | 6.0 | 19.8 |
| `strings` | 4.1 | 5.0 | 4.7 | 4.0 | 7.8 | 12.4 |

These are the first native ARM64 timings of the JS-09 corpus (Linux,
Windows and macOS), which pays [TD-125](../TECH_DEBT.md#td-125--arm64-benchmark-timings-are-emulated-paid).
The M8 speed ratchet (at most 2.0) is not met by any workload on any
device; see [PERFORMANCE.md](../PERFORMANCE.md#m8-budgets) for where that gap
is closed.

## Not covered (external)

| Item | Why |
| --- | --- |
| Physical reference machines with fixed clocks | Need hardware the project does not own; hosted runners are shared VMs |
| Chrome, Firefox and Safari as browsers (page load, Speedometer, MotionMark) | Needs the browser-hosted harness and real desktops with those browsers |
| SpiderMonkey on hosted runners | No shell on the runner images; measured on a controlled host in the [M5 report](2026-11-19-perf-01-m5.md) |
| Memory and energy | Energy needs platform counters or meters on physical devices |
