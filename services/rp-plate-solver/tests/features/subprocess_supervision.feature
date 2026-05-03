Feature: Subprocess supervision (timeout escalation, single-flight queueing)

  Every solve is bounded by a wall-clock deadline. On expiry the wrapper
  signals the child gracefully (SIGTERM on Unix, CTRL_BREAK_EVENT on
  Windows), waits a fixed 2-second grace period, then force-kills
  (SIGKILL / TerminateProcess). The wrapper always waits the child
  fully before returning; no orphaned child processes.

  Overlapping requests queue behind a single-flight semaphore (default
  capacity 1). Queue wait time is not counted against the per-request
  deadline.

  Background:
    Given the wrapper is running with mock_astap as its solver

  Scenario: Hung child responding to graceful signal returns solve_timeout (terminated)
    Given mock_astap is configured for "hang" mode
    And a writable FITS path
    When I POST to /api/v1/solve with that fits_path and timeout "100ms"
    Then the response status is 504
    And the response field "error" is "solve_timeout"
    And the response time is at most 2500 milliseconds

  Scenario: Hung child ignoring graceful signal is force-killed after grace
    Given mock_astap is configured for "ignore_sigterm" mode
    And a writable FITS path
    When I POST to /api/v1/solve with that fits_path and timeout "100ms"
    Then the response status is 504
    And the response field "error" is "solve_timeout"
    And the response time is at least 2000 milliseconds
    And the response time is at most 4500 milliseconds

  Scenario: Single-flight semaphore serializes overlapping solves
    Given mock_astap is configured for "hang" mode
    And a writable FITS path
    When I POST two concurrent solve requests with timeout "100ms" each
    Then both responses have status 504
    And the second request's spawn time is after the first request's exit time
