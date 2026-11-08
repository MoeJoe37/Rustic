# Testing strategy

## Current and planned coverage

The current suite has viewport boundary/error tests, CLI subprocess tests, a
rustdoc example, shared primitive boundary/concurrency tests, DOM hierarchy and
payload tests, seeded mutations against an independent ordered-tree model, repository
consistency checks, and compiler/linter checks.
URL parsing now adds focused host/origin/limit, file URL and diagnostics tests and an 887-case pinned WPT
URL-data subset; see [scope and provenance](URL.md). HTML input/tokenization adds
53 focused tests plus 1,079 selected declaration cases and all 4,626 cases in the
pinned html5lib reference files, with token fields, diagnostic order and chunk
invariance checked. See [HTML evidence](HTML.md) for
the pinned revision, selection policy and untested fields. Text-mode tests add
109 records expanded into 282 initial contexts and exercise pull feedback,
queued input, raw-byte boundaries, probe budgets and Unicode fixture adaptation.
Full browser conformance, rendering, coverage-guided fuzzing, GC stress, integration servers and benchmarks are planned;
they do not run in the initial CI. Add each harness with the subsystem it tests.

| Layer | What it proves | First milestone / execution |
| --- | --- | --- |
| Unit tests | State machines, arithmetic, object semantics, negative cases | M0 onward; every PR |
| Properties/invariants | Tree ownership, serializer stability, chunking, stale handles | M1; every relevant PR |
| Pinned upstream parser data | Token/tree recovery against known fixtures | M1; selected required corpus |
| Style/layout structural dumps | Cascade and box/fragment correctness | M2/M3; every relevant PR |
| CPU image/reftests | Paint order, text, clips, geometry, resize | M3; small required suite plus scheduled broad suite |
| Test262 | Language semantics and host-independent runtime behavior | M4; selected required suite, broader scheduled runs |
| Scripted document fixtures | Bindings, GC edges, tasks/events, reentry and invalidation | M5; every relevant PR |
| Network/profile integration | Redirects, policy, cancellation, partitioning and persistence | M6; loopback servers in PR jobs |
| Hostile-peer/isolation tests | Broker and OS restrictions, process recovery | M7; supported-platform required gates |
| WPT testharness/reftests | Exposed platform behavior through a real host/adapter | Incremental from M3/M5, integrated M8 |
| Fuzzing/sanitizers | Crashes, hangs, memory errors and hostile boundary input | M1 targets, expanded M4/M7; bounded smoke + scheduled campaigns |
| Performance corpus | Time, memory and responsiveness regressions | M3 onward; stable controlled runners |
| Manual/accessibility checks | Assistive technology, IME, permission and chrome flows | M8; release checklist |

## Deterministic harness contracts

Use an injected monotonic clock and explicit task pump; avoid sleeps as a promise
that an asynchronous operation completed. Fix random seeds, locale/time zone,
viewport/scale, fonts, color space and network fixtures. Test pending work after
close, navigation supersession, timeout, cancellation, and service death. Record
all reproduction inputs. Normal tests require no public internet and never use a
real user profile. Fetching a pinned upstream suite is a separate setup step.

Test parsing at arbitrary chunk boundaries, including inside encodings/entities
and script text. Test tree cycles, adoption between documents, destruction,
detached nodes and generation reuse. For incremental style/layout, compare results
with full recomputation under randomized mutation sequences. A fast path is not
correct merely because its own golden file changed with it.

For GC, collect at frequent allocation points, preserve rooted values through
reentry, reclaim unreachable JS/DOM listener cycles, and ensure weak caches do not
resurrect objects. Use negative syntax and runtime tests, exception propagation,
UTF-16/lone-surrogate cases, coercions, and abrupt completion. Native integrations
need ownership and shutdown tests in addition to happy-path output.

## Conformance runners

Start with data-driven URL/HTML corpora independently of a browser host. Pin suite
SHAs and retain their notices. M4's Test262 runner reads flags, features, includes,
negative phases/types, module/raw/strict execution and asynchronous completion;
enforce per-test time/memory limits. Separate unsupported features from unexpected
semantic failures. Never run every file as a plain non-strict script and call the
result Test262 conformance.

WPT integration needs the appropriate test servers, certificates, hostnames,
automation/runner adapter, screenshot mechanism, testharness result collection,
and timeout handling. Introduce suites only when their execution requirements are
met. See [WPT test types](https://web-platform-tests.org/writing-tests/index.html)
and [expectations](https://web-platform-tests.org/tools/wptrunner/docs/expectation.html).

Expected failures carry a reduced explanation, tracking task, platform condition
and suite revision. Unexpected failures and crashes block the declared required
subset; unexpected passes prompt expectation cleanup. Timeouts are not skips.
Keep reports with selected/run/pass/fail/skip/crash/timeout totals and test/subtest
denominators as described in [standards reporting](STANDARDS.md).

## Rendering tests

Bundle redistributable test fonts with license notices. Reference pages should
express the same result through a different construction where possible. Use
exact CPU pixels for deterministic primitives. If antialiasing/font behavior
needs tolerance, specify the maximum channel difference and count of differing
pixels per test with a reason; do not raise global thresholds to hide failures.
Store actual/expected/diff images, layout dump, display list, font hashes, scale
and backend. A reviewer inspects baseline updates. GPU tests compare against the
reference backend and exercise device loss/CPU fallback separately.

## Fuzzing, corpus and failure triage

Targets: URL/HTML/CSS/JS parsers, DOM mutation sequences, image/font adapters,
serialization/IPC, and storage recovery. Add grammar-aware generation and seeded
malformed cases. Enforce budgets inside targets; use timeouts outside them. Keep
small minimized regressions in source and large corpora in external test storage.
`rustic-fuzz` (DOC-08) provides dependency-free, deterministic, blind mutation
fuzzing for the HTML tokenizer, document and fragment parsers, the CSS parser, typed values, selectors, cascade and computed style, and the URL parser, with seed replay,
minimized reproducers, a required fixed-seed CI smoke job and a nightly fresh-seed
campaign; see [implementation progress](IMPLEMENTATION.md). Coverage-guided
cargo-fuzz/libFuzzer and sanitizers remain candidate tools after a
version/toolchain review; no workflow installs or claims to run them.

Triage order: reproducible crash/security issue, unexpected conformance regression,
deterministic functional failure, flaky result, performance regression. Minimize
the input, identify the violated contract/spec, add a failing regression test, fix
the shared behavior, then rerun the affected suites. Quarantine a flaky test only
with a reason and owner; never relabel a security failure as harmless flakiness.

## CI growth

Keep required PR runs short and deterministic: compile/lint/unit tests plus a
small reliable conformance set. Add heavy conformance/fuzz/sanitizer jobs on a
schedule when their harnesses exist. Artifacts need bounded sizes/retention and
must not include profiles or credentials. A green initial CI means the scaffold
works; it says nothing about browser compatibility or sandbox correctness.
