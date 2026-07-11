@serial
Feature: Session recovery across rp restarts
  rp persists its session registry (session and workflow ids, status,
  start time) and the planner's record_exposure counters to
  session.session_state_file — atomically, on every session transition
  and after every recorded exposure. On startup rp reads the file back:
  a live session is restored — the counters return to the planner and
  the orchestrator is re-invoked with recovery reason "rp_restart" and
  the original workflow and session ids. Every transition to idle
  (manual stop, workflow completion, invocation failure) deletes the
  file, so a finished session is never resumed. A process shutdown
  deliberately keeps the file: a restart is exactly the outage the file
  exists to survive.

  Background:
    Given a running Alpaca simulator
    And rp's session state file is pinned to a fresh path

  Scenario: An rp restart mid-session re-invokes the orchestrator with recovery context
    Given a test orchestrator that waits for a stop signal
    And rp is running with a camera and filter wheel on the simulator and the test orchestrator
    When a session is started via the REST API
    Then the test orchestrator should have been invoked
    When rp is killed
    And rp is restarted after the crash
    Then the test orchestrator should have been re-invoked with recovery reason "rp_restart"
    And the recovery invocation should carry the original workflow and session ids
    And the session status should become "active"

  Scenario: Planner progress counters survive an rp restart
    Given a test orchestrator that waits for a stop signal
    And rp is configured with site latitude 51.0786 longitude -0.2944
    And rp is configured with the always-visible target "Test Field" whose exposure plan is:
      | filter | duration_secs | count |
      | Red    | 120           | 4     |
    And rp is running with a mount on the simulator
    And an MCP client connected to rp
    When a session is started via the REST API
    And the MCP client calls "record_exposure" for target "Test Field" filter "Red"
    And the MCP client calls "record_exposure" for target "Test Field" filter "Red"
    And rp is killed
    And rp is restarted after the crash
    And an MCP client connected to rp
    And the MCP client calls "get_session_progress"
    Then the tool call should succeed
    And the progress for target "Test Field" filter "Red" should be 2 of 4

  Scenario: A completed session is not resumed after a restart
    Given a test orchestrator that completes immediately
    And rp is running with a camera and filter wheel on the simulator and the test orchestrator
    When a session is started via the REST API
    Then the test orchestrator should have been invoked
    And the session status should become "idle"
    When rp is killed
    And rp is restarted after the crash
    Then the session status should be "idle"
    And the test orchestrator should have been invoked exactly 1 time

  Scenario: A manually stopped session is not resumed after a restart
    Given a test orchestrator that waits for a stop signal
    And rp is running with a camera and filter wheel on the simulator and the test orchestrator
    When a session is started via the REST API
    Then the test orchestrator should have been invoked
    When the session is stopped via the REST API
    And rp is killed
    And rp is restarted after the crash
    Then the session status should be "idle"
    And the test orchestrator should have been invoked exactly 1 time
