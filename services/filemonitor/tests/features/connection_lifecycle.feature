Feature: Device connection lifecycle
  The safety monitor can be connected and disconnected.
  Connection requires the monitored file to exist.

  Scenario: Device starts disconnected
    Given a monitoring file containing "test content"
    And a device configured to monitor this file
    Then the device should be disconnected

  Scenario: Device is connected after connect
    Given a monitoring file containing "test content"
    And a device configured to monitor this file
    When I connect the device
    Then the device should be connected

  Scenario: Device is disconnected after connect then disconnect
    Given a monitoring file containing "test content"
    And a device configured to monitor this file
    When I connect the device
    And I disconnect the device
    Then the device should be disconnected

  Scenario: Reconnection reloads file content
    Given a monitoring file containing "CLOSED"
    And a contains rule with pattern "OPEN" that evaluates to safe
    And a contains rule with pattern "CLOSED" that evaluates to unsafe
    And a device configured with these rules and monitoring this file
    When I connect the device
    Then is_safe should return false
    When the file content changes to "OPEN"
    And I disconnect the device
    And I connect the device
    Then is_safe should return true

  Scenario: Double disconnect is safe
    Given a monitoring file containing "test content"
    And a device configured to monitor this file
    When I connect the device
    And I disconnect the device
    And I disconnect the device
    Then the device should be disconnected

  Scenario: Fail to connect when monitored file does not exist
    Given a device configured to monitor "/nonexistent/path/file.txt"
    When I try to connect the device
    Then connecting should fail with an error
