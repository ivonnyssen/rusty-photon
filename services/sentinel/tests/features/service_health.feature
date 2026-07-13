Feature: Service health supervision
  Sentinel supervises the health of HTTP services. A services entry with a
  health block is probed with GET at its configured poll_interval; only a
  clean 200 counts as alive, and any other status, timeout, or connection
  error counts as a failed probe. After failure_threshold consecutive
  failures sentinel runs the service's restart_command autonomously, then
  backs off (doubling from restart_backoff up to restart_backoff_max)
  before any further attempt, probing at the same cadence throughout and
  never giving up. A successful probe resets the outage. Autonomous
  restarts notify and are recorded in history; the first restart of an
  outage notifies at normal priority and every later one escalates with a
  message that says the service is still unhealthy. All restart paths
  share one in-flight slot per service, so an autonomous restart never
  races a manual one. GET /api/services reports each supervised service's
  health, counters, and next scheduled restart.

  Scenario: A healthy service is probed and never restarted
    Given a stub service whose health endpoint answers 200
    And sentinel is running with service "plate-solver" supervised at the stub with a restart command that appends to a marker file
    When the dashboard reports service "plate-solver" health "up"
    Then the restart marker file does not exist after a settle period

  Scenario: Consecutive failed probes trigger an autonomous restart
    Given a stub service whose health endpoint answers 503
    And sentinel is running with notifiers and service "plate-solver" supervised at the stub with a restart command that appends to a marker file
    Then the restart marker file records at least 1 restart within 10 seconds
    And the dashboard reports service "plate-solver" health "down"
    And the notification history records an autonomous restart of "plate-solver"

  Scenario: A restart that cures the service ends the outage
    Given a stub service whose health endpoint answers 503
    And sentinel is running with service "plate-solver" supervised at the stub with a restart command that appends to a marker file
    When the restart marker file records at least 1 restart within 10 seconds
    And the stub service becomes healthy
    Then the dashboard reports service "plate-solver" health "up"
    And the dashboard reports zero restarts in the current outage for "plate-solver"

  Scenario: Restarts that never cure the service back off and escalate
    Given a stub service whose health endpoint answers 503
    And sentinel is running with notifiers and service "plate-solver" supervised at the stub with a restart command that appends to a marker file
    Then the restart marker file records at least 2 restarts within 15 seconds
    And the notification history records an escalated still-unhealthy notification for "plate-solver"
    And the dashboard reports a scheduled next restart for "plate-solver"

  Scenario: Supervised service health is listed on the services API
    Given a stub service whose health endpoint answers 200
    And sentinel is running with service "plate-solver" supervised at the stub with a restart command that appends to a marker file
    When the dashboard reports service "plate-solver" health "up"
    And the services endpoint is requested
    Then the response status should be 200
    And the services response lists "plate-solver" with health "up"
