Feature: Movement control
  The focuser can be moved to absolute positions and halted.

  Scenario: Move to valid position succeeds and sets is-moving
    Given a focuser device with standard mock responses and a move response
    When I connect the device
    And I move the focuser to position 20000
    Then the cached target position should be 20000
    And the cached is-moving should be true

  Scenario: Move to negative position rejected with invalid-value
    Given a focuser device with standard mock responses
    When I connect the device
    And I try to move the focuser to position -1
    Then the operation should fail with invalid-value

  Scenario: Move beyond max step rejected with invalid-value
    Given a focuser device with standard mock responses
    When I connect the device
    And I try to move the focuser to position 100000
    Then the operation should fail with invalid-value

  Scenario: Move fails when not connected
    Given a focuser device with standard mock responses
    When I try to move the focuser to position 5000
    Then the operation should fail with not-connected

  Scenario: Halt stops movement and clears is-moving
    Given a focuser device with standard mock responses and move then abort responses
    When I connect the device
    And I move the focuser to position 20000
    And I halt the focuser
    Then the cached is-moving should be false
    And the cached target position should be empty

  Scenario: Halt fails when not connected
    Given a focuser device with standard mock responses
    When I try to halt the focuser
    Then the operation should fail with not-connected

  Scenario: Move completion detected when position reaches target
    Given a serial manager with responses for move then position-at-target 5000
    When I connect the serial manager
    And I send a move-absolute command to 5000
    Then the cached is-moving should be true
    When I refresh the position
    Then the cached is-moving should be false
    And the cached target position should be empty
    And the cached position should be 5000

  Scenario: Set speed succeeds when connected
    Given a serial manager with standard mock responses and a set-speed response
    When I connect the serial manager
    And I set the speed to 5
    Then the operation should succeed

  Scenario: Set speed fails when not connected
    Given a serial manager with no responses
    When I try to set the speed to 5
    Then the serial manager operation should fail with not-connected

  Scenario: Set reverse succeeds when connected
    Given a serial manager with standard mock responses and a set-reverse response
    When I connect the serial manager
    And I set reverse to true
    Then the operation should succeed

  Scenario: Set reverse fails when not connected
    Given a serial manager with no responses
    When I try to set reverse to true
    Then the serial manager operation should fail with not-connected
