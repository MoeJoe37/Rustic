# ADR 0040: Local parser-inserted JavaScript modules

- Date: 2026-11-17
- Status: Accepted for the local JavaScript module slice

The opt-in scripted load uses the classic script provider to fetch module
entry points and static imports. It shares the request and byte budgets.
Each module URL has one source record per load. Inline modules have element
identities and use the document base at preparation time. Static imports
resolve against the importing module and pass the same local URL grant.

Async modules can start when their graph settles. Other modules share the
classics' ordered after-EOF queue. Evaluation uses the existing ECMAScript
linker and page realm. The module map retains namespaces and completion
promises across entries. Module evaluation leaves `document.currentScript`
null. Hosts can pump tasks while evaluation awaits a promise. Document
publication waits for pending evaluations.

The local host supplies UTF-8 bytes and enforces canonical filesystem
containment when opening resources. Dynamic imports, import maps, data
module types, and readiness events remain separate work.

[WHATWG HTML script preparation](https://html.spec.whatwg.org/multipage/scripting.html#prepare-the-script-element)
