@serial
Feature: Planner convenience tools
  rp exposes three convenience MCP tools — `get_target_status`,
  `get_next_target`, `get_meridian_status` — that compose the
  primitives from `ephemeris_primitives.feature` plus the embedded
  catalog. v1 implements §"Dynamic Planner" decision-logic bullets
  1, 2, and 6 (altitude / set-time elimination, prefer transiting,
  twilight + end-of-session fallback). Per-target progress and
  filter-change minimisation are deferred until session-state
  plumbing is wired through.

  The reason discriminant for `get_next_target` is a structured
  string (`best_transiting_candidate`, `no_targets_configured`,
  `all_below_min_altitude`, `wait_for_twilight`, `end_of_session`)
  so a planner plugin can branch without parsing free-form text.

  Scenario: Tool catalog includes the convenience tools
    Given a running Alpaca simulator
    And rp is configured with site latitude 51.0786 longitude -0.2944
    And rp is running with a mount on the simulator
    And an MCP client connected to rp
    When the MCP client lists available tools
    Then the tool list should include "get_target_status"
    And the tool list should include "get_next_target"
    And the tool list should include "get_meridian_status"

  Scenario: get_target_status accepts a catalog name
    Given a running Alpaca simulator
    And rp is configured with site latitude 51.0786 longitude -0.2944
    And rp is running with a mount on the simulator
    And an MCP client connected to rp
    When the MCP client calls "get_target_status" for target "M 31"
    Then the tool call should succeed
    And the result target_name should be "M 31"
    And the result altitude_degrees should be a finite number

  Scenario: get_next_target with no targets configured returns the structured branch
    Given a running Alpaca simulator
    And rp is configured with site latitude 51.0786 longitude -0.2944
    And rp is running with a mount on the simulator
    And an MCP client connected to rp
    When the MCP client calls "get_next_target"
    Then the tool call should succeed
    And the result reason should be "no_targets_configured"
    And the result target should be null

  Scenario: get_target_status fails when site is missing
    Given a running Alpaca simulator
    And rp is running with a mount on the simulator
    And an MCP client connected to rp
    When the MCP client calls "get_target_status" for target "M 31"
    Then the tool call should fail
    And the tool error message should mention "site not configured"

  Scenario: get_target_status fails for an unknown name
    Given a running Alpaca simulator
    And rp is configured with site latitude 51.0786 longitude -0.2944
    And rp is running with a mount on the simulator
    And an MCP client connected to rp
    When the MCP client calls "get_target_status" for target "M 999"
    Then the tool call should fail
    And the tool error payload should have field "error" equal to "target_not_found"
