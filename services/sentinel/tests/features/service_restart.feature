Feature: Service restart API
  Sentinel owns process restart for the observatory stack. For each entry in
  the top-level services map the dashboard exposes
  POST /api/services/{name}/restart: it runs the service's configured
  restart_command through the platform shell and, when a health_command is
  configured, polls it (exit 0 means healthy) until it succeeds or the
  service's max_restart_duration budget elapses. The command's outcome is a
  domain result reported on HTTP 200 with a JSON body carrying "status"
  ("ok" or "failed") and "recovery" ("healthy", "timeout", or "skipped");
  4xx is reserved for addressing errors: 404 for an unknown service name,
  409 for a service that is not restartable or is already restarting.

  Scenario: A restart runs the configured command and confirms recovery
    Given sentinel is running with a supervised service "dsd-fp2" whose restart command writes a marker file and whose health command succeeds
    When the restart endpoint is requested for "dsd-fp2"
    Then the response status should be 200
    And the restart response reports status "ok" and recovery "healthy"
    And the restart marker file exists

  Scenario: A restart without a health command skips recovery confirmation
    Given sentinel is running with a supervised service "dsd-fp2" whose restart command writes a marker file
    When the restart endpoint is requested for "dsd-fp2"
    Then the response status should be 200
    And the restart response reports status "ok" and recovery "skipped"
    And the restart marker file exists

  Scenario: Recovery that never confirms is reported as a timeout
    Given sentinel is running with a supervised service "dsd-fp2" whose restart command succeeds, whose health command fails, and whose restart budget is "1s"
    When the restart endpoint is requested for "dsd-fp2"
    Then the response status should be 200
    And the restart response reports status "ok" and recovery "timeout"

  Scenario: A failing restart command is a domain failure, not a transport error
    Given sentinel is running with a supervised service "dsd-fp2" whose restart command fails
    When the restart endpoint is requested for "dsd-fp2"
    Then the response status should be 200
    And the restart response reports status "failed"

  Scenario: Restarting an unknown service is not found
    Given sentinel is running with no monitors
    When the restart endpoint is requested for "unknown-service"
    Then the response status should be 404

  Scenario: A service with no restart command is not restartable
    Given sentinel is running with a supervised service "star-adventurer" that has no restart command
    When the restart endpoint is requested for "star-adventurer"
    Then the response status should be 409
