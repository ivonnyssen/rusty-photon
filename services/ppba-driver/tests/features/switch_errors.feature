Feature: Switch Errors
  As an ASCOM client
  I want proper error handling from the switch device
  So that I can handle failures gracefully

  Scenario: get_switch_value returns NOT_CONNECTED when disconnected
    Given a running PPBA server
    When I try to get switch 0 value
    Then the last error code should be NOT_CONNECTED

  Scenario: get_switch returns NOT_CONNECTED when disconnected
    Given a running PPBA server
    When I try to get switch 0 boolean
    Then the last error code should be NOT_CONNECTED

  Scenario: set_switch returns NOT_CONNECTED when disconnected
    Given a running PPBA server
    When I try to set switch 0 boolean to true
    Then the last error code should be NOT_CONNECTED

  Scenario: set_switch_value returns NOT_CONNECTED when disconnected
    Given a running PPBA server
    When I try to set switch 2 value to 128.0
    Then the last error code should be NOT_CONNECTED

  Scenario: can_write returns NOT_CONNECTED when disconnected
    Given a running PPBA server
    When I try to query can_write for switch 0
    Then the last error code should be NOT_CONNECTED

  Scenario: Invalid switch ID 16 fails for all operations
    Given a running PPBA server with the switch connected
    Then all operations on switch 16 should fail

  Scenario: Invalid switch ID 99 fails for all metadata operations
    Given a running PPBA server with the switch connected
    Then switch 99 name query should fail
    And switch 99 description query should fail
    And switch 99 min value query should fail
    And switch 99 max value query should fail
    And switch 99 step query should fail

  Scenario: Invalid switch IDs fail for all operations
    Given a running PPBA server with the switch connected
    When I wait for the switch data to be available
    Then operations on invalid switch IDs 16, 17, 100, 999 should all fail

  Scenario: Setting value out of range returns INVALID_VALUE
    Given a running PPBA server with the switch connected
    When I try to set switch 2 value to 300.0
    Then the last error code should be INVALID_VALUE

  Scenario: Setting negative value is rejected
    Given a running PPBA server with the switch connected
    When I try to set switch 2 value to -10.0
    Then the last operation should have failed

  Scenario: Setting value exceeding boolean max is rejected
    Given a running PPBA server with the switch connected
    When I try to set switch 0 value to 2.0
    Then the last operation should have failed

  Scenario: SwitchNotWritable maps to NOT_IMPLEMENTED
    Given a running PPBA server with the switch connected
    When I try to set switch 10 value to 0.0
    Then the last error code should be NOT_IMPLEMENTED

  Scenario: InvalidSwitchId maps to INVALID_VALUE for get
    Given a running PPBA server with the switch connected
    When I wait for the switch data to be available
    And I try to get switch 99 value
    Then the last error code should be INVALID_VALUE

  Scenario: InvalidSwitchId maps to INVALID_VALUE for set
    Given a running PPBA server with the switch connected
    When I try to set switch 99 value to 0.0
    Then the last error code should be INVALID_VALUE

  Scenario: InvalidValue maps to INVALID_VALUE
    Given a running PPBA server with the switch connected
    When I try to set switch 2 value to -1.0
    Then the last error code should be INVALID_VALUE

  Scenario: can_async returns false for all valid switches
    Given a running PPBA server with the switch connected
    Then can_async should return false for all 16 switches

  Scenario: state_change_complete returns true for all valid switches
    Given a running PPBA server with the switch connected
    Then state_change_complete should return true for all 16 switches

  Scenario: cancel_async succeeds for all valid switches
    Given a running PPBA server with the switch connected
    Then cancel_async should succeed for all 16 switches

  Scenario: can_async with invalid switch ID fails
    Given a running PPBA server
    When I try to query can_async for switch 16
    Then the last operation should have failed

  Scenario: state_change_complete with invalid switch ID fails
    Given a running PPBA server with the switch connected
    When I try to query state_change_complete for switch 16
    Then the last operation should have failed

  Scenario: cancel_async with invalid switch ID fails
    Given a running PPBA server with the switch connected
    When I try to call cancel_async on switch 16
    Then the last operation should have failed

  Scenario: set_async with invalid switch ID fails
    Given a running PPBA server with the switch connected
    When I try to call set_async on switch 16
    Then the last operation should have failed

  Scenario: set_async_value with invalid switch ID fails
    Given a running PPBA server with the switch connected
    When I try to call set_async_value on switch 16
    Then the last operation should have failed

  Scenario: can_async returns NOT_CONNECTED when disconnected
    Given a running PPBA server
    When I try to query can_async for switch 0
    Then the last error code should be NOT_CONNECTED

  Scenario: state_change_complete returns NOT_CONNECTED when disconnected
    Given a running PPBA server
    When I try to query state_change_complete for switch 0
    Then the last error code should be NOT_CONNECTED

  Scenario: cancel_async returns NOT_CONNECTED when disconnected
    Given a running PPBA server
    When I try to call cancel_async on switch 0
    Then the last error code should be NOT_CONNECTED

  Scenario: set_async returns NOT_CONNECTED when disconnected
    Given a running PPBA server
    When I try to call set_async on switch 0
    Then the last error code should be NOT_CONNECTED

  Scenario: set_async_value returns NOT_CONNECTED when disconnected
    Given a running PPBA server
    When I try to call set_async_value on switch 0
    Then the last error code should be NOT_CONNECTED
