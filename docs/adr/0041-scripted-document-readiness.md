# ADR 0041: Scripted document readiness and headless lifecycle

- Date: 2026-11-17
- Status: Accepted for the local scripted host

The engine moves the document to interactive after EOF and before running
deferred scripts. It dispatches DOMContentLoaded after the ordered script
queue drains. Async scripts and outstanding module evaluations can still
delay document publication and the window load event. Each transition uses
the existing trusted dispatch and a bounded microtask checkpoint.

The command-line host can opt into the same lifecycle on local files. It
services local requests at parser barriers and EOF. The directory grant
matches the existing local stylesheet provider. The CLI uses a manual clock
for reproducible timer deadlines and offers one frame before capturing the
document. An unfinished load without a timer or request and workloads that
exceed the timer turn limit produce explicit errors. Scripting defaults to off.

External script acquisition failure and lifecycle events are synchronous with
the host's supported completion paths. Stylesheet dependencies and wider task
source interleaving are tracked by TD-134.

[WHATWG HTML stop parsing](https://html.spec.whatwg.org/multipage/parsing.html#stop-parsing)
