@serial
Feature: Calibrator flats workflow document (end-to-end)
  session-runner is the generic workflow orchestrator: rp invokes it at
  session start, it loads the named workflow document, validates it against
  rp's live tool catalog, and drives the session with MCP tool calls.

  These scenarios execute the shipped calibrator_flats.json first-party
  document across the same three-process topology as the calibrator-flats
  service's suite (OmniSim + rp + session-runner). That Rust orchestrator
  is the behavioral oracle: the document must produce the same events, the
  same frame counts, and the same cleanup.

  Scenario: The calibrator flats document captures flats and completes the session
    Given a running Alpaca simulator
    And a flat plan of 2 "Luminance" flats and 2 "Red" flats
    And rp is running with a camera, filter wheel, cover calibrator, and the session-runner orchestrator
    When a session is started via the REST API
    And the workflow document runs to completion
    Then the session status should be "idle"

  Scenario: The calibrator flats document emits exposure events for each flat
    Given a running Alpaca simulator
    And a test webhook receiver subscribed to "exposure_complete"
    And a flat plan of 2 "Luminance" flats and 2 "Red" flats
    And rp is running with a camera, filter wheel, cover calibrator, and the session-runner orchestrator
    When a session is started via the REST API
    And the workflow document runs to completion
    Then the test webhook receiver should have received at least 4 "exposure_complete" events
    And the session status should be "idle"
