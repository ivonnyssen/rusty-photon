Feature: Service lifecycle
  Sentinel starts as a binary, serves a dashboard, and shuts down
  cleanly when stopped. It connects to configured monitors on startup.

  Scenario: Sentinel starts with empty config and dashboard is healthy
    Given sentinel is running with no monitors
    Then the dashboard health endpoint should return OK

  Scenario: Sentinel starts with a monitored filemonitor device
    Given a monitoring file containing "OPEN"
    And filemonitor is running with a contains rule "OPEN" as safe
    And sentinel is configured to monitor the filemonitor
    And sentinel is running
    Then the dashboard health endpoint should return OK

  Scenario: Sentinel fails to start with missing config file
    When I try to start sentinel with config "nonexistent_config.json"
    Then sentinel should fail to start
