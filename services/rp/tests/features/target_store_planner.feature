@serial @wip
Feature: Planner altitude-gating parity against the target store (P1 planned)
  Decision 9 (docs/plans/planetarium-target-import.md) is a fixed P1
  migration requirement: `get_next_target` must keep eliminating
  targets below their altitude floor once targets move from the
  config `targets[]` array (see planner.feature) to the rp-targets
  store. The floor comes from `target.scheduling.min_altitude_degrees`,
  falling back to `targets.default_scheduling.min_altitude_degrees`
  from config — the same two-level per-target-then-default fallback
  the config-array planner already applies today (rp.md § Target
  Store, § Dynamic Planner). *(Planned, P1 — not yet implemented;
  scenarios are tagged @wip.)*

  Scenario: A store-backed target below its per-target altitude floor is eliminated
    Given a running Alpaca simulator
    And rp is configured with site latitude 51.0786 longitude -0.2944
    And rp is running with a mount on the simulator
    And an MCP client connected to rp
    And the MCP client has added a target named "Below Floor" at ra_hours 0.0 dec_degrees 0.0 with min_altitude_degrees 90
    When the MCP client calls "get_next_target"
    Then the tool call should succeed
    And the result reason should be "all_below_min_altitude"

  Scenario: A store-backed target with no override falls back to the configured default floor
    Given rp is configured with a target-store default minimum altitude of 90 degrees
    And a running Alpaca simulator
    And rp is configured with site latitude 51.0786 longitude -0.2944
    And rp is running with a mount on the simulator
    And an MCP client connected to rp
    And the MCP client has added a target named "No Override" at ra_hours 0.0 dec_degrees 0.0
    When the MCP client calls "get_next_target"
    Then the tool call should succeed
    And the result reason should be "all_below_min_altitude"
