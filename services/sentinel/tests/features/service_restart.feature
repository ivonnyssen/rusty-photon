Feature: Service restart API
  Sentinel owns process restart for the observatory stack. For each
  discovered service the dashboard exposes
  POST /api/services/{name}/restart, where {name} is the service's
  discovered name (the unit name minus the rusty-photon- prefix): it runs
  the restart command derived from the unit name and polls the derived
  recovery check (systemctl is-active on Linux) until it passes or the
  restart budget elapses. The command's outcome is a domain result reported
  on HTTP 200 with a JSON body carrying "status" ("ok" or "failed") and
  "recovery" ("healthy", "timeout", or "skipped"); 4xx is reserved for
  addressing errors: 404 for a name no discovered service carries, 409 for
  a restart already in flight. The manual restart is the operator's
  recovery hammer, so it also works for services whose run state autonomous
  supervision leaves alone (stopped, inert, disabled).

  Scenario: A restart runs the derived command and confirms recovery
    Given a discovered unit "rusty-photon-dsd-fp2" in state "running"
    And sentinel is running with no monitors
    When the restart endpoint is requested for "dsd-fp2"
    Then the response status should be 200
    And the restart response reports status "ok" and recovery "healthy"
    And the service manager records a restart of "rusty-photon-dsd-fp2"

  Scenario: Recovery that never confirms is reported as a timeout
    Given a discovered unit "rusty-photon-dsd-fp2" in state "stopped"
    And the service manager leaves restarted units in their prior state
    And sentinel is running with no monitors
    When the restart endpoint is requested for "dsd-fp2"
    Then the response status should be 200
    And the restart response reports status "ok" and recovery "timeout"

  Scenario: A failing restart command is a domain failure, not a transport error
    Given a discovered unit "rusty-photon-dsd-fp2" in state "running"
    And the service manager fails restarts of "rusty-photon-dsd-fp2"
    And sentinel is running with no monitors
    When the restart endpoint is requested for "dsd-fp2"
    Then the response status should be 200
    And the restart response reports status "failed"

  Scenario: Restarting an unknown service is not found
    Given sentinel is running with no monitors
    When the restart endpoint is requested for "unknown-service"
    Then the response status should be 404

  Scenario: A stopped service can still be restarted manually
    Given a discovered unit "rusty-photon-dsd-fp2" in state "stopped"
    And sentinel is running with no monitors
    When the restart endpoint is requested for "dsd-fp2"
    Then the response status should be 200
    And the restart response reports status "ok" and recovery "healthy"
    And the service manager records a restart of "rusty-photon-dsd-fp2"
