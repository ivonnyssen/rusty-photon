Feature: Calibrator flat field workflow (end-to-end)
  The calibrator-flats orchestrator is a real service that connects to rp
  as an MCP client. It closes the cover, turns on the calibrator,
  iteratively finds the optimal exposure time per filter, captures flat
  frames, then turns off the calibrator and opens the cover.

  These tests start all three processes (OmniSim, rp, calibrator-flats)
  and verify the full workflow end-to-end.

  @serial
  Scenario: Calibrator-flats orchestrator captures flats and completes the session
    Given a running Alpaca simulator
    And the calibrator-flats service is configured for 2 "Luminance" flats and 2 "Red" flats
    And rp is running with a camera, filter wheel, cover calibrator, and the calibrator-flats orchestrator
    When a session is started via the REST API
    And the calibrator-flats orchestrator runs to completion
    Then the session status should be "idle"

  @serial
  Scenario: Calibrator-flats orchestrator emits exposure events for each flat
    Given a running Alpaca simulator
    And a test webhook receiver subscribed to "exposure_complete"
    And the calibrator-flats service is configured for 2 "Luminance" flats and 2 "Red" flats
    And rp is running with a camera, filter wheel, cover calibrator, webhook, and the calibrator-flats orchestrator
    When a session is started via the REST API
    And the calibrator-flats orchestrator runs to completion
    Then the test webhook receiver should have received at least 4 "exposure_complete" event
    And the session status should be "idle"
