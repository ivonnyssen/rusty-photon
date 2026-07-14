Feature: rp configuration page
  The BFF serves rp's own configuration at /config/rp over rp's plain-REST
  config API (GET /api/config, GET /api/config/schema, PUT /api/config) —
  the same schema-driven form machinery as any driver, through a REST
  transport instead of ASCOM actions. rp has no in-process reload
  (ApplyDisposition::Restart), so a successful apply persists to rp's config
  file, reports the changed paths as restart-required, and the page renders
  the restart callout instead of the reconnect poll. rp's equipment arrays
  are arrays of objects and skipped by the schema walker (they round-trip
  through the hidden blob and are edited on the equipment page instead —
  where a camera entry's integer-enum cooler_targets_c array renders as a
  checkbox grid), and rp's optional blocks (site, guider, plate_solver,
  planner) blob-round-trip the same way under the standard composite-skip
  rule — the form edits rp's scalar leaves (session, safety, imaging,
  centering, cooling, server).

  Scenario: The rp config page renders rp's effective configuration
    Given a running rp orchestrator with an empty roster
    And a BFF pointed at rp
    When I open the config page for "rp"
    Then the page shows an input named "session.file_naming_pattern" with value "{target}_{filter}_{duration}s_{sequence:04}"
    And the input named "server.port" is disabled

  Scenario: Applying a change persists to rp's config file and renders the restart callout
    Given a running rp orchestrator with an empty roster
    And a BFF pointed at rp
    When I open the config page for "rp"
    And I submit the rp form with "session.file_naming_pattern" set to "{target}_{sequence:05}"
    Then the page reports the changes take effect when rp is restarted
    And the restart callout lists "session.file_naming_pattern"
    And rp's config file on disk contains the string "{sequence:05}"

  Scenario: A value rp cannot parse is rejected and rp's config file is untouched
    Given a running rp orchestrator with an empty roster
    And a BFF pointed at rp
    When I open the config page for "rp"
    And I submit the rp form with "safety.poll_interval" set to "never"
    Then the page shows an error banner mentioning "invalid config JSON"
    And rp's config file on disk does not contain the string "never"

  Scenario: An unreachable rp renders an error banner with a retry
    Given a BFF pointed at an unreachable rp
    When I open the config page for "rp"
    Then the page shows an error banner mentioning "could not reach"
