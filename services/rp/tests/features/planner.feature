@serial
Feature: Planner convenience tools
  rp exposes the planner MCP tools — `get_target_status`,
  `get_next_target`, `get_meridian_status`, `record_exposure`,
  `get_session_progress` — that compose the primitives from
  `ephemeris_primitives.feature`, the embedded catalog, and the
  per-target/per-filter progress counters. v1 implements §"Dynamic
  Planner" decision-logic bullets 1 (altitude half), 2, 3, 4, and 6
  (eliminate below-floor and goal-met targets, prefer transiting,
  break near-transit ties by least progress then filter batching,
  twilight / end-of-session fallback).

  The reason discriminant for `get_next_target` is a structured
  string (`best_transiting_candidate`, `no_targets_configured`,
  `all_below_min_altitude`, `wait_for_twilight`, `end_of_session`)
  so a planner plugin can branch without parsing free-form text.

  A recommendation also carries the target's exposure plan: `filter`
  and `duration_secs` are the first entry of the target's
  `exposures[]` config whose `count` the `record_exposure` counters
  have not yet met, or null when the target defines no plan — the
  orchestrator then falls back to its own exposure parameters.

  Scenario: Tool catalog includes the convenience tools
    Given a running Alpaca simulator
    And rp is configured with site latitude 51.0786 longitude -0.2944
    And rp is running with a mount on the simulator
    And an MCP client connected to rp
    When the MCP client lists available tools
    Then the tool list should include "get_target_status"
    And the tool list should include "get_next_target"
    And the tool list should include "get_meridian_status"
    And the tool list should include "record_exposure"
    And the tool list should include "get_session_progress"

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

  Scenario: get_next_target returns the first exposure-plan entry for the recommended target
    Given a running Alpaca simulator
    And rp is configured with site latitude 51.0786 longitude -0.2944
    And rp is configured with the always-visible target "Test Field" whose exposure plan is:
      | filter | duration_secs |
      | Red    | 120           |
      | Blue   | 60            |
    And rp is running with a mount on the simulator
    And an MCP client connected to rp
    When the MCP client calls "get_next_target"
    Then the tool call should succeed
    And the result reason should be "best_transiting_candidate"
    And the result filter should be "Red"
    And the result duration_secs should be 120

  Scenario: get_next_target leaves the exposure plan null for a target without one
    Given a running Alpaca simulator
    And rp is configured with site latitude 51.0786 longitude -0.2944
    And rp is configured with the always-visible target "Bare Field" and no exposure plan
    And rp is running with a mount on the simulator
    And an MCP client connected to rp
    When the MCP client calls "get_next_target"
    Then the tool call should succeed
    And the result reason should be "best_transiting_candidate"
    And the result filter should be null
    And the result duration_secs should be null

  # The dusk/dawn pair pins bullet 6's sky gating end-to-end against
  # the real ephemeris: a never-visible target (floor 90 degrees, which
  # no computed altitude reaches) forces the no-survivors branch, and
  # the explicit evaluation time puts the Sun in evening vs morning
  # twilight at the configured site. 2026-03-20 is the March equinox:
  # at this longitude the Sun sits near -10 degrees and descending at
  # 19:20 UTC, near -10 degrees and climbing at 05:00 UTC.
  Scenario: get_next_target in evening twilight says wait_for_twilight
    Given a running Alpaca simulator
    And rp is configured with site latitude 51.0786 longitude -0.2944
    And rp is configured with the never-visible target "Below Floor"
    And rp is running with a mount on the simulator
    And an MCP client connected to rp
    When the MCP client calls "get_next_target" at time "2026-03-20T19:20:00Z"
    Then the tool call should succeed
    And the result reason should be "wait_for_twilight"
    And the result target should be null

  Scenario: get_next_target in morning twilight says end_of_session
    Given a running Alpaca simulator
    And rp is configured with site latitude 51.0786 longitude -0.2944
    And rp is configured with the never-visible target "Below Floor"
    And rp is running with a mount on the simulator
    And an MCP client connected to rp
    When the MCP client calls "get_next_target" at time "2026-03-20T05:00:00Z"
    Then the tool call should succeed
    And the result reason should be "end_of_session"
    And the result target should be null

  # The progress scenarios drive the record_exposure counters through
  # the MCP surface end-to-end: an always-visible target (floor -90)
  # keeps the recommendation deterministic at any wall-clock, and
  # counted plan entries give the planner finite integration goals.

  Scenario: record_exposure reports the per-filter counter and its goal
    Given a running Alpaca simulator
    And rp is configured with site latitude 51.0786 longitude -0.2944
    And rp is configured with the always-visible target "Test Field" whose exposure plan is:
      | filter | duration_secs | count |
      | Red    | 120           | 2     |
    And rp is running with a mount on the simulator
    And an MCP client connected to rp
    When the MCP client calls "record_exposure" for target "Test Field" filter "Red"
    Then the tool call should succeed
    And the result completed should be 1 with a goal of 2

  Scenario: A met integration goal rotates the recommendation to the next plan entry
    Given a running Alpaca simulator
    And rp is configured with site latitude 51.0786 longitude -0.2944
    And rp is configured with the always-visible target "Test Field" whose exposure plan is:
      | filter | duration_secs | count |
      | Red    | 120           | 1     |
      | Blue   | 60            | 1     |
    And rp is running with a mount on the simulator
    And an MCP client connected to rp
    When the MCP client calls "record_exposure" for target "Test Field" filter "Red"
    And the MCP client calls "get_next_target"
    Then the tool call should succeed
    And the result reason should be "best_transiting_candidate"
    And the result filter should be "Blue"
    And the result duration_secs should be 60

  Scenario: Exhausting every integration goal ends the session
    Given a running Alpaca simulator
    And rp is configured with site latitude 51.0786 longitude -0.2944
    And rp is configured with the always-visible target "Test Field" whose exposure plan is:
      | filter | duration_secs | count |
      | Red    | 120           | 1     |
    And rp is running with a mount on the simulator
    And an MCP client connected to rp
    When the MCP client calls "record_exposure" for target "Test Field" filter "Red"
    And the MCP client calls "get_next_target"
    Then the tool call should succeed
    And the result reason should be "end_of_session"
    And the result target should be null

  Scenario: get_session_progress reports every configured target's counters
    Given a running Alpaca simulator
    And rp is configured with site latitude 51.0786 longitude -0.2944
    And rp is configured with the always-visible target "Test Field" whose exposure plan is:
      | filter | duration_secs | count |
      | Red    | 120           | 2     |
      | Blue   | 60            | 1     |
    And rp is running with a mount on the simulator
    And an MCP client connected to rp
    When the MCP client calls "record_exposure" for target "Test Field" filter "Red"
    And the MCP client calls "get_session_progress"
    Then the tool call should succeed
    And the progress for target "Test Field" filter "Red" should be 1 of 2
    And the progress for target "Test Field" filter "Blue" should be 0 of 1

  Scenario: The planner balances equally transiting targets by progress
    Given a running Alpaca simulator
    And rp is configured with site latitude 51.0786 longitude -0.2944
    And rp is configured with the always-visible targets "First Field" and "Second Field", each wanting 2 unfiltered 2-second frames
    And rp is running with a mount on the simulator
    And an MCP client connected to rp
    # Identical coordinates mean an exact transit tie; without the
    # recorded frame, config order would recommend "First Field".
    When the MCP client calls "record_exposure" for target "First Field" with no filter
    And the MCP client calls "get_next_target"
    Then the tool call should succeed
    And the recommended target should be "Second Field"

  Scenario: record_exposure rejects a target that is not configured
    Given a running Alpaca simulator
    And rp is configured with site latitude 51.0786 longitude -0.2944
    And rp is configured with the always-visible target "Test Field" and no exposure plan
    And rp is running with a mount on the simulator
    And an MCP client connected to rp
    When the MCP client calls "record_exposure" for target "Not Configured" filter "Red"
    Then the tool call should fail
    And the tool error message should mention "unknown target"

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
