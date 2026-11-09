# Reftests

Each `*.html` file here that does not end in `-ref.html` is a reftest in the
web-platform-tests format: it names its references with
`<link rel=match href>` or `<link rel=mismatch href>` and may allow small
differences with `<meta name=fuzzy content="maxDifference=A;totalPixels=B">`.
`cargo test -p rustic-cli --test reftests` runs every test through
`rustic reftest` at scales 1 and 2 and expects each to pass. The pages use only
what M3 paints today: block backgrounds, solid borders, paint order, overflow
clips and opacity, with no text.
