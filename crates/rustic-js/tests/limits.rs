//! JS-09: the limits and performance gate. Deep recursion, heap
//! exhaustion and endless loops end within their budgets; the benchmark
//! corpus (`benchmarks/*.js`) gives the same results with and without
//! inline caches and with a collection at every safe point, and none of
//! its workloads does quadratic work; and the heap-size report matches
//! the committed one.

use std::path::Path;

use rustic_base::CancellationToken;
use rustic_js::runtime::{Agent, Roots};
use rustic_js::{EvalError, VmLimits};

fn text(agent: &mut Agent, source: &str) -> String {
    match agent.evaluate(source) {
        Ok(value) => agent.display(value),
        Err(EvalError::Throw(value)) => {
            let s = agent.to_string(value).expect("a printable error");
            format!("uncaught {}", agent.heap.string(s).to_text())
        }
        Err(other) => format!("error: {other}"),
    }
}

// ---- limits ---------------------------------------------------------------

#[test]
fn deep_recursion_throws_a_catchable_range_error_at_the_frame_limit() {
    let mut agent = Agent::new().expect("agent");
    agent.set_vm_limits(VmLimits {
        max_frames: 100,
        ..VmLimits::default()
    });
    // The script's own frame is one of the hundred.
    let source = "var depth = 0; function f() { depth++; f() }
        var caught; try { f() } catch (e) { caught = e instanceof RangeError } caught + ' ' + depth";
    assert_eq!(text(&mut agent, source), "true 99");
    // One frame fewer fits, and the stack is usable afterwards.
    assert_eq!(
        text(
            &mut agent,
            "function g(n) { return n ? g(n - 1) : 'ok' } g(97)"
        ),
        "ok"
    );
    assert_eq!(
        text(&mut agent, "try { g(99) } catch (e) { e.message }"),
        "Maximum call stack size exceeded"
    );
}

#[test]
fn the_register_stack_is_bounded_too() {
    let mut agent = Agent::new().expect("agent");
    agent.set_vm_limits(VmLimits {
        max_stack_values: 4096,
        ..VmLimits::default()
    });
    assert_eq!(
        text(
            &mut agent,
            "function f(a, b, c, d, e, f2, g, h) { var x = a, y = b, z = c; return f(x, y, z, d, e, f2, g, h) }
             try { f(1, 2, 3, 4, 5, 6, 7, 8) } catch (e) { e.name }"
        ),
        "RangeError"
    );
    assert_eq!(text(&mut agent, "1 + 1"), "2");
}

#[test]
fn heap_exhaustion_ends_the_run_and_leaves_a_usable_agent() {
    let limit = 2 * 1024 * 1024;
    let mut agent = Agent::with_heap_limit(limit).expect("agent");
    for source in [
        // Objects kept alive.
        "(function () { var keep = []; for (;;) keep.push({n: keep.length}) })()",
        // One string doubled until it cannot fit.
        "(function () { var s = 'abcdefgh'; for (;;) s = s + s })()",
        // A rope whose flat copy would not fit is refused when it is made.
        "(function () { var t = 'x'.repeat(1000), u = ''; for (;;) u += t })()",
        // Script cannot catch it.
        "(function () { try { var k = []; for (;;) k.push([k.length]) } catch (e) { return 'caught' } })()",
    ] {
        assert_eq!(
            agent.evaluate(source),
            Err(EvalError::OutOfMemory),
            "{source}"
        );
        assert!(agent.heap.bytes_used() <= limit, "{source}");
        // Nothing the run made is reachable; the agent then runs again.
        agent.collect_garbage(Roots::default());
        assert_eq!(
            text(&mut agent, "[1, 2, 3].map(x => x * 2).join()"),
            "2,4,6"
        );
    }
}

#[test]
fn requests_larger_than_the_heap_are_catchable_range_errors() {
    let mut agent = Agent::with_heap_limit(2 * 1024 * 1024).expect("agent");
    for source in [
        "try { 'x'.repeat(1e9) } catch (e) { e.name }",
        "try { new ArrayBuffer(1e9) } catch (e) { e.name }",
        "try { 'x'.padEnd(1e9) } catch (e) { e.name }",
    ] {
        assert_eq!(text(&mut agent, source), "RangeError", "{source}");
    }
}

#[test]
fn endless_loops_stop_on_cancellation_and_on_the_step_budget() {
    let mut agent = Agent::new().expect("agent");
    let token = CancellationToken::default();
    agent.set_cancellation(Some(token.clone()));
    token.cancel();
    for source in [
        "for (;;) {}",
        "function f() { try { f() } finally { f() } } f()",
        "(function* () { for (;;) yield 1 })().toArray()",
        "/(a*)*b/.test('a'.repeat(40))",
    ] {
        assert_eq!(
            agent.evaluate(source),
            Err(EvalError::Cancelled),
            "{source}"
        );
    }
    agent.set_cancellation(None);
    agent.set_step_budget(Some(1000));
    assert_eq!(agent.evaluate("for (;;) {}"), Err(EvalError::Cancelled));
    assert_eq!(agent.step_budget(), Some(0));
    agent.set_step_budget(None);
    assert_eq!(text(&mut agent, "6 * 7"), "42");
}

#[test]
fn collation_work_respects_the_step_budget() {
    let mut agent = Agent::new().expect("agent");
    agent
        .evaluate("const longer = '1'.repeat(4_096); const compare = Intl.Collator('en', {numeric: true}).compare;")
        .expect("collation setup");
    agent.set_step_budget(Some(1_000));
    assert_eq!(
        agent.evaluate("compare(longer, '2')"),
        Err(EvalError::Cancelled)
    );
    agent.set_step_budget(None);
    assert_eq!(text(&mut agent, "compare(longer, '2')"), "1");
}

#[test]
fn uri_work_respects_the_step_budget() {
    let mut agent = Agent::new().unwrap();
    agent
        .evaluate("let text = '🚀'.repeat(4096); let escaped = '%61'.repeat(4096);")
        .unwrap();
    for source in ["encodeURIComponent(text)", "decodeURIComponent(escaped)"] {
        agent.set_step_budget(Some(256));
        assert_eq!(agent.evaluate(source), Err(EvalError::Cancelled));
        assert_eq!(agent.step_budget(), Some(0));
    }
    agent.set_step_budget(None);
    assert_eq!(text(&mut agent, "encodeURIComponent('é')"), "%C3%A9");
}

#[test]
fn number_formatting_work_respects_the_step_budget() {
    let mut agent = Agent::new().expect("agent");
    agent
        .evaluate("const digits = '7'.repeat(4_096); const big = BigInt(digits); const format = new Intl.NumberFormat('en').format;")
        .expect("number format setup");
    for source in ["format(digits)", "format(big)", "big.toLocaleString('de')"] {
        agent.set_step_budget(Some(1_000));
        assert_eq!(
            agent.evaluate(source),
            Err(EvalError::Cancelled),
            "{source}"
        );
        agent.set_step_budget(None);
    }
    assert_eq!(text(&mut agent, "format(big).length"), "5461");
    assert_eq!(text(&mut agent, "format(digits)"), "∞");
}

#[test]
fn segmentation_work_respects_the_step_budget() {
    let mut agent = Agent::new().expect("agent");
    agent
        .evaluate("const long = 'Word. '.repeat(1_024); const sentences = new Intl.Segmenter('en', { granularity: 'sentence' });")
        .expect("segmenter setup");
    agent.set_step_budget(Some(1_000));
    assert_eq!(
        agent.evaluate("sentences.segment(long)"),
        Err(EvalError::Cancelled)
    );
    agent.set_step_budget(None);
    assert_eq!(
        text(&mut agent, "[...sentences.segment(long)].length"),
        "1024"
    );
}

#[test]
fn segment_iterators_keep_their_boundary_tables_charged() {
    let mut agent = Agent::new().expect("agent");
    agent
        .evaluate("const input = 'x'.repeat(8192); const segmenter = new Intl.Segmenter();")
        .expect("segmenter setup");
    let baseline = agent.collect_garbage(Roots::default()).bytes_after;
    agent
        .evaluate("let iterator = segmenter.segment(input)[Symbol.iterator]();")
        .expect("segment iterator setup");
    let retained = agent.collect_garbage(Roots::default()).bytes_after;
    assert!(retained >= baseline + 8192 * size_of::<u32>());
    assert_eq!(text(&mut agent, "iterator.next().value.segment"), "x");
    agent
        .evaluate("iterator = undefined;")
        .expect("drop iterator");
    let released = agent.collect_garbage(Roots::default()).bytes_after;
    assert!(retained.saturating_sub(released) >= 8192 * size_of::<u32>());
}

#[test]
fn cancelled_segment_copy_preserves_the_iterator_position() {
    let mut agent = Agent::new().expect("agent");
    agent
        .evaluate(
            r#"const word = 'a' + '\u0301'.repeat(8192);
       const segments = new Intl.Segmenter().segment(word + 'b');
       const iterator = segments[Symbol.iterator]();"#,
        )
        .expect("segment iterator setup");
    for source in ["segments.containing(4000)", "iterator.next()"] {
        agent.set_step_budget(Some(8));
        assert_eq!(
            agent.evaluate(source),
            Err(EvalError::Cancelled),
            "{source}"
        );
        agent.set_step_budget(None);
    }
    assert_eq!(
        text(&mut agent, "iterator.next().value.segment === word"),
        "true"
    );
    assert_eq!(text(&mut agent, "iterator.next().value.segment"), "b");
    assert_eq!(text(&mut agent, "iterator.next().done"), "true");
}

// ---- ropes ----------------------------------------------------------------

#[test]
fn long_concatenations_are_ropes_with_string_semantics() {
    let mut agent = Agent::new().expect("agent");
    let source = "var a = 'ab'.repeat(200), b = 'cd'.repeat(200), wide = '\\u20ac'.repeat(300);
        var r = a + b, w = a + wide, deep = '';
        for (var i = 0; i < 3000; i++) deep += 'x' + (i % 10);
        var again = a + b;
        [r.length, r.charAt(399), r.charAt(400), r === again, r == again, r < w,
         w.length, w.charCodeAt(400), w.slice(398, 402) === 'ab\\u20ac\\u20ac',
         deep.length, deep.slice(-4), deep.indexOf('x9x0'), ({[r]: 1})[again],
         (r + r).length, typeof r, JSON.stringify(a + b).length].join()";
    let expected = "800,b,c,true,true,true,700,8364,true,6000,x8x9,18,1,1600,string,802";
    assert_eq!(text(&mut agent, source), expected);
    // The same under a collection at every safe point.
    let mut stressed = Agent::new().expect("agent");
    stressed.heap.set_gc_stress(true);
    assert_eq!(text(&mut stressed, source), expected);
}

#[test]
fn appending_in_a_loop_allocates_linearly() {
    let allocated = |n: u32| {
        let mut agent = Agent::new().expect("agent");
        let before = agent.heap.allocated_bytes();
        let source = format!(
            "var s = ''; for (var i = 0; i < {n}; i++) s += 'item' + (i % 10) + ';'; s.length"
        );
        assert_eq!(text(&mut agent, &source), (6 * n).to_string());
        agent.heap.allocated_bytes() - before
    };
    let (small, large) = (allocated(20_000), allocated(40_000));
    assert!(
        large < small * 5 / 2,
        "doubling the appends took {small} -> {large} bytes"
    );
}

#[test]
fn a_collection_flattens_live_ropes_and_frees_their_chains() {
    let mut agent = Agent::new().expect("agent");
    agent.collect_garbage(Roots::default());
    let before = agent.heap.string_count();
    agent
        .evaluate("var s = ''; for (var i = 0; i < 5000; i++) s += 'part' + i; s.length")
        .expect("runs");
    // Only `s` is reachable: the 5,000 links behind it go, and so do
    // their pieces, without anything having read the string.
    agent.collect_garbage(Roots::default());
    let after = agent.heap.string_count();
    assert!(after < before + 50, "{before} strings, then {after}");
    assert_eq!(
        text(&mut agent, "s.slice(0, 11) + s.length"),
        "part0part1p38890"
    );
}

#[test]
fn a_rope_too_large_to_copy_stays_a_rope_through_a_collection() {
    // Twenty doublings make 8 MiB of text out of twenty small links. The
    // copy would not fit this heap, so nothing may force it.
    let mut agent = Agent::with_heap_limit(4 * 1024 * 1024).expect("agent");
    let source = "var s = 'abcdefgh'.repeat(64); for (var i = 0; i < 12; i++) s = s + s; s.length";
    assert_eq!(text(&mut agent, source), "2097152");
    agent.collect_garbage(Roots::default());
    assert!(agent.heap.bytes_used() < 1024 * 1024);
    // Reading it needs the copy, which fits; one more doubling does not.
    assert_eq!(text(&mut agent, "s.charAt(2097151)"), "h");
    assert_eq!(agent.evaluate("s + s"), Err(EvalError::OutOfMemory));
}

// ---- the benchmark corpus ---------------------------------------------------

/// Every workload with its result for `run(2000)`. An independent model
/// of the arithmetic workloads (in Python) gives the same numbers.
const CORPUS: [(&str, &str); 8] = [
    ("alloc_churn", "2936"),
    ("arrays", "98403973"),
    ("calls", "36747"),
    ("closures", "66002"),
    ("framework_diff", "40"),
    ("json_tree", "2091003"),
    ("property_access", "6997000"),
    ("strings", "128864"),
];

fn workload(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("benchmarks")
        .join(format!("{name}.js"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Run `run(size)` of a workload and return its result, the safe points
/// it passed and the bytes it allocated.
fn measure(name: &str, size: u32, configure: impl Fn(&mut Agent)) -> (String, u64, u64) {
    let mut agent = Agent::new().expect("agent");
    configure(&mut agent);
    agent.evaluate(&workload(name)).expect("the workload loads");
    let budget = 1 << 40;
    agent.set_step_budget(Some(budget));
    let allocated = agent.heap.allocated_bytes();
    let result = text(&mut agent, &format!("run({size})"));
    (
        result,
        budget - agent.step_budget().expect("a budget is set"),
        agent.heap.allocated_bytes() - allocated,
    )
}

#[test]
fn the_corpus_lists_every_workload() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("benchmarks");
    let mut files: Vec<String> = std::fs::read_dir(dir)
        .expect("the corpus directory")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .into_string()
                .expect("UTF-8")
        })
        .collect();
    files.sort();
    let expected: Vec<String> = CORPUS
        .iter()
        .map(|(name, _)| format!("{name}.js"))
        .collect();
    assert_eq!(files, expected);
}

#[test]
fn corpus_results_do_not_depend_on_caches_or_collection() {
    for (name, expected) in CORPUS {
        let (plain, steps, _) = measure(name, 2000, |_| {});
        assert_eq!(plain, expected, "{name}");
        let (uncached, uncached_steps, _) = measure(name, 2000, |a| a.set_inline_caches(false));
        assert_eq!(uncached, expected, "{name} without inline caches");
        assert_eq!(uncached_steps, steps, "{name} without inline caches");
        let (stressed, stressed_steps, _) = measure(name, 2000, |a| a.heap.set_gc_stress(true));
        assert_eq!(stressed, expected, "{name} under GC stress");
        assert_eq!(stressed_steps, steps, "{name} under GC stress");
    }
}

#[test]
fn no_workload_is_quadratic_in_its_input() {
    for (name, _) in CORPUS {
        let (_, steps, bytes) = measure(name, 8000, |_| {});
        let (_, steps2, bytes2) = measure(name, 16000, |_| {});
        // Linear work doubles; sorting adds a logarithmic factor. A
        // quadratic workload would quadruple.
        assert!(
            steps2 < steps * 5 / 2 && bytes2 < bytes.max(4096) * 5 / 2,
            "{name}: steps {steps} -> {steps2}, bytes {bytes} -> {bytes2}"
        );
    }
}

#[test]
fn the_corpus_fits_a_small_heap() {
    // Peak live memory, not total allocation, is what a workload needs:
    // each fits a heap far smaller than what it allocates in total.
    for (name, expected) in CORPUS {
        let mut agent = Agent::with_heap_limit(8 * 1024 * 1024).expect("agent");
        agent.evaluate(&workload(name)).expect("the workload loads");
        assert_eq!(text(&mut agent, "run(2000)"), expected, "{name}");
        assert!(agent.heap.peak_bytes() <= 8 * 1024 * 1024, "{name}");
    }
}

// ---- the heap-size report ---------------------------------------------------

/// Bytes each of `count` values made by `make` costs once only they are
/// reachable: the growth of the live heap over an array of `count` nulls.
fn bytes_each(make: &str, count: usize) -> usize {
    let mut agent = Agent::new().expect("agent");
    agent
        .evaluate(&format!(
            "var keep = []; for (var i = 0; i < {count}; i++) keep[i] = null;"
        ))
        .expect("runs");
    let before = agent.collect_garbage(Roots::default()).bytes_after;
    agent
        .evaluate(&format!(
            "var make = {make}; for (var i = 0; i < {count}; i++) keep[i] = make(i); make = null;"
        ))
        .expect("runs");
    let after = agent.collect_garbage(Roots::default()).bytes_after;
    (after - before) / count
}

fn heap_report() -> String {
    let mut lines = vec![
        "# Bytes the engine charges for representative values (64-bit targets).".to_owned(),
        "# Regenerate with RUSTIC_HEAP_REPORT_BLESS=1; explain any line that grows.".to_owned(),
    ];
    let mut line = |label: &str, bytes: usize| lines.push(format!("{label}: {bytes}"));
    line(
        "empty object",
        bytes_each("function () { return {} }", 1000),
    );
    line(
        "object with four properties",
        bytes_each("function (i) { return {a: i, b: 2, c: 3, d: 4} }", 1000),
    );
    line(
        "dense array of 1,000 small numbers",
        bytes_each(
            "function () { var a = []; for (var j = 0; j < 1000; j++) a[j] = j; return a }",
            50,
        ),
    );
    line(
        "Latin-1 string of 1,000 characters",
        bytes_each("function (i) { return String(i % 10).repeat(1000) }", 100),
    );
    line(
        "UTF-16 string of 1,000 characters",
        bytes_each("function () { return '\\u20ac'.repeat(1000) }", 100),
    );
    line(
        "concatenation of two 500-character strings",
        bytes_each(
            "(function () { var a = 'a'.repeat(500), b = 'b'.repeat(500); return function () { return a + b } })()",
            100,
        ),
    );
    line(
        "closure capturing nothing",
        bytes_each("function () { return function () {} }", 1000),
    );
    line(
        "closure capturing one variable",
        bytes_each("function (i) { return function () { return i } }", 1000),
    );
    line(
        "symbol",
        bytes_each("function () { return Symbol() }", 1000),
    );
    line(
        "fractional number in an array slot",
        bytes_each("function (i) { return i + 0.5 }", 1000),
    );
    // Compiled code: the cost of a function and of each instruction.
    let mut agent = Agent::new().expect("agent");
    let (bytes, ops) = agent.heap.code_stats();
    agent.evaluate("function empty() {}").expect("runs");
    let (with_empty, ops_with_empty) = agent.heap.code_stats();
    // The script's own code is collected; the function's stays.
    agent.collect_garbage(Roots::default());
    let (kept, kept_ops) = agent.heap.code_stats();
    let _ = (with_empty, ops_with_empty);
    line("compiled empty function", kept - bytes);
    line("instructions in an empty function", kept_ops - ops);
    agent
        .evaluate(
            "function busy(a, b) { var t = 0; for (var i = 0; i < a; i++) { t += b.x * i; if (t > 100) t -= b.y } return t }",
        )
        .expect("runs");
    agent.collect_garbage(Roots::default());
    let (busy, busy_ops) = agent.heap.code_stats();
    line(
        "bytes per instruction, tables included (x100)",
        (busy - kept) * 100 / (busy_ops - kept_ops),
    );
    let bare = Agent::new().expect("agent");
    line("agent with the standard library", bare.heap.bytes_used());
    lines.join("\n") + "\n"
}

#[test]
#[cfg(target_pointer_width = "64")]
fn the_heap_report_matches_the_committed_one() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/heap_report.txt");
    let report = heap_report();
    if std::env::var_os("RUSTIC_HEAP_REPORT_BLESS").is_some() {
        std::fs::write(&path, &report).expect("the report is writable");
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(
        report, committed,
        "the heap-size report changed; if the change is intended, regenerate it with \
         RUSTIC_HEAP_REPORT_BLESS=1 and explain any line that grew"
    );
}

#[test]
fn the_heap_report_is_deterministic() {
    assert_eq!(heap_report(), heap_report());
}
