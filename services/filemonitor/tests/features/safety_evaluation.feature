Feature: Safety evaluation rules
  The safety monitor evaluates file content against configured parsing rules.
  Parsing rules are evaluated in order; the first match determines safety.
  No match defaults to unsafe.

  Scenario Outline: Contains rule evaluation
    Given case-insensitive matching
    And a contains rule with pattern "OPEN" that evaluates to safe
    And a contains rule with pattern "CLOSED" that evaluates to unsafe
    And a device configured with these rules
    When I evaluate the safety of "<content>"
    Then the result should be <expected>

    Examples:
      | content              | expected |
      | Roof Status: OPEN    | safe     |
      | Roof Status: CLOSED  | unsafe   |
      | roof status: open    | safe     |
      | roof status: closed  | unsafe   |
      | Unknown status       | unsafe   |

  Scenario: Case-sensitive match succeeds with exact case
    Given case-sensitive matching
    And a contains rule with pattern "OPEN" that evaluates to safe
    And a device configured with these rules
    When I evaluate the safety of "Status: OPEN"
    Then the result should be safe

  Scenario: Case-sensitive match fails with wrong case
    Given case-sensitive matching
    And a contains rule with pattern "OPEN" that evaluates to safe
    And a device configured with these rules
    When I evaluate the safety of "Status: open"
    Then the result should be unsafe

  Scenario: Regex rule matches safe pattern
    Given case-insensitive matching
    And a regex rule with pattern "safe_or_ok" that evaluates to safe
    And a regex rule with pattern "danger_or_error" that evaluates to unsafe
    And a device configured with these rules
    When I evaluate the safety of "Status: SAFE"
    Then the result should be safe

  Scenario: Regex rule matches safe pattern with whitespace
    Given case-insensitive matching
    And a regex rule with pattern "safe_or_ok" that evaluates to safe
    And a regex rule with pattern "danger_or_error" that evaluates to unsafe
    And a device configured with these rules
    When I evaluate the safety of "Status:   OK"
    Then the result should be safe

  Scenario: Regex rule matches unsafe pattern
    Given case-insensitive matching
    And a regex rule with pattern "safe_or_ok" that evaluates to safe
    And a regex rule with pattern "danger_or_error" that evaluates to unsafe
    And a device configured with these rules
    When I evaluate the safety of "Status: DANGER"
    Then the result should be unsafe

  Scenario: Regex rule matches unsafe pattern without whitespace
    Given case-insensitive matching
    And a regex rule with pattern "safe_or_ok" that evaluates to safe
    And a regex rule with pattern "danger_or_error" that evaluates to unsafe
    And a device configured with these rules
    When I evaluate the safety of "Status:ERROR"
    Then the result should be unsafe

  Scenario: Regex rule no match defaults to unsafe
    Given case-insensitive matching
    And a regex rule with pattern "safe_or_ok" that evaluates to safe
    And a regex rule with pattern "danger_or_error" that evaluates to unsafe
    And a device configured with these rules
    When I evaluate the safety of "Status: UNKNOWN"
    Then the result should be unsafe

  Scenario: First matching rule wins
    Given case-insensitive matching
    And a contains rule with pattern "SAFE" that evaluates to safe
    And a contains rule with pattern "SAFE" that evaluates to unsafe
    And a device configured with these rules
    When I evaluate the safety of "Status: SAFE"
    Then the result should be safe

  Scenario: Invalid regex pattern defaults to unsafe
    Given case-insensitive matching
    And a regex rule with pattern "invalid_bracket" that evaluates to safe
    And a device configured with these rules
    When I evaluate the safety of "any content"
    Then the result should be unsafe

  Scenario: Disconnected device reports unsafe via is_safe
    Given a monitoring file containing "OPEN"
    And a contains rule with pattern "OPEN" that evaluates to safe
    And a device configured with these rules and monitoring this file
    Then is_safe should return false

  Scenario: Connected device with no matching rules reports unsafe
    Given a monitoring file containing "UNKNOWN STATUS"
    And a device configured with no rules and monitoring this file
    When I connect the device
    Then is_safe should return false

  Scenario: Device reports ASCOM metadata from configuration
    Given a configuration file at "tests/config.json"
    When I load the configuration
    And I create a device from the configuration
    Then the static name should be "File Safety Monitor"
    And the unique ID should be "filemonitor-001"
    And the description should be "ASCOM Alpaca SafetyMonitor that monitors file content"
    And the driver info should be "ASCOM Alpaca SafetyMonitor that monitors file content"
    And the driver version should be "0.1.0"
