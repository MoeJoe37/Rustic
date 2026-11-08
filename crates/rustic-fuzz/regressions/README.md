# Fuzz regressions

Minimized reproducers of fixed defects live in `<target>/` subdirectories,
named as the harness saved them (`<kind>-<fingerprint>.bin`). Every campaign
replays them first, and `tests/harness.rs` replays them on every test run. Add
the fix and the reproducer in the same change, with a note in the commit
message describing the violated contract.

| Reproducer | Contract | Fix |
| --- | --- | --- |
| `css-media/violation-076bafb4550d968d.bin` | Media query serialization must be a fixed point; parentheses nested past `MAX_MEDIA_NESTING` around general-enclosed collapsed on reparse | CSS-06: parentheses around general-enclosed stay general-enclosed |
| `css-media/violation-d6d25ca1ecbf13e5.bin` | Media query serialization must be a fixed point; whitespace split by a comment inside general-enclosed doubled and then merged on reparse | CSS-06: consecutive whitespace values serialize as one space |
