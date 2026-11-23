# ADR 0072 — Automation and DevTools protocol and its authority model

Date: 2026-11-23
Status: Accepted (local stdio sessions; remote debugging deliberately absent)

## Context

M8 needs developer tools (UI-04: DOM, style and layout inspection, a
console, source debugging, network diagnostics) and a headless automation
surface (UI-06) that the WPT runner and later embedders drive. Every such
surface is an authority boundary: a debugger reads every variable of a
page, an automation client can click on the user's behalf, and network
diagnostics see cookies and credentials. Chromium's DevTools protocol and
WebDriver BiDi expose this over a socket, and a reachable debugging port is
a long-standing source of compromise. The renderer is untrusted (ADR 0056),
so it cannot be the place that decides what a client may do.

## Decision

1. **`rustic automate <file>`**, protocol `rustic-automation/1`: one JSON
   object per line on standard input (`{"id", "method", "params"}`), one
   response (`{"id", "result"}` or `{"id", "error": {"code", "message"}}`)
   or event (`{"event", "params"}`) per line on standard output. Lines are
   at most 1 MiB in and 64 MiB out; the JSON reader is bounded (depth 32).
   There is **no listening socket**: remote debugging is off by
   construction, and exposing one later needs a new record.
2. **Grants.** `inspect`, `console` and `profile` are the default (read
   only, no page code runs). `network`, `input`, `evaluate` and `debug` are
   opt-in through `--devtools-allow`, which replaces the default. Every
   method has exactly one required grant (`rustic_devtools::METHODS`).
3. **The broker is the gate.** In a split run the broker reads standard
   input, admits each line (`protocol::admit`: known method, required grant,
   well-formed parameters) and answers refusals itself; only admitted
   commands cross to the renderer (`AutomationCommand`, tag 32). The
   renderer may only ask for the next command (`AutomationRead`) and send
   output lines (`AutomationOutput`), which the broker validates as one
   well-formed response or event before writing them out. A compromised
   renderer cannot widen its grants, read stdin, or forge non-protocol
   output. A paused session is bounded by the renderer timeout.
4. **Debugger inside the VM.** `rustic_js::debug` adds a `DebugHook`.
   Code compiled while a hook is attached carries `Step` ops at statement
   starts (and `DebuggerStep` for `debugger`); code compiled without one has
   none, so pages without DevTools pay nothing. Breakpoints are by script
   and line (first statement on the line), stepping is into/over/out by
   frame depth, and stack and scope inspection reads registers and
   environments without running getters or other page code. Inline script
   lines are mapped to document lines.
5. **Diagnostics without secrets.** The network log records method, URL,
   status, type and sizes; `Cookie`, `Set-Cookie`, `Authorization`,
   `Proxy-Authorization` and similar headers and URL userinfo are redacted
   before they reach the client.
6. **WPT runner adapter.** `tools/run_wpt_web_tests.py` reads each test's
   completion report through `rustic automate` with only the `inspect`
   grant (`--driver dump-dom` keeps the old path); both give the same
   recorded baseline.

## Alternatives considered

- **Chrome DevTools Protocol or WebDriver BiDi now.** Large surfaces whose
  value is compatibility with existing tools over a socket; they can be
  layered on this protocol later behind a new record.
- **A debugging port bound to localhost.** Any local process (or a browser
  page through DNS rebinding) could reach it; stdio limits the client to
  whoever started the process.
- **Checking grants in the renderer.** A compromised renderer would ignore
  them.
- **Always-on step instrumentation.** Costs every page; compiling steps
  only with a hook attached keeps the fast path unchanged.

## Consequences

- Automation clients must start Rustic themselves; there is no attach.
- Code compiled before a hook attaches has no steps; `eval`, `Function`
  and module code have none yet (TD-187).
- Inline line mapping and the network log have documented gaps
  (TD-188, TD-189); CSS matched-rule inspection and key input are not in
  version 1 (TD-190).

## Validation

`crates/rustic-js/tests/debugger.rs` (breakpoints, stepping, scopes, no
page code during inspection, profiling), `crates/rustic-devtools/tests/protocol.rs`
(gate, codec, grants, redaction), `crates/rustic-cli/tests/devtools.rs`
(every method family in single-process and split runs, broker refusal, no
remote endpoint, compromised renderer, paused-session timeout), and the
CI WPT step through the automate driver.

## Revisit trigger

A request for remote debugging, an embedder API, CDP/BiDi compatibility,
or a finding against the gate.
