Feature: Session lifecycle with flat calibration orchestrator
  A session is started via the REST API. rp invokes the configured
  orchestrator plugin, which connects to rp's MCP server and drives
  the workflow using tool calls. For flat calibration, the orchestrator
  captures a set of flat frames per filter and then completes.

  Scenario: Session starts and invokes the orchestrator
    Given a running Alpaca simulator
    And a test orchestrator that completes immediately
    And rp is running with a camera and filter wheel on the simulator and the test orchestrator
    When a session is started via the REST API
    Then the test orchestrator should have been invoked
    And the invocation payload should contain a session id
    And the invocation payload should contain the MCP server URL

  Scenario: Session emits started event
    Given a running Alpaca simulator
    And a test orchestrator that completes immediately
    And a test webhook receiver subscribed to "session_started"
    And rp is running with equipment and both plugins configured
    When a session is started via the REST API
    Then the test webhook receiver should receive a "session_started" event

  Scenario: Session emits stopped event when orchestrator completes
    Given a running Alpaca simulator
    And a test orchestrator that completes immediately
    And a test webhook receiver subscribed to "session_stopped"
    And rp is running with equipment and both plugins configured
    When a session is started via the REST API
    And the test orchestrator posts completion to rp
    Then the test webhook receiver should receive a "session_stopped" event

  Scenario: Session status reports active while orchestrator is running
    Given a running Alpaca simulator
    And a test orchestrator that waits for a stop signal
    And rp is running with a camera and filter wheel on the simulator and the test orchestrator
    When a session is started via the REST API
    Then the session status should be "active"

  Scenario: Session status reports idle after orchestrator completes
    Given a running Alpaca simulator
    And a test orchestrator that completes immediately
    And rp is running with a camera and filter wheel on the simulator and the test orchestrator
    When a session is started via the REST API
    And the test orchestrator posts completion to rp
    Then the session status should be "idle"

  Scenario: Session stop cancels the orchestrator gracefully
    Given a running Alpaca simulator
    And a test orchestrator that waits for a stop signal
    And rp is running with a camera and filter wheel on the simulator and the test orchestrator
    When a session is started via the REST API
    And the session is stopped via the REST API
    Then the test orchestrator should have been cancelled
    And the session status should be "idle"

  @serial
  Scenario: Flat calibration orchestrator captures flats across filters
    Given a running Alpaca simulator
    And a test flat-calibration orchestrator configured for 2 "Luminance" flats and 2 "Red" flats
    And a test webhook receiver subscribed to "exposure_complete" and "filter_switch"
    And rp is running with equipment and both plugins configured
    When a session is started via the REST API
    And the test orchestrator runs to completion
    Then the test webhook receiver should have received 4 "exposure_complete" events
    And the test webhook receiver should have received at least 1 "filter_switch" event
    And the session status should be "idle"

  Scenario: Starting a session while one is active fails
    Given a running Alpaca simulator
    And a test orchestrator that waits for a stop signal
    And rp is running with a camera and filter wheel on the simulator and the test orchestrator
    When a session is started via the REST API
    And I try to start another session via the REST API
    Then the second session start should fail with an error

  Scenario: Session status is idle before any session starts
    Given a running Alpaca simulator
    And rp is running with a camera and filter wheel on the simulator
    Then the session status should be "idle"

  Scenario: Stopping a session when idle succeeds
    Given a running Alpaca simulator
    And rp is running with a camera and filter wheel on the simulator
    When the session is stopped via the REST API
    Then the session status should be "idle"

  Scenario: Workflow complete with unknown workflow id is ignored
    Given a running Alpaca simulator
    And a test orchestrator that waits for a stop signal
    And rp is running with a camera and filter wheel on the simulator and the test orchestrator
    When a session is started via the REST API
    And a workflow completion is posted with an unknown workflow id
    Then the session status should be "active"

  Scenario: Session with unreachable orchestrator still starts
    Given a running Alpaca simulator
    And a plugin configured as orchestrator with invoke URL "http://localhost:1/invoke"
    And rp is running with a camera and filter wheel on the simulator and the test orchestrator
    When a session is started via the REST API
    Then the session status should be "active"

  Scenario: Session can be restarted after completion
    Given a running Alpaca simulator
    And a test orchestrator that waits for a stop signal
    And rp is running with a camera and filter wheel on the simulator and the test orchestrator
    When a session is started via the REST API
    And the test orchestrator posts completion to rp
    Then the session status should be "idle"
    When a session is started via the REST API
    Then the session status should be "active"

  Scenario: Session can be restarted after manual stop
    Given a running Alpaca simulator
    And a test orchestrator that waits for a stop signal
    And rp is running with a camera and filter wheel on the simulator and the test orchestrator
    When a session is started via the REST API
    And the session is stopped via the REST API
    Then the session status should be "idle"
    When a session is started via the REST API
    Then the session status should be "active"
