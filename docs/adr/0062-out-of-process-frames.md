# ADR 0062 — Out-of-process frames

Date: 2026-11-21
Status: Accepted for `render` in split runs (verified on Linux x86-64, sandboxed and not)

## Context

ADR 0057 §5 left frames for when iframes exist: "a cross-site iframe gets
its own renderer keyed like a top-level document". Rustic had no iframe
loading at all; an `<iframe>` was a 300×150 replaced box with nothing in
it. Two things make frames an isolation question rather than a layout
one. First, anything a renderer can address it can read (Spectre, or plain
code execution after a bug), so a cross-site frame's document, its
subresources *and its pixels* must not reach the embedding renderer.
Second, the embedder paints above and around the frame: an overlay with
`opacity`, a clip, a rounded corner, an outline. Compositing therefore has
to respect the embedder's paint order without the embedder seeing the
frame.

Chromium solves this with out-of-process iframes whose surfaces the GPU
process composites; Gecko's Fission does the same through its compositor.
Rustic has no compositor or GPU process yet (M8), and the CLI renders one
static frame on the CPU.

## Decision

1. **Every frame document gets its own renderer and grant.** When `render`
   runs in a renderer, the renderer lays out its page, finds its
   `<iframe>` elements and sends the broker one `Frame` request each
   (`src` resolved against the document base, or `srcdoc`, the `sandbox`
   tokens and the content-box size in CSS pixels; IPC protocol version 2).
   The broker decides the frame's document and grant from *its own* record
   of the embedding document, never from the request's claims, starts a
   fresh renderer for it (sandboxed like any renderer), renders it at that
   size and the top-level device scale, and keeps the pixels. The
   embedding renderer gets back only a handle, or a refusal. This is the
   strict end of ADR 0057's key rule: same-site frames do not share a
   process either, because the CLI runs one renderer per document. Its
   cost (no same-origin scripting across frames) is TD-178.
2. **Grants.** A local document's frames are files inside its granted
   directory (the same containment check as its other resources, after
   symbolic links resolve); the frame's renderer is granted the frame
   file's own directory, which is never wider than the parent's. `data:`
   frames get no grant beyond their bytes. A network document's frames are
   nested navigations through its session: CSP `frame-src` (falling back
   to `child-src` and `default-src`) and mixed content on every redirect
   hop, `Sec-Fetch-Dest: iframe`, the response's CSP `frame-ancestors` or,
   without it, `X-Frame-Options` (HTML's algorithm, including the
   conflicting-values rule), and the embedder's COEP (a `require-corp`
   parent takes only frames with a COEP, and cross-origin ones also need
   CORP). The frame's session shares the cookie jar and cache with its
   parent but carries the top-level site, so cross-site frames are third
   parties: no `SameSite=Lax` cookies, partitioned cache and CHIPS.
   `srcdoc` takes the parent's origin, policies and grant; `sandbox`
   without `allow-same-origin` gives an opaque origin (no storage, and for
   local `srcdoc` no grant), and without `allow-scripts` no scripts.
3. **Compositing without the embedder seeing the frame.** The embedding
   renderer rasterizes its page with each accepted frame's content box
   painted opaque black, then rasterizes the frame's rectangle again with
   that frame opaque white. For source-over compositing, which is linear
   in premultiplied color, the difference is exactly how much of the
   frame shows at each pixel after every clip, opacity group and overlay
   above it. The renderer sends the black page plus one coverage byte per
   pixel per frame; the broker checks the surface (size, premultiplied
   pixels, layer bounds, handles it issued) and adds
   `coverage × frame pixel` (`rustic_raster::Surface::add_frame`), then
   encodes the PNG itself. A renderer that lies about coverage can only
   misplace pixels it never sees, which it could paint anyway. A frame is
   composited before its own parent, so nesting works bottom-up.
4. **Failure is local.** A frame that is refused, fails to load, crashes,
   breaks the protocol, times out or renders at the wrong size paints
   white, with one diagnostic line; the page renders on. `about:blank`
   needs no renderer. Limits: 16 frames per document, frames nested three
   deep below the top level, 32 frame documents per command; frames past
   a limit paint white.

## Consequences

Frames cost a process each, plus one extra rasterization of each frame's
rectangle in the embedder. Blend modes other than `normal` and color
filters are not linear, so frames under them composite approximately;
frames under a CSS transform are not composited at all and paint white,
and frame documents are opaque (a white canvas) rather than transparent
(TD-177). Only `render` in a split run loads frames: dump commands,
`hit-test`, `--single-process` and `reftest` show frames white or empty,
frames render once (no `load` event, no script-driven navigation of a
frame) and there is no `contentWindow`, `contentDocument`, `parent` or
`postMessage` (TD-178). `window.open` and opener groups remain (ADR 0057
§3, TD-130).

## Revisit

When the M8 compositor or GPU process exists (frames become surfaces it
composites, transforms included), when same-site frames should share an
agent cluster for scripting, and when a long-lived shell keeps frames
alive across script turns.
