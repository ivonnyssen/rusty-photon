@serial
Feature: File content polling
  The safety monitor polls the monitored file at a configured interval.
  Changes to the file are reflected in subsequent safety evaluations.

  Scenario: Polling detects file content changes
    Given case-insensitive matching
    And a contains rule with pattern "UPDATED" that evaluates to safe
    And a monitoring file containing "initial"
    And a device configured with these rules and monitoring this file with 1 second polling
    When I connect the device
    And the file content changes to "UPDATED"
    And I wait for the polling interval to elapse
    Then is_safe should return true
