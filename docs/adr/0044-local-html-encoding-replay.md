# ADR 0044 — Local HTML encoding replay

Date: 2026-11-18
Status: Accepted for WEB-05

The local loader selects an encoding before it publishes parser state to
author code. It retains at most 64 KiB of original bytes from the same
file handle that supplies the rest of the document. A BOM commits the
encoding immediately. Otherwise, a private HTML parser starts in UTF-8
and finds the first effective charset declaration in the prefix. It
uses the page's scripting mode for tree construction. Script boundaries
in this pass have no author side effects.

The loader replays the original prefix into the admitted document in
the selected encoding, then streams the rest. Author scripts run once.
The definitive parser applies its own raw-byte and tree budgets. An
absent declaration or a probe that hits its limits leaves UTF-8 as the
local fallback.

The selected encoding is available through the three Document encoding
getters. It supplies the fallback for local stylesheets and classic
scripts. Classic scripts can supply their own charset label. Modules
use UTF-8.

The local replay boundary is fixed before publication. Transport-level
confidence and navigation replacement after publication belong to
NET-02 and `url.restart`.

The integration regression declares Windows-1252 after the first
script and beyond byte 1024. It exercises byte-at-a-time input, a legacy
stylesheet, an external classic and the encoding getters, with one
execution of the first inline script. Parser tests exercise BOM
precedence, the replay boundary and meta declarations in comments,
raw text, templates and scripting-dependent noscript markup.

[HTML input byte stream](https://html.spec.whatwg.org/multipage/parsing.html#the-input-byte-stream)
[ADR 0043](0043-encoding-standard.md)
