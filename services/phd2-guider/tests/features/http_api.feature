Feature: Guider HTTP service contract

  phd2-guider serve is the rp-managed guider service: a narrow HTTP
  API in front of PHD2's JSON-RPC interface. rp's guider MCP tools
  proxy to these endpoints, and this feature is the canonical contract
  they are written against.

  Guiding and dithering requests block until PHD2 reports the star
  settled and return the rolling guiding RMS in guide-camera pixels
  (fields carry the _px suffix). Stopping blocks until PHD2 confirms
  the Stopped state. Errors use the structured envelope shared with
  the plate-solver service: an error code, a human-readable message,
  and optional details.

  The mock PHD2 emits two guide steps with RADistanceRaw 0.3 and -0.3
  and DECDistanceRaw -0.4 and 0.4, so a settled response always
  reports rms_ra_px 0.3, rms_dec_px 0.4, and total_rms_px 0.5.

  Scenario: Starting guiding blocks until PHD2 settles and reports the guiding RMS
    Given a mock PHD2 that settles successfully
    And the guider service is running
    When the client starts guiding
    Then the response status should be 200
    And the response field "state" should be "guiding"
    And the response field "rms_ra_px" should be 0.3
    And the response field "rms_dec_px" should be 0.4
    And the response field "total_rms_px" should be 0.5
    And the response field "sample_count" should be 2

  Scenario: Starting guiding without a settle override forwards the configured defaults to PHD2
    Given a mock PHD2 that settles successfully
    And the guider service is running
    When the client starts guiding
    Then the mock PHD2 should have received a "guide" request with settle pixels 0.5, time 10, and timeout 60
    And the mock PHD2 guide request should not ask for recalibration

  Scenario: A per-request settle override wins over the configured defaults
    Given a mock PHD2 that settles successfully
    And the guider service is running
    When the client starts guiding with settle pixels 2.0, time "5s", and timeout "30s"
    Then the response status should be 200
    And the mock PHD2 should have received a "guide" request with settle pixels 2.0, time 5, and timeout 30

  Scenario: A failed settle surfaces PHD2's error text as guide_failed
    Given a mock PHD2 that fails to settle
    And the guider service is running
    When the client starts guiding
    Then the response status should be 422
    And the response error should be "guide_failed" mentioning "Mock star lost"

  Scenario: A PHD2 that never settles trips the wall-clock backstop
    Given a mock PHD2 that never settles
    And the guider service is running
    When the client starts guiding with settle pixels 1.0, time "1s", and timeout "1s"
    Then the response status should be 504
    And the response error should be "settle_timeout"

  Scenario: Stopping guiding blocks until PHD2 confirms the stopped state
    Given a mock PHD2 that settles successfully
    And the guider service is running
    When the client starts guiding
    And the client stops guiding
    Then the response status should be 200
    And the response field "state" should be "stopped"
    And the mock PHD2 should have received a "stop_capture" request

  Scenario: Stopping while already stopped succeeds without sending stop_capture
    Given a mock PHD2 that settles successfully
    And the guider service is running
    When the client stops guiding
    Then the response status should be 200
    And the response field "state" should be "stopped"
    And the mock PHD2 should not have received a "stop_capture" request

  Scenario: A PHD2 that never reaches Stopped trips the stop timeout
    Given a mock PHD2 that ignores stop requests
    And the guider service is running with a stop timeout of "1s"
    When the client starts guiding
    And the client stops guiding
    Then the response status should be 504
    And the response error should be "stop_timeout"

  Scenario: Dithering forwards the offset and settle parameters to PHD2
    Given a mock PHD2 that settles successfully
    And the guider service is running
    When the client starts guiding
    And the client dithers by 5.0 pixels RA-only with settle pixels 1.5, time "8s", and timeout "40s"
    Then the response status should be 200
    And the response field "total_rms_px" should be 0.5
    And the mock PHD2 should have received a dither request with amount 5.0, raOnly true, settle pixels 1.5, time 8, and timeout 40

  Scenario: Dithering is rejected while PHD2 is not guiding
    Given a mock PHD2 that settles successfully
    And the guider service is running
    When the client dithers by 5.0 pixels
    Then the response status should be 409
    And the response error should be "not_guiding"
    And the mock PHD2 should not have received a "dither" request

  Scenario: A dither offset must be a positive pixel count
    Given a mock PHD2 that settles successfully
    And the guider service is running
    When the client dithers by -1.0 pixels
    Then the response status should be 400
    And the response error should be "invalid_request"
    And the mock PHD2 should not have received a "dither" request

  Scenario: Pause and resume forward to PHD2's set_paused
    Given a mock PHD2 that settles successfully
    And the guider service is running
    When the client pauses guiding fully
    Then the response status should be 200
    And the response field "state" should be "paused"
    And the mock PHD2 should have received a full pause request
    When the client resumes guiding
    Then the response status should be 200
    And the response field "state" should be "resumed"
    And the mock PHD2 should have received an unpause request

  Scenario: Guiding stats report the application state and the rolling RMS window
    Given a mock PHD2 that settles successfully
    And the guider service is running
    When the client starts guiding
    And the client requests the guiding stats
    Then the response status should be 200
    And the response field "app_state" should be "Guiding"
    And the response field "guiding" should be true
    And the response field "rms_ra_px" should be 0.3
    And the response field "snr" should be 25.1
    And the response field "sample_count" should be 2

  Scenario: Health reports ok while PHD2 is connected
    Given a mock PHD2 that settles successfully
    And the guider service is running
    When the client probes the service health
    Then the response status should be 200
    And the response field "status" should be "ok"

  Scenario: An unreachable PHD2 fails guiding requests and the health probe
    Given the guider service is running against an unreachable PHD2
    When the client starts guiding
    Then the response status should be 502
    And the response error should be "phd2_unreachable"
    When the client probes the service health
    Then the response status should be 503
    And the response field "status" should be "unavailable"
