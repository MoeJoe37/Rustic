# Fuzz regressions

Minimized reproducers of fixed defects live in `<target>/` subdirectories,
named as the harness saved them (`<kind>-<fingerprint>.bin`). Every campaign
replays them first, and `tests/harness.rs` replays them on every test run. Add
the fix and the reproducer in the same change, with a note in the commit
message describing the violated contract. None exist yet: as of 2026-11-08 the
targets have not produced a failure.
