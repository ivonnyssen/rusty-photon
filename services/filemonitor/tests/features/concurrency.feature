@serial
Feature: Concurrent operation safety
  The safety monitor must handle concurrent HTTP operations
  without panics or data races.

  Scenario: Concurrent connection state changes
    Given a monitoring file containing "test"
    And filemonitor is running
    When 10 tasks toggle the connection while 10 tasks read it
    Then no panics should occur

  Scenario: Concurrent safety evaluations return consistent results
    Given case-sensitive matching
    And a contains rule with pattern "SAFE" that evaluates to safe
    And a monitoring file containing "SAFE operation"
    And filemonitor is running with these rules
    When I connect the device
    And 50 tasks check is_safe concurrently
    Then all concurrent is_safe results should be true

  Scenario: Stress test with mixed concurrent operations
    Given a monitoring file containing "test"
    And filemonitor is running
    When 20 tasks perform mixed operations concurrently
    Then no panics should occur
