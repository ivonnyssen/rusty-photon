Feature: Calibrator flat calibration workflow
  The calibrator-flats orchestrator closes the cover, turns on the calibrator,
  iteratively finds the optimal exposure time per filter to achieve ~50%
  of the camera's well depth, captures the requested number of flat frames,
  then turns off the calibrator and opens the cover.

  @serial
  Scenario: Orchestrator captures flats and reports completion
    Given a running Alpaca simulator
    And rp is running with a camera, filter wheel, and cover calibrator on the simulator
    And the calibrator-flats orchestrator is configured for 2 "Luminance" flats and 2 "Red" flats
    And the calibrator-flats orchestrator is registered with rp
    When a session is started via the REST API
    And the calibrator-flats orchestrator runs to completion
    Then the session status should be "idle"

  @serial
  Scenario: Orchestrator produces correct number of exposure events
    Given a running Alpaca simulator
    And rp is running with a camera, filter wheel, and cover calibrator on the simulator
    And a test webhook receiver subscribed to "exposure_complete"
    And the calibrator-flats orchestrator is configured for 2 "Luminance" flats and 2 "Red" flats
    And the calibrator-flats orchestrator is registered with rp
    When a session is started via the REST API
    And the calibrator-flats orchestrator runs to completion
    Then the test webhook receiver should have received at least 4 "exposure_complete" events
