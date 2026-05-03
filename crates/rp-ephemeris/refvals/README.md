# Reference values for `rp-ephemeris`

These JSON files pin canonical alt/az and ICRS positions for known
objects at known sites and times. The matching test in
`tests/reference_values.rs` asserts that `ErfarsEphemeris` reproduces
each entry within tight tolerances (0.1″ for alt/az, ~1 s for
transit / rise / set).

Astropy uses ERFA internally, so agreement should be near-perfect.
Disagreement is a wrapping bug, not an algorithmic one — the test
points at the seam, not at the math.

## Regenerating

```sh
pip install 'astropy>=6' numpy
python3 crates/rp-ephemeris/refvals/gen.py
```

The script is run **by hand**, not in CI. Re-run after upgrading
`erfars` or `astropy`. Commit the diffed JSONs alongside the dep
bump so a reviewer can verify provenance from the diff.

The generator is deterministic for a fixed Astropy version and a
fixed input table; pin the version in the commit message.

## Why not run `gen.py` in CI?

- It would tie test pass/fail to whichever Astropy point release is
  on the runner that day.
- It hides whether the values were actually computed offline against
  the version the workspace was authored against.
- It adds a Python toolchain dependency to the Rust build.

The committed JSONs are the artefact under review. Treat them like
golden test fixtures: changes are explicit.
