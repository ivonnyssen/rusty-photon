Feature: Dashboard rendering
  The sentinel dashboard renders monitor statuses and notification
  history as HTML for users to view in a browser.

  Scenario: Dashboard shows monitor with poll data
    Given a monitor "Sky" with poll timestamp 1700000000000 in state "Safe"
    When the dashboard index page is requested
    Then the response should contain "Sky"
    And the response should contain a time script for epoch 1700000000000

  Scenario: Dashboard shows notification history
    Given a monitor "Sky" in the dashboard state
    And a notification record for "Sky" with message "weather alert" that succeeded
    And a notification record for "Sky" with message "recovery notice" that failed
    When the dashboard index page is requested
    Then the response should contain "weather alert"
    And the response should contain "recovery notice"
    And the response should contain "OK"
    And the response should contain "Failed"
