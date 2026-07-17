Feature: Service health supervision
  Sentinel supervises every discovered service in the running or failed run
  state — supervision is universal and unconfigured, with all policy values
  constants (poll interval 30s, failure threshold 3, restart backoff 60s
  doubling to 900s, restart budget 300s; these scenarios tighten them
  through the test stub's policy file). A running service is probed with
  GET at its derived health URL: a 200 counts as alive, and so do 401 and
  403 — an auth-enabled service that challenges an unauthenticated probe
  has proven it is up. Any other status, a timeout, or a connection error
  counts as a failed probe. After the failure threshold sentinel runs the
  derived restart command autonomously, then backs off (doubling) before
  any further attempt, probing at the same cadence throughout and never
  giving up. A successful probe resets the outage. A failed unit — one the
  OS supervisor gave up on — has no HTTP to probe and is restarted on the
  same threshold-and-backoff state machine. Stopped, inert, and disabled
  services are never probed or restarted. Autonomous restarts notify and
  are recorded in history; the first restart of an outage notifies at
  normal priority and every later one escalates with a message that says
  the service is still unhealthy. All restart paths share one in-flight
  slot per service, so an autonomous restart never races a manual one.

  Scenario: A healthy running service is probed and never restarted
    Given a stub service whose health endpoint answers 200
    And the stub service is discovered as "plate-solver" in state "running"
    And sentinel is running with no monitors
    When the dashboard reports service "plate-solver" health "up"
    Then the service manager records no restarts after a settle period

  Scenario: An auth-challenging service counts as alive
    Given a stub service whose health endpoint answers 401
    And the stub service is discovered as "plate-solver" in state "running"
    And sentinel is running with no monitors
    When the dashboard reports service "plate-solver" health "up"
    Then the service manager records no restarts after a settle period

  Scenario: Consecutive failed probes trigger an autonomous restart
    Given a stub service whose health endpoint answers 503
    And the stub service is discovered as "plate-solver" in state "running"
    And sentinel is running with notifiers and no monitors
    Then the service manager records at least 1 restart of "rusty-photon-plate-solver" within 10 seconds
    And the dashboard reports service "plate-solver" health "down"
    And the notification history records an autonomous restart of "plate-solver"

  Scenario: A restart that cures the service ends the outage
    Given a stub service whose health endpoint answers 503
    And the stub service is discovered as "plate-solver" in state "running"
    And sentinel is running with no monitors
    When the service manager records at least 1 restart of "rusty-photon-plate-solver" within 10 seconds
    And the stub service becomes healthy
    Then the dashboard reports service "plate-solver" health "up"
    And the dashboard reports zero restarts in the current outage for "plate-solver"

  Scenario: Restarts that never cure the service back off and escalate
    Given a stub service whose health endpoint answers 503
    And the stub service is discovered as "plate-solver" in state "running"
    And sentinel is running with notifiers and no monitors
    Then the service manager records at least 2 restarts of "rusty-photon-plate-solver" within 15 seconds
    And the notification history records an escalated still-unhealthy notification for "plate-solver"
    And the dashboard reports a scheduled next restart for "plate-solver"

  Scenario: A failed unit is restarted without an HTTP probe
    Given a discovered unit "rusty-photon-qhy-focuser" in state "failed"
    And sentinel is running with no monitors
    Then the service manager records at least 1 restart of "rusty-photon-qhy-focuser" within 10 seconds

  Scenario: A stopped service is never probed or restarted
    Given a stub service whose health endpoint answers 503
    And the stub service is discovered as "plate-solver" in state "stopped"
    And sentinel is running with no monitors
    When the services endpoint is requested
    Then the services response lists "plate-solver" with health "unknown"
    And the service manager records no restarts after a settle period
