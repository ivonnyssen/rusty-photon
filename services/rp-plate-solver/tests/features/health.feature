@wip
Feature: GET /health — readiness probe

  The /health endpoint is the standard HTTP health-probe pattern,
  exposed for operational tooling and any future Sentinel probe path.
  It checks both runtime dependencies — the configured ASTAP binary
  and the configured database directory — match the startup-validation
  set, so "healthy at probe time" means "still capable of solving."

  The probe is intentionally cheap: two filesystem stats, no subprocess
  spawn. Frequent polling does not cost wrapper performance.

  Scenario: Health returns 200 with status ok after a clean start
    Given the wrapper is running with mock_astap as its solver
    When I GET /health
    Then the response status is 200
    And the response field "status" is "ok"

  Scenario: Health returns 503 when the configured binary is removed
    Given the wrapper is running with a temp-dir copy of mock_astap as its binary path
    When I delete the configured astap_binary_path
    And I GET /health
    Then the response status is 503

  Scenario: Health returns 503 when the configured db directory is removed
    Given the wrapper is running with a temp astap_db_directory
    When I delete the configured astap_db_directory
    And I GET /health
    Then the response status is 503
