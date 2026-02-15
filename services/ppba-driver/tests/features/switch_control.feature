Feature: Switch Control
  As an ASCOM client
  I want to control PPBA switches
  So that I can manage power outputs and dew heaters

  Scenario: Get boolean switch value when connected
    Given a switch device with standard mock responses
    When I connect the switch device
    And I wait for status cache
    Then switch 0 value should be 1.0

  Scenario: Get switch boolean when connected
    Given a switch device with standard mock responses
    When I connect the switch device
    And I wait for status cache
    Then switch 0 boolean should be true

  Scenario: Set boolean switch value
    Given a switch device with standard mock responses
    When I connect the switch device
    Then setting switch 0 boolean to true should succeed

  Scenario: Switches 0 to 5 are writable with auto-dew off
    Given a switch device with standard mock responses
    When I connect the switch device
    Then switches 0 through 5 should be writable

  Scenario: Switches 6 to 15 are read-only
    Given a switch device with standard mock responses
    When I connect the switch device
    Then switches 6 through 15 should not be writable

  Scenario: Auto-dew enabled makes dew heaters not writable
    Given a switch device with auto-dew enabled mock responses
    When I connect the switch device
    And I wait for status cache
    Then switch 2 should not be writable
    And switch 3 should not be writable

  Scenario: Auto-dew enabled leaves other switches writable
    Given a switch device with auto-dew enabled mock responses
    When I connect the switch device
    And I wait for status cache
    Then switch 0 should be writable
    And switch 1 should be writable
    And switch 4 should be writable
    And switch 5 should be writable

  Scenario: Writing dew heater with auto-dew enabled returns INVALID_OPERATION
    Given a switch device with auto-dew enabled mock responses
    When I connect the switch device
    And I wait for status cache
    And I try to set switch 2 value to 100.0
    Then the last error code should be INVALID_OPERATION

  Scenario: USB hub set uses special PU command path
    Given a switch device with USB hub mock responses
    When I connect the switch device
    And I wait for status cache
    And I set switch 4 value to 1.0
    Then switch 4 value should be 1.0

  Scenario: Auto-dew toggle uses PD command and refreshes
    Given a switch device with auto-dew toggle mock responses
    When I connect the switch device
    And I wait for status cache
    Then setting switch 5 value to 1.0 should succeed

  Scenario: Read-only sensor write is rejected
    Given a switch device with standard mock responses
    When I connect the switch device
    And I try to set switch 10 value to 12.0
    Then the last error code should be NOT_IMPLEMENTED

  Scenario: All switches are queryable when connected
    Given a switch device with standard mock responses
    When I connect the switch device
    And I wait for status cache
    Then all 16 switches should be queryable for name, description, min, max, step, value, and can_write

  Scenario: Boolean switch conversions work for all controllable switches
    Given a switch device with standard mock responses
    When I connect the switch device
    And I wait for status cache
    Then get_switch should work for switches 0 through 5

  Scenario: All writable switches identified correctly
    Given a switch device with standard mock responses
    When I connect the switch device
    Then switches 0 through 5 should be writable
    And switches 6 through 15 should not be writable
