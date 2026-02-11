@serial
Feature: Concurrent operation safety
  The safety monitor must handle concurrent operations
  without panics or data races.

  Scenario: Concurrent connection state changes
    Given a monitoring file containing "test"
    And a device configured to monitor this file
    When 10 tasks toggle the connection state while 10 tasks read it
    Then no panics should occur

  Scenario: Concurrent safety evaluations are correct
    Given case-sensitive matching
    And a contains rule with pattern "SAFE" that evaluates to safe
    And a device configured with these rules
    When 5 tasks evaluate "SAFE operation" and "unsafe operation" 10 times each
    Then all "SAFE operation" results should be safe
    And all "unsafe operation" results should be unsafe

  Scenario: Stress test with mixed concurrent operations
    Given a monitoring file containing "test"
    And a device configured to monitor this file
    When 20 tasks perform mixed operations concurrently
    Then no panics should occur
