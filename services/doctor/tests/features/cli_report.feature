Feature: Doctor CLI and report contract
  rusty-photon-doctor diagnoses a config directory and the host's service
  manager, read-only, and reports what it finds. The default output is a
  human-readable report; --json emits the DoctorReport schema instead
  (schema_version 1). The exit code is the machine summary: 0 means the
  diagnosis ran and found no failing check (warnings allowed), 1 means at
  least one check failed, 2 means doctor itself could not run. Passing
  checks are included in the JSON report — an empty report must never be
  mistaken for a clean one.

  Scenario: A coherent packaged install exits 0
    Given a config directory with a valid "qhy-focuser.json" on port 11113
    And platform facts with an enabled unit "rusty-photon-qhy-focuser"
    When I run doctor with --json
    Then doctor exits with code 0
    And the report field "schema_version" is 1
    And the report field "mode" is "packaged"
    And the report has no "fail" checks
    And the report contains an "ok" check named "config.server-shape" for service "qhy-focuser"

  Scenario: A failing check exits 1
    Given a config directory where "qhy-focuser.json" is not valid JSON
    And platform facts with an enabled unit "rusty-photon-qhy-focuser"
    When I run doctor with --json
    Then doctor exits with code 1
    And the report contains a "fail" check named "config.json-syntax" for service "qhy-focuser"

  Scenario: An unresolvable config directory exits 2
    When I run doctor pointed at a config directory that does not exist
    Then doctor exits with code 2

  Scenario: Warnings alone still exit 0
    Given a config directory with a valid "qhy-focuser.json" on port 11113
    And platform facts with enabled units:
      | unit                          |
      | rusty-photon-qhy-focuser      |
      | rusty-photon-dsd-fp2          |
    When I run doctor with --json
    Then doctor exits with code 0
    And the report contains a "warn" check named "inventory.unit-without-config" for service "dsd-fp2"

  Scenario: The human-readable report summarizes counts and prints failures in full
    Given a config directory where "qhy-focuser.json" is not valid JSON
    And platform facts with an enabled unit "rusty-photon-qhy-focuser"
    When I run doctor without --json
    Then doctor exits with code 1
    And the text output contains "config.json-syntax"
    And the text output contains a summary line with the ok, warn, and fail counts

  Scenario: A host with no rusty-photon units is diagnosed config-only
    Given a config directory with a valid "qhy-focuser.json" on port 11113
    And platform facts with no rusty-photon units
    When I run doctor with --json
    Then doctor exits with code 0
    And the report field "mode" is "config-only"
    And the report has no checks named "inventory.unit-without-config"
