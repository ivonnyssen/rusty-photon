@wip
Feature: Target progress derivation (P1 planned)
  Progress is computed on demand from goals plus on-disk frames, never
  stored (rp.md § Target Store → Progress derivation): `get_target` and
  `get_session_progress` report, per target, a list of `{filter,
  binning, exposure, good, total, desired}` — one entry per
  `AcquisitionGoal` — superseding the filter-only `{completed, goal}`
  shape the config-array planner uses today (see planner.feature),
  which cannot distinguish two goals that share a filter (e.g. Ha at
  two different exposure lengths). *(Planned, P1 — not yet
  implemented; scenarios are tagged @wip.)*

  These scenarios are scoped to targets with no captured frames yet
  (`good`/`total` both 0): actual on-disk good-vs-rejected frame
  counting needs the grading plugin's sidecar section shape, which is
  explicitly deferred past P1 (rp-targets.md § MVP scope) and so isn't
  scaffolded here.

  Scenario: A target with no captured frames reports zero progress against every goal
    Given rp is running with a target store and filter roster "Luminance, Red"
    And an MCP client connected to rp
    And the MCP client has added a target named "Fresh Frame" at ra_hours 5.0 dec_degrees 10.0
    And the MCP client has set its goals to:
      | filter    | binning | exposure | desired_count |
      | Luminance | 1x1     | 300s     | 40            |
      | Red       | 1x1     | 300s     | 20            |
    When the MCP client calls "get_target" for slug "fresh-frame"
    Then the tool call should succeed
    And the reported progress should be exactly:
      | filter    | binning | exposure | good | total | desired |
      | Luminance | 1x1     | 300s     | 0    | 0     | 40      |
      | Red       | 1x1     | 300s     | 0    | 0     | 20      |

  Scenario: get_session_progress reports every target's per-goal progress
    Given rp is running with a target store and filter roster "Luminance, Red"
    And an MCP client connected to rp
    And the MCP client has added a target named "First Frame" at ra_hours 5.0 dec_degrees 10.0
    And the MCP client has set its goals to:
      | filter    | binning | exposure | desired_count |
      | Luminance | 1x1     | 300s     | 40            |
    And the MCP client has added a target named "Second Frame" at ra_hours 6.0 dec_degrees 12.0
    And the MCP client has set its goals to:
      | filter | binning | exposure | desired_count |
      | Red    | 1x1     | 300s     | 20            |
    When the MCP client calls "get_session_progress"
    Then the tool call should succeed
    And the progress for target "first-frame" should be exactly:
      | filter    | binning | exposure | good | total | desired |
      | Luminance | 1x1     | 300s     | 0    | 0     | 40      |
    And the progress for target "second-frame" should be exactly:
      | filter | binning | exposure | good | total | desired |
      | Red    | 1x1     | 300s     | 0    | 0     | 20      |
