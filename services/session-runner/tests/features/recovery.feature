@serial
Feature: Resume (the re-entrancy contract)
  A session interrupted mid-run continues from the persisted blackboard
  when re-invoked with recovery context: re-execution from the root skips
  once-marked setup and picks the capture loop up at the recorded frame
  count instead of starting over.

  rp's own recovery re-invocation machinery is not implemented yet, so
  these scenarios POST /invoke directly, standing in for it. The fixture
  document (recovery_capture_loop, tests/fixtures/workflows/) plans 4
  frames of 2s each; its progress counter lives in session.frames.

  Scenario: A killed engine resumes without repeating recorded frames
    Given a running Alpaca simulator
    And rp is running with a camera and the session-runner orchestrator running the "recovery_capture_loop" workflow
    And an SSE client is watching rp's event stream
    When a session is started via the REST API
    And the blackboard records at least 2 frames
    And the session-runner is killed
    And the session-runner is restarted
    And the session is re-invoked with recovery context
    Then the session ends within 60 seconds
    And the SSE stream should show between 4 and 5 "exposure_complete" events
    And the SSE stream should show exactly 1 "filter_switch" event
    And the blackboard is deleted within 10 seconds

  Scenario: An rp outage terminates the run; the session resumes against the restarted rp
    Given a running Alpaca simulator
    And rp is running with a camera and the session-runner orchestrator running the "recovery_capture_loop" workflow
    When a session is started via the REST API
    And the blackboard records at least 2 frames
    And rp is killed
    Then the session-runner is still healthy and the blackboard is kept
    When rp is restarted
    And an SSE client is watching rp's event stream
    And the session is re-invoked with recovery context
    Then the blackboard is deleted within 60 seconds
    And the SSE stream should show only the remaining "exposure_complete" events
