@requires-astap
Feature: Real-ASTAP smoke (mock-honesty backstop)

  Two scenarios exercise the wrapper against the operator's real ASTAP
  install. Gated by the `@requires-astap` tag — they fire only when
  `ASTAP_BINARY` is set in the environment. The dedicated nightly
  cross-platform workflow sets that var via the `install-astap` action;
  PR jobs do not, so these scenarios are skipped on every PR.

  See `docs/plans/rp-plate-solver.md` §"Real-ASTAP coverage: cadence
  and gating" for the rationale and the bdd.rs filter snippet.

  Note: the `@wip` tag is removed by Phase 4 along with all other
  feature files'. The `@requires-astap` tag stays — it's the permanent
  cadence gate.

  Scenario: Real ASTAP solves the m31 fixture within tolerance
    Given the wrapper is running with the real ASTAP_BINARY as its solver
    And the m31_known.fits fixture is on disk
    When I POST to /api/v1/solve with that fits_path
    Then the response status is 200
    And the response field "ra_center" is approximately 10.6848 within 0.01 degrees
    And the response field "dec_center" is approximately 41.2690 within 0.01 degrees
    And the response field "solver" contains "astap" case-insensitively

  Scenario: Real ASTAP returns solve_failed on a degenerate FITS
    Given the wrapper is running with the real ASTAP_BINARY as its solver
    And the degenerate_no_stars.fits fixture is on disk
    When I POST to /api/v1/solve with that fits_path
    Then the response status is 422
    And the response field "error" is "solve_failed"
