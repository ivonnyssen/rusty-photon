Feature: Unit wiring and privilege diagnosis
  Two failure modes live between the unit files and the services they start,
  both invisible until the moment they matter. A unit gated on
  ConditionPathExists= whose target file is missing is installed, enabled,
  and silently inert — systemd "starts" it without running anything. And
  sentinel's restart machinery needs a polkit rule: its unit runs as an
  unprivileged user with NoNewPrivileges=yes, so without a rule granting
  org.freedesktop.systemd1.manage-units for rusty-photon-* units, every
  restart it attempts is denied at the privilege boundary. Both facts are
  systemd-specific and the checks run only where they exist.

  Scenario: An enabled unit gated on a missing config file is inert
    Given an empty config directory
    And platform facts where enabled unit "rusty-photon-plate-solver" is gated on a missing file
    When I run doctor with --json
    Then doctor exits with code 1
    And the report contains a "fail" check named "units.config-gated" for service "plate-solver"
    And that check's detail mentions "ConditionPathExists"
    And the report contains a "warn" check named "inventory.unit-without-config" for service "plate-solver"
    And that check's suggestion mentions "hand-written"

  Scenario: A gate whose file exists is satisfied
    Given a config directory with a valid "plate-solver.json" on port 11131
    And platform facts where enabled unit "rusty-photon-plate-solver" is gated on config file "plate-solver.json"
    When I run doctor with --json
    Then the report contains an "ok" check named "units.config-gated" for service "plate-solver"

  Scenario: Sentinel without the polkit rule cannot restart anything
    Given a config directory with a valid "sentinel.json" on port 11114
    And platform facts with an enabled unit "rusty-photon-sentinel"
    And the platform facts say no polkit rule grants sentinel restarts
    When I run doctor with --json
    Then doctor exits with code 1
    And the report contains a "fail" check named "sentinel.privilege-path" for service "sentinel"
    And that check's detail mentions "polkit"

  Scenario: Sentinel with the packaged polkit rule is restart-capable
    Given a config directory with a valid "sentinel.json" on port 11114
    And platform facts with an enabled unit "rusty-photon-sentinel"
    And the platform facts say a polkit rule grants sentinel restarts
    When I run doctor with --json
    Then the report contains an "ok" check named "sentinel.privilege-path" for service "sentinel"

  Scenario: The privilege check does not run where sentinel is not installed
    Given a config directory with a valid "qhy-focuser.json" on port 11113
    And platform facts with an enabled unit "rusty-photon-qhy-focuser"
    When I run doctor with --json
    Then the report has no checks named "sentinel.privilege-path"

  Scenario: Systemd-specific checks do not run on Windows facts
    Given a config directory with a valid "sentinel.json" on port 11114
    And Windows platform facts with an enabled unit "rusty-photon-sentinel"
    When I run doctor with --json
    Then the report has no checks named "sentinel.privilege-path"
    And the report has no checks named "units.config-gated"
