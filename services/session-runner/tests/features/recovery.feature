@serial
Feature: Resume (the re-entrancy contract)
  A session interrupted mid-run continues from the persisted blackboard
  when re-invoked with recovery context: re-execution from the root skips
  once-marked setup and picks the capture loop up at the recorded frame
  count instead of starting over.

  The safety scenario exercises rp's own recovery machinery end-to-end:
  an unsafe SafetyMonitor reading terminates the run, and the safe
  transition re-invokes the engine with recovery context. The
  startup-recovery scenario exercises rp's other recovery path the same
  way: an rp whose session state file survives the restart reads its
  persisted session registry back and re-invokes the engine by itself,
  with recovery reason "rp_restart". The engine-kill and rp-outage
  scenarios POST /invoke directly instead — they pin the engine's side
  of the recovery contract in isolation, independent of who sends the
  invocation (the engine-kill scenario never restarts rp at all, and
  the rp-outage scenario's restarted rp gets a fresh session state
  file, so rp-side recovery stays out of the way). The fixture document
  (recovery_capture_loop, tests/fixtures/workflows/) plans 4 frames of
  2s each; its progress counter lives in session.frames.

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

  Scenario: An rp restart re-invokes the engine by itself and the session completes
    Given a running Alpaca simulator
    And rp's session state file survives restarts
    And a test webhook receiver subscribed to "exposure_complete"
    And rp is running with a camera and the session-runner orchestrator running the "recovery_capture_loop" workflow
    When a session is started via the REST API
    And the blackboard records at least 2 frames
    And rp is killed
    Then the session-runner is still healthy and the blackboard is kept
    When rp is restarted
    Then the blackboard is deleted within 60 seconds
    And rp reports the session as "idle" within 10 seconds
    # Between 3 and 5 for a 4-frame plan: the SIGKILL can eat the
    # webhook delivery of an already-recorded frame (floor = plan - 1),
    # and a frame that completed in flight but was never recorded is
    # legitimately re-shot by the resumed run (ceiling = plan + 1). The
    # ceiling is the no-repeated-frames invariant.
    And the test webhook receiver should have received between 3 and 5 "exposure_complete" events

  Scenario: A safety interruption pauses the session and rp resumes it once conditions are safe
    Given a running Alpaca simulator
    And a safety monitor guards the session
    And rp is running with a camera and the session-runner orchestrator running the "recovery_capture_loop" workflow
    And an SSE client is watching rp's event stream
    When a session is started via the REST API
    And the blackboard records at least 2 frames
    And the safety monitor reports unsafe
    Then rp reports the session as "interrupted" within 5 seconds
    And the blackboard is kept
    When the safety monitor reports safe again
    Then rp reports the session as "active" within 5 seconds
    And the blackboard is deleted within 60 seconds
    And the SSE stream should show between 4 and 5 "exposure_complete" events
    And the SSE stream should show exactly 1 "filter_switch" event
    And the SSE stream should show exactly 2 "safety_changed" events
