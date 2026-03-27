Feature: Dashboard API
  The sentinel dashboard provides JSON API endpoints for monitor status
  and notification history that are reachable over HTTP.

  Scenario: Health endpoint returns OK
    Given sentinel is running with no monitors
    When the health endpoint is requested
    Then the response status should be 200
    And the response body should be "OK"

  Scenario: Status endpoint returns monitor data after polling
    Given a monitoring file containing "OPEN"
    And filemonitor is running with a contains rule "OPEN" as safe
    And sentinel is configured to monitor the filemonitor
    And sentinel is running
    When I wait for sentinel to poll
    And the status API endpoint is requested
    Then the response should be a JSON array with 1 entry
    And the first entry should have name "Roof Monitor"
    And the first entry should have state "Safe"

  Scenario: History endpoint starts empty
    Given sentinel is running with no monitors
    When the history API endpoint is requested
    Then the response should be an empty JSON array
